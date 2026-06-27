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
    BenchmarkConfig, BenchmarkRunner, BenchmarkRunnerReport, StreamingIoSample, TerminalUi,
    UiAction,
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
        if let Some(run) = &running {
            for sample in run.samples.try_iter() {
                ui.observe_sample(sample);
                should_render = true;
            }
        }

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
    samples: Receiver<StreamingIoSample>,
    done: Receiver<Result<BenchmarkRunnerReport, String>>,
}

fn spawn_benchmark(config: BenchmarkConfig) -> RunningBenchmark {
    let stop = Arc::new(AtomicBool::new(false));
    let should_stop = Arc::clone(&stop);
    let (sample_tx, samples) = mpsc::channel();
    let (done_tx, done) = mpsc::channel();

    thread::spawn(move || {
        let result = BenchmarkRunner::default()
            .run(
                &config,
                |sample| {
                    let _ = sample_tx.send(sample);
                },
                || should_stop.load(Ordering::Relaxed),
            )
            .map_err(|error| error.to_string());
        let _ = done_tx.send(result);
    });

    RunningBenchmark {
        stop,
        samples,
        done,
    }
}

fn finish_completed_run(running: &mut Option<RunningBenchmark>, ui: &mut TerminalUi) -> bool {
    let Some(run) = running else {
        return false;
    };

    match run.done.try_recv() {
        Ok(result) => {
            for sample in run.samples.try_iter() {
                ui.observe_sample(sample);
            }
            ui.finish_run_with_passes(
                match &result {
                    Ok(report) if report.stopped => String::from("Stopped"),
                    Ok(report) => format!("Done - {} passes", report.passes.len()),
                    Err(error) => format!("Error - {error}"),
                },
                result.ok().map_or_else(Vec::new, |report| report.passes),
            );
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
    use studiofs_bench::StreamingIoPhase;

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
            samples: mpsc::channel().1,
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
        let mut running = Some(RunningBenchmark {
            stop,
            samples: mpsc::channel().1,
            done,
        });

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
        let mut running = Some(RunningBenchmark {
            stop,
            samples: mpsc::channel().1,
            done,
        });

        let changed = finish_completed_run(&mut running, &mut ui);

        assert!(!changed);
        assert!(running.is_some());
        assert!(ui.is_running());
    }

    #[test]
    fn finish_completed_run_drains_final_samples_before_clearing_run() {
        let mut ui = TerminalUi::default();
        ui.handle_action(UiAction::Submit);
        let stop = Arc::new(AtomicBool::new(false));
        let (sample_tx, samples) = mpsc::channel();
        let (done_tx, done) = mpsc::channel();
        sample_tx
            .send(StreamingIoSample {
                phase: StreamingIoPhase::Write,
                pass_number: 1,
                timestamp: std::time::SystemTime::UNIX_EPOCH,
                offset: 0,
                bytes_processed: 500_000_000,
                mb_per_second: 125.0,
            })
            .unwrap();
        done_tx.send(Ok(stopped_report())).unwrap();
        let mut running = Some(RunningBenchmark {
            stop,
            samples,
            done,
        });

        let changed = finish_completed_run(&mut running, &mut ui);

        assert!(changed);
        assert!(running.is_none());
        assert_render_contains(&ui, "125.0 MB/s");
    }

    #[test]
    fn replace_running_stops_previous_run_before_replacement() {
        let stop = Arc::new(AtomicBool::new(false));
        let (done_tx, done) = mpsc::channel();
        done_tx.send(Ok(stopped_report())).unwrap();
        let mut running = Some(RunningBenchmark {
            stop: Arc::clone(&stop),
            samples: mpsc::channel().1,
            done,
        });

        let completed = stop_running(running.take());

        assert!(completed);
        assert!(stop.load(Ordering::Relaxed));
        assert!(running.is_none());
    }

    fn assert_render_contains(ui: &TerminalUi, expected: &str) {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(96, 24)).unwrap();

        terminal.draw(|frame| ui.render(frame)).unwrap();

        assert!(terminal.backend().to_string().contains(expected));
    }
}
