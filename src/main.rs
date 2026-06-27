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

    while !ui.should_exit() {
        finish_completed_run(&mut running, &mut ui);

        terminal.draw(|frame| ui.render(frame))?;

        if !event::poll(Duration::from_millis(100))? {
            continue;
        }

        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }

        let was_running = ui.is_running();
        if let Some(action) = key_action(key.code) {
            ui.handle_action(action);
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

fn finish_completed_run(running: &mut Option<RunningBenchmark>, ui: &mut TerminalUi) {
    let Some(run) = running else {
        return;
    };

    match run.done.try_recv() {
        Ok(result) => {
            ui.finish_run(match result {
                Ok(report) if report.stopped => String::from("Stopped"),
                Ok(report) => format!("Done - {} passes", report.passes.len()),
                Err(error) => format!("Error - {error}"),
            });
            *running = None;
        }
        Err(mpsc::TryRecvError::Disconnected) => {
            ui.finish_run("Error - benchmark thread stopped unexpectedly");
            *running = None;
        }
        Err(mpsc::TryRecvError::Empty) => {}
    }
}

fn stop_running(running: Option<RunningBenchmark>) {
    if let Some(run) = running {
        run.stop.store(true, Ordering::Relaxed);
        let _ = run.done.recv();
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

        stop_running(Some(RunningBenchmark {
            stop: Arc::clone(&stop),
            done,
        }));

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

        finish_completed_run(&mut running, &mut ui);

        assert!(running.is_none());
        assert!(!ui.is_running());
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

        stop_running(running.take());

        assert!(stop.load(Ordering::Relaxed));
        assert!(running.is_none());
    }
}
