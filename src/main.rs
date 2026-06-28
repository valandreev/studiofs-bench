//! Terminal UI entrypoint for studiofs-bench.

use std::{
    env,
    fmt::Write as _,
    io::{self, IsTerminal},
    path::PathBuf,
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
    BenchmarkConfig, BenchmarkPassReport, BenchmarkRunner, BenchmarkRunnerReport, CacheMode,
    DiskTestMode, ExecutionMode, FileLayout, RunMode, StreamingIoSample, TerminalUi, UiAction,
    Workload, WorkloadSize,
};

fn main() -> io::Result<()> {
    let args = env::args().skip(1).collect::<Vec<_>>();
    if args.first().is_some_and(|arg| arg == "--scripted") {
        return run_scripted(&args[1..]).map_err(io::Error::other);
    }

    if !io::stdout().is_terminal() {
        println!("studiofs-bench");
        return Ok(());
    }

    let mut terminal = ratatui::init();
    let result = run(&mut terminal);
    ratatui::restore();
    result
}

fn run_scripted(args: &[String]) -> Result<(), String> {
    let options = ScriptedOptions::parse(args)?;
    let runner = BenchmarkRunner::default();
    let report = if let Some(bytes) = options.workload_bytes {
        let workload = Workload::create_for_bytes(
            &options.config.target_path,
            bytes,
            options.config.file_layout,
        )
        .map_err(|error| error.to_string())?;
        runner
            .run_workload(workload, &options.config, |_| {}, || false)
            .map_err(|error| error.to_string())?
    } else {
        runner
            .run(&options.config, |_| {}, || false)
            .map_err(|error| error.to_string())?
    };

    if let Some(path) = &options.report_path {
        save_reports(path, &options.config, options.workload_bytes, &report)?;
    }

    println!("Done - {} passes", report.passes.len());
    Ok(())
}

struct ScriptedOptions {
    config: BenchmarkConfig,
    workload_bytes: Option<u64>,
    report_path: Option<PathBuf>,
}

impl ScriptedOptions {
    fn parse(args: &[String]) -> Result<Self, String> {
        let mut config = BenchmarkConfig::for_target(PathBuf::from("."));
        config.save_report = false;
        let mut workload_bytes = None;
        let mut report_path = None;
        let mut args = args.iter().map(String::as_str);

        while let Some(arg) = args.next() {
            match arg {
                "--target" => config.target_path = PathBuf::from(next_arg(&mut args)?),
                "--workload-gb" => {
                    config.workload_size = WorkloadSize::CustomGb(parse_u64(next_arg(&mut args)?)?)
                }
                "--workload-bytes" => workload_bytes = Some(parse_u64(next_arg(&mut args)?)?),
                "--run-mode" => config.run_mode = parse_run_mode(next_arg(&mut args)?)?,
                "--mode" => config.test_mode = parse_test_mode(next_arg(&mut args)?)?,
                "--layout" => config.file_layout = parse_layout(next_arg(&mut args)?)?,
                "--file-size-mb" => {
                    config.file_layout =
                        FileLayout::FixedFileSizeMb(parse_u64(next_arg(&mut args)?)?)
                }
                "--cache" => config.cache_mode = parse_cache_mode(next_arg(&mut args)?)?,
                "--execution" => {
                    config.execution_mode = parse_execution_mode(next_arg(&mut args)?)?
                }
                "--keep-files" => config.keep_files = true,
                "--save-report" => {
                    config.save_report = true;
                    report_path = Some(PathBuf::from(next_arg(&mut args)?));
                }
                value => return Err(format!("unknown argument: {value}")),
            }
        }

        if config.execution_mode == ExecutionMode::Continuous {
            return Err(String::from(
                "scripted mode does not support continuous execution",
            ));
        }

        Ok(Self {
            config,
            workload_bytes,
            report_path,
        })
    }
}

fn next_arg<'a>(args: &mut impl Iterator<Item = &'a str>) -> Result<&'a str, String> {
    args.next()
        .ok_or_else(|| String::from("missing argument value"))
}

fn parse_u64(value: &str) -> Result<u64, String> {
    value
        .parse()
        .map_err(|_| format!("invalid unsigned integer: {value}"))
}

fn parse_run_mode(value: &str) -> Result<RunMode, String> {
    match value {
        "local" | "local-filesystem" => Ok(RunMode::LocalFilesystem),
        "mounted" | "mounted-filesystem" => Ok(RunMode::MountedFilesystem),
        _ => Err(format!("invalid run mode: {value}")),
    }
}

fn parse_test_mode(value: &str) -> Result<DiskTestMode, String> {
    match value {
        "read-write" => Ok(DiskTestMode::ReadWrite),
        "write-only" => Ok(DiskTestMode::WriteOnly),
        "write-once-read-loop" => Ok(DiskTestMode::WriteOnceReadLoop),
        _ => Err(format!("invalid mode: {value}")),
    }
}

fn parse_layout(value: &str) -> Result<FileLayout, String> {
    match value {
        "single-file" => Ok(FileLayout::SingleFile),
        "hundred-files-plus-minus-five" => Ok(FileLayout::HundredFilesPlusMinusFive),
        _ => Err(format!("invalid layout: {value}")),
    }
}

fn parse_cache_mode(value: &str) -> Result<CacheMode, String> {
    match value {
        "enabled" => Ok(CacheMode::Enabled),
        "disabled" => Ok(CacheMode::Disabled),
        _ => Err(format!("invalid cache mode: {value}")),
    }
}

fn parse_execution_mode(value: &str) -> Result<ExecutionMode, String> {
    match value {
        "run-once" => Ok(ExecutionMode::RunOnce),
        "continuous" => Ok(ExecutionMode::Continuous),
        _ => Err(format!("invalid execution mode: {value}")),
    }
}

fn save_reports(
    path: &std::path::Path,
    config: &BenchmarkConfig,
    workload_bytes: Option<u64>,
    report: &BenchmarkRunnerReport,
) -> Result<(), String> {
    let json_path = path.with_extension("json");
    let csv_path = path.with_extension("csv");
    let payload = serde_json::json!({
        "config": config,
        "workload_bytes": workload_bytes,
        "report": report,
    });
    let json = serde_json::to_vec_pretty(&payload).map_err(|error| error.to_string())?;
    std::fs::write(json_path, json).map_err(|error| error.to_string())?;
    std::fs::write(csv_path, passes_csv(&report.passes)).map_err(|error| error.to_string())?;
    Ok(())
}

fn passes_csv(passes: &[BenchmarkPassReport]) -> String {
    let mut csv = String::from(
        "phase,pass_number,bytes_processed,stopped,sample_count,average_mb_per_second,stable_mb_per_second,minimum_mb_per_second,drop_count\n",
    );
    for pass in passes {
        let _ = writeln!(
            csv,
            "{:?},{},{},{},{},{},{},{},{}",
            pass.phase,
            pass.pass_number,
            pass.bytes_processed,
            pass.stopped,
            pass.metrics.sample_count,
            pass.metrics.average_mb_per_second,
            pass.metrics.stable_mb_per_second,
            pass.metrics.minimum_mb_per_second,
            pass.metrics.drop_count
        );
    }
    csv
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
            match result {
                Ok(report) => {
                    let message = if report.stopped {
                        String::from("Stopped")
                    } else {
                        format!("Done - {} passes", report.passes.len())
                    };
                    ui.finish_run_with_passes(message, report.passes);
                }
                Err(error) => ui.finish_run(format!("Error - {error}")),
            }
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
    fn finish_completed_run_preserves_pass_summaries_when_run_errors() {
        let mut ui = TerminalUi::default();
        ui.handle_action(UiAction::Submit);
        ui.observe_sample(StreamingIoSample {
            phase: StreamingIoPhase::Write,
            pass_number: 1,
            timestamp: std::time::SystemTime::UNIX_EPOCH,
            offset: 0,
            bytes_processed: 1_000_000,
            mb_per_second: 100.0,
        });
        ui.observe_sample(StreamingIoSample {
            phase: StreamingIoPhase::Read,
            pass_number: 1,
            timestamp: std::time::SystemTime::UNIX_EPOCH,
            offset: 0,
            bytes_processed: 500_000,
            mb_per_second: 80.0,
        });
        let stop = Arc::new(AtomicBool::new(false));
        let (_sample_tx, samples) = mpsc::channel();
        let (done_tx, done) = mpsc::channel();
        done_tx.send(Err(String::from("failed"))).unwrap();
        let mut running = Some(RunningBenchmark {
            stop,
            samples,
            done,
        });

        let changed = finish_completed_run(&mut running, &mut ui);

        assert!(changed);
        assert_render_contains(&ui, "write pass 1: Avg 100.0");
        assert_render_contains(&ui, "Error - failed");
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

    #[test]
    fn scripted_options_reject_continuous_execution() {
        let args = ["--execution", "continuous"].map(String::from);

        let error = match ScriptedOptions::parse(&args) {
            Ok(_) => panic!("continuous execution should be rejected"),
            Err(error) => error,
        };

        assert_eq!(error, "scripted mode does not support continuous execution");
    }

    fn assert_render_contains(ui: &TerminalUi, expected: &str) {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(96, 24)).unwrap();

        terminal.draw(|frame| ui.render(frame)).unwrap();

        assert!(terminal.backend().to_string().contains(expected));
    }
}
