//! Terminal UI entrypoint for studiofs-bench.

use std::{
    io::{self, IsTerminal},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
    },
    thread,
    time::Duration,
};

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use studiofs_bench::{
    BenchmarkConfig, BenchmarkRunner, BenchmarkRunnerReport, TerminalUi, UiAction,
};

fn main() -> io::Result<()> {
    if !io::stdout().is_terminal() {
        println!("studiofs-bench");
        return Ok(());
    }

    let mut terminal = ratatui::init();
    let result = run(&mut terminal);
    ratatui::restore();
    result
}

fn run(terminal: &mut ratatui::DefaultTerminal) -> io::Result<()> {
    let mut ui = TerminalUi::default();
    let mut running: Option<RunningBenchmark> = None;
    let mut should_render = true;

    while !ui.should_exit() {
        if finish_completed_run(&mut running, &mut ui) {
            should_render = true;
        }

        if should_render {
            terminal.draw(|frame| ui.render(frame))?;
            should_render = false;
        }

        if !event::poll(Duration::from_millis(100))? {
            continue;
        }

        match event::read()? {
            Event::Key(key) => {
                if key.kind != KeyEventKind::Press {
                    continue;
                }

                let was_running = ui.is_running();
                if let Some(action) = key_action(key.code) {
                    ui.handle_action(action);
                    should_render = true;
                }

                if was_running && !ui.is_running() {
                    if let Some(run) = &running {
                        run.stop.store(true, Ordering::Relaxed);
                    }
                } else if !was_running && ui.is_running() {
                    stop_running(running.take());
                    running = Some(spawn_benchmark(ui.config().clone()));
                }
            }
            Event::Resize(_, _) => {
                should_render = true;
            }
            _ => {}
        }
    }

    stop_running(running);
    Ok(())
}

fn key_action(code: KeyCode) -> Option<UiAction> {
    match code {
        KeyCode::Up => Some(UiAction::MoveUp),
        KeyCode::Down => Some(UiAction::MoveDown),
        KeyCode::Left => Some(UiAction::PreviousValue),
        KeyCode::Right => Some(UiAction::NextValue),
        KeyCode::Enter => Some(UiAction::Submit),
        KeyCode::Esc => Some(UiAction::Cancel),
        KeyCode::Backspace => Some(UiAction::Backspace),
        KeyCode::Char(value) => Some(UiAction::InsertText(value)),
        _ => None,
    }
}

struct RunningBenchmark {
    stop: Arc<AtomicBool>,
    done: Receiver<Result<BenchmarkRunnerReport, String>>,
}

fn spawn_benchmark(config: BenchmarkConfig) -> RunningBenchmark {
    let stop = Arc::new(AtomicBool::new(false));
    let should_stop = Arc::clone(&stop);
    let (done_tx, done) = mpsc::channel();

    thread::spawn(move || {
        let result = BenchmarkRunner::default()
            .run(&config, |_| {}, || should_stop.load(Ordering::Relaxed))
            .map_err(|error| error.to_string());
        let _ = done_tx.send(result);
    });

    RunningBenchmark { stop, done }
}

fn finish_completed_run(running: &mut Option<RunningBenchmark>, ui: &mut TerminalUi) -> bool {
    let Some(run) = running else {
        return false;
    };

    match run.done.try_recv() {
        Ok(result) => {
            ui.finish_run(match result {
                Ok(report) if report.stopped => String::from("Stopped"),
                Ok(report) => format!("Done - {} passes", report.passes.len()),
                Err(error) => format!("Error - {error}"),
            });
            *running = None;
            true
        }
        Err(mpsc::TryRecvError::Disconnected) => {
            ui.finish_run("Error - benchmark thread stopped unexpectedly");
            *running = None;
            true
        }
        Err(mpsc::TryRecvError::Empty) => false,
    }
}

fn stop_running(running: Option<RunningBenchmark>) -> bool {
    if let Some(run) = running {
        run.stop.store(true, Ordering::Relaxed);
        let _ = run.done.recv();
        true
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stopped_report() -> BenchmarkRunnerReport {
        BenchmarkRunnerReport {
            run_dir: ".".into(),
            files_kept: false,
            cleanup_error: None,
            passes: Vec::new(),
            stopped: true,
        }
    }

    #[test]
    fn stop_running_requests_stop_and_waits_for_completion() {
        let stop = Arc::new(AtomicBool::new(false));
        let (done_tx, done) = mpsc::channel();

        done_tx.send(Ok(stopped_report())).unwrap();

        let completed = stop_running(Some(RunningBenchmark {
            stop: Arc::clone(&stop),
            done,
        }));

        assert!(completed);
        assert!(stop.load(Ordering::Relaxed));
    }

    #[test]
    fn finish_completed_run_handles_disconnected_channel() {
        let mut ui = TerminalUi::default();
        ui.handle_action(UiAction::Submit);
        let stop = Arc::new(AtomicBool::new(false));
        let (done_tx, done) = mpsc::channel();
        drop(done_tx);
        let mut running = Some(RunningBenchmark { stop, done });

        let changed = finish_completed_run(&mut running, &mut ui);

        assert!(changed);
        assert!(running.is_none());
        assert!(!ui.is_running());
    }

    #[test]
    fn finish_completed_run_returns_false_while_run_is_pending() {
        let mut ui = TerminalUi::default();
        ui.handle_action(UiAction::Submit);
        let stop = Arc::new(AtomicBool::new(false));
        let (_done_tx, done) = mpsc::channel();
        let mut running = Some(RunningBenchmark { stop, done });

        let changed = finish_completed_run(&mut running, &mut ui);

        assert!(!changed);
        assert!(running.is_some());
        assert!(ui.is_running());
    }

    #[test]
    fn replace_running_stops_previous_run_before_replacement() {
        let stop = Arc::new(AtomicBool::new(false));
        let (done_tx, done) = mpsc::channel();
        done_tx.send(Ok(stopped_report())).unwrap();
        let mut running = Some(RunningBenchmark {
            stop: Arc::clone(&stop),
            done,
        });

        let completed = stop_running(running.take());

        assert!(completed);
        assert!(stop.load(Ordering::Relaxed));
        assert!(running.is_none());
    }
}
