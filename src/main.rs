//! Terminal UI entrypoint for studiofs-bench.

use std::{
    env,
    fs::File,
    io::{self, IsTerminal},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
    },
    thread,
    time::{Duration, SystemTime},
};

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use studiofs_bench::{
    BenchmarkConfig, BenchmarkPassReport, BenchmarkRunner, BenchmarkRunnerReport, CacheMode,
    DiskTestMode, ExecutionMode, FileLayout, RunMode, StreamingIoPhase, StreamingIoSample,
    TerminalUi, UiAction, Workload, WorkloadSize,
};

static REPORT_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn main() -> io::Result<()> {
    let args = env::args().skip(1).collect::<Vec<_>>();
    if args.iter().any(|arg| arg == "--scripted") {
        let args = args
            .iter()
            .filter(|arg| arg.as_str() != "--scripted")
            .cloned()
            .collect::<Vec<_>>();
        return run_scripted(&args).map_err(io::Error::other);
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

    if options.config.save_report {
        let launch_dir = env::current_dir()
            .map_err(|error| format!("failed to read launch directory: {error}"))?;
        let paths = save_reports(
            &launch_dir,
            &options.config,
            options.workload_bytes,
            &report,
        )?;
        println!(
            "Reports saved - {}, {}",
            paths.json.display(),
            paths.csv.display()
        );
    }

    println!("Done - {} passes", report.passes.len());
    Ok(())
}

struct ScriptedOptions {
    config: BenchmarkConfig,
    workload_bytes: Option<u64>,
}

impl ScriptedOptions {
    fn parse(args: &[String]) -> Result<Self, String> {
        let mut config = BenchmarkConfig::for_target(PathBuf::from("."));
        config.save_report = false;
        let mut workload_bytes = None;
        let mut args = args.iter().map(String::as_str);

        while let Some(arg) = args.next() {
            match arg {
                "--target" => config.target_path = PathBuf::from(next_arg(&mut args)?),
                "--workload-gb" => {
                    config.workload_size = WorkloadSize::CustomGb(parse_u64(next_arg(&mut args)?)?);
                }
                "--workload-bytes" => workload_bytes = Some(parse_u64(next_arg(&mut args)?)?),
                "--run-mode" => config.run_mode = parse_run_mode(next_arg(&mut args)?)?,
                "--mode" => config.test_mode = parse_test_mode(next_arg(&mut args)?)?,
                "--layout" => config.file_layout = parse_layout(next_arg(&mut args)?)?,
                "--file-size-mb" => {
                    config.file_layout =
                        FileLayout::FixedFileSizeMb(parse_u64(next_arg(&mut args)?)?);
                }
                "--cache" => config.cache_mode = parse_cache_mode(next_arg(&mut args)?)?,
                "--execution" => {
                    config.execution_mode = parse_execution_mode(next_arg(&mut args)?)?;
                }
                "--keep-files" => config.keep_files = true,
                "--save-report" => {
                    config.save_report = true;
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
    launch_dir: &std::path::Path,
    config: &BenchmarkConfig,
    workload_bytes: Option<u64>,
    report: &BenchmarkRunnerReport,
) -> Result<SavedReportPaths, String> {
    if report.passes.is_empty() {
        return Err(String::from("no completed pass data; report was not saved"));
    }

    let payload = serde_json::json!({
        "run": {
            "workload_bytes": workload_bytes,
            "run_dir": report.run_dir,
            "files_kept": report.files_kept,
            "stopped": report.stopped,
            "cleanup_error": report.cleanup_error,
        },
        "platform": {
            "os": env::consts::OS,
            "arch": env::consts::ARCH,
        },
        "config": config,
        "cache_method": cache_method_label(config.cache_mode),
        "passes": report.passes,
    });
    let files = create_report_files(launch_dir)?;
    write_report_files(
        files,
        |json_file| serde_json::to_writer_pretty(json_file, &payload).map_err(io::Error::other),
        |csv_file| write_passes_csv(csv_file, &report.passes),
    )
}

#[derive(Debug)]
struct SavedReportPaths {
    json: PathBuf,
    csv: PathBuf,
}

struct OpenedReportFiles {
    paths: SavedReportPaths,
    json: File,
    csv: File,
}

fn create_report_files(launch_dir: &std::path::Path) -> Result<OpenedReportFiles, String> {
    loop {
        let counter = REPORT_COUNTER.fetch_add(1, Ordering::Relaxed);
        let prefix = report_path_prefix(launch_dir, SystemTime::now(), counter);
        let json_path = prefix.with_extension("json");
        let csv_path = prefix.with_extension("csv");
        let json = match create_new_file(&json_path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(format!(
                    "failed to create JSON report {}: {error}",
                    json_path.display()
                ));
            }
        };
        let csv = match create_new_file(&csv_path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                drop(json);
                remove_report_file(&json_path);
                continue;
            }
            Err(error) => {
                drop(json);
                remove_report_file(&json_path);
                return Err(format!(
                    "failed to create CSV report {}: {error}",
                    csv_path.display()
                ));
            }
        };

        return Ok(OpenedReportFiles {
            paths: SavedReportPaths {
                json: json_path,
                csv: csv_path,
            },
            json,
            csv,
        });
    }
}

fn report_path_prefix(
    launch_dir: &std::path::Path,
    timestamp: SystemTime,
    counter: u64,
) -> PathBuf {
    let seconds = timestamp
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    launch_dir.join(format!("studiofs-bench-report-{seconds}-{counter}"))
}

fn create_new_file(path: &std::path::Path) -> io::Result<File> {
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
}

fn write_report_files(
    files: OpenedReportFiles,
    write_json: impl FnOnce(&mut File) -> io::Result<()>,
    write_csv: impl FnOnce(&mut File) -> io::Result<()>,
) -> Result<SavedReportPaths, String> {
    let OpenedReportFiles {
        paths,
        mut json,
        mut csv,
    } = files;

    if let Err(error) = write_json(&mut json) {
        drop(json);
        drop(csv);
        cleanup_report_files(&paths);
        return Err(format!(
            "failed to write JSON report to {}: {error}",
            paths.json.display()
        ));
    }
    if let Err(error) = write_csv(&mut csv) {
        drop(json);
        drop(csv);
        cleanup_report_files(&paths);
        return Err(format!(
            "failed to write CSV report to {}: {error}",
            paths.csv.display()
        ));
    }

    Ok(paths)
}

fn cleanup_report_files(paths: &SavedReportPaths) {
    remove_report_file(&paths.json);
    remove_report_file(&paths.csv);
}

fn remove_report_file(path: &std::path::Path) {
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => eprintln!(
            "failed to remove partial report {}: {error}",
            path.display()
        ),
    }
}

fn write_passes_csv(output: &mut impl io::Write, passes: &[BenchmarkPassReport]) -> io::Result<()> {
    writeln!(output, "phase,pass_number,sample_index,mb_per_second")?;
    for pass in passes {
        for (sample_index, mb_per_second) in pass.throughput_samples.iter().enumerate() {
            writeln!(
                output,
                "{},{},{},{}",
                phase_csv(pass.phase),
                pass.pass_number,
                sample_index,
                mb_per_second
            )?;
        }
    }
    Ok(())
}

fn phase_csv(phase: StreamingIoPhase) -> &'static str {
    match phase {
        StreamingIoPhase::Write => "write",
        StreamingIoPhase::Read => "read",
    }
}

fn cache_method_label(cache_mode: CacheMode) -> &'static str {
    match cache_mode {
        CacheMode::Enabled => "normal_file_io",
        CacheMode::Disabled => disabled_cache_method_label(),
    }
}

#[cfg(windows)]
fn disabled_cache_method_label() -> &'static str {
    "write_through"
}

#[cfg(target_os = "macos")]
fn disabled_cache_method_label() -> &'static str {
    "fcntl_no_cache"
}

#[cfg(target_os = "linux")]
fn disabled_cache_method_label() -> &'static str {
    "posix_fadvise_dont_need"
}

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
fn disabled_cache_method_label() -> &'static str {
    "best_effort_unavailable"
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
            .map_err(|error| error.to_string())
            .and_then(|report| {
                if config.save_report {
                    let launch_dir = env::current_dir().map_err(|error| error.to_string())?;
                    save_reports(&launch_dir, &config, None, &report)?;
                }
                Ok(report)
            });
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
    use std::{io::Write as _, sync::atomic::AtomicU64};

    use studiofs_bench::{BenchmarkPassMetrics, StreamingIoPhase};

    static TEST_DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn test_dir(name: &str) -> PathBuf {
        let id = TEST_DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("{name}-{}-{id}", std::process::id()))
    }

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

        let Err(error) = ScriptedOptions::parse(&args) else {
            panic!("continuous execution should be rejected");
        };

        assert_eq!(error, "scripted mode does not support continuous execution");
    }

    #[test]
    fn scripted_options_parse_scripted_settings() {
        let args = [
            "--target",
            "E:/bench-target",
            "--workload-bytes",
            "8",
            "--run-mode",
            "mounted",
            "--mode",
            "write-only",
            "--layout",
            "hundred-files-plus-minus-five",
            "--cache",
            "disabled",
            "--keep-files",
            "--save-report",
        ]
        .map(String::from);

        let options = match ScriptedOptions::parse(&args) {
            Ok(options) => options,
            Err(error) => panic!("scripted options should parse: {error}"),
        };

        assert_eq!(options.config.target_path, PathBuf::from("E:/bench-target"));
        assert_eq!(options.workload_bytes, Some(8));
        assert_eq!(options.config.run_mode, RunMode::MountedFilesystem);
        assert_eq!(options.config.test_mode, DiskTestMode::WriteOnly);
        assert_eq!(
            options.config.file_layout,
            FileLayout::HundredFilesPlusMinusFive
        );
        assert_eq!(options.config.cache_mode, CacheMode::Disabled);
        assert!(options.config.keep_files);
        assert!(options.config.save_report);
    }

    #[test]
    fn report_path_prefix_uses_launch_dir_timestamp_and_counter() {
        let dir = PathBuf::from("reports");
        let first = report_path_prefix(
            &dir,
            std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000),
            0,
        );
        let second = report_path_prefix(
            &dir,
            std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000),
            1,
        );

        assert_eq!(first, dir.join("studiofs-bench-report-1700000000-0"));
        assert_eq!(second, dir.join("studiofs-bench-report-1700000000-1"));
    }

    #[test]
    fn save_reports_rejects_empty_pass_data() {
        let report = stopped_report();
        let config = BenchmarkConfig::for_target(PathBuf::from("."));
        let dir = test_dir("studiofs-bench-sfs-577-empty");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let error = save_reports(&dir, &config, None, &report).unwrap_err();

        assert_eq!(error, "no completed pass data; report was not saved");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_reports_writes_json_context_and_csv_samples() {
        let dir = test_dir("studiofs-bench-sfs-577-report");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut config = BenchmarkConfig::for_target(PathBuf::from("E:/bench-target"));
        config.test_mode = DiskTestMode::WriteOnly;
        let report = BenchmarkRunnerReport {
            run_dir: "E:/bench-target/studiofs-bench-run".into(),
            files_kept: false,
            cleanup_error: None,
            passes: vec![BenchmarkPassReport {
                phase: StreamingIoPhase::Write,
                pass_number: 1,
                bytes_processed: 8,
                stopped: false,
                metrics: BenchmarkPassMetrics::default(),
                throughput_samples: vec![10.0, 20.0],
            }],
            stopped: false,
        };

        let paths = save_reports(&dir, &config, Some(8), &report).unwrap();

        let json = std::fs::read_to_string(paths.json).unwrap();
        let csv = std::fs::read_to_string(paths.csv).unwrap();
        assert!(json.contains("\"platform\""));
        assert!(json.contains("\"cache_method\""));
        assert!(json.contains("\"file_layout\""));
        assert!(csv.contains("phase,pass_number,sample_index,mb_per_second"));
        assert!(csv.contains("write,1,0,10"));
        assert!(csv.contains("write,1,1,20"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn write_report_files_removes_json_when_csv_write_fails() {
        let dir = test_dir("studiofs-bench-sfs-577-partial");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let json_path = dir.join("report.json");
        let csv_path = dir.join("report.csv");
        let files = OpenedReportFiles {
            paths: SavedReportPaths {
                json: json_path.clone(),
                csv: csv_path.clone(),
            },
            json: File::create(&json_path).unwrap(),
            csv: File::create(&csv_path).unwrap(),
        };

        let error = write_report_files(
            files,
            |json| json.write_all(b"{\"ok\":true}"),
            |_csv| Err(io::Error::other("csv failed")),
        )
        .unwrap_err();

        assert!(
            error.contains("failed to write CSV report"),
            "unexpected error: {error}"
        );
        assert!(!json_path.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn scripted_options_reject_missing_argument_value() {
        let args = ["--target"].map(String::from);

        let Err(error) = ScriptedOptions::parse(&args) else {
            panic!("missing argument value should be rejected");
        };

        assert_eq!(error, "missing argument value");
    }

    #[test]
    fn scripted_options_reject_invalid_number() {
        let args = ["--workload-bytes", "nope"].map(String::from);

        let Err(error) = ScriptedOptions::parse(&args) else {
            panic!("invalid number should be rejected");
        };

        assert_eq!(error, "invalid unsigned integer: nope");
    }

    fn assert_render_contains(ui: &TerminalUi, expected: &str) {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(96, 24)).unwrap();

        terminal.draw(|frame| ui.render(frame)).unwrap();

        assert!(terminal.backend().to_string().contains(expected));
    }
}
