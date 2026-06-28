//! Terminal UI entrypoint for studiofs-bench.

use std::{
    env,
    fs::File,
    io::{self, IsTerminal, Write},
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
    BenchmarkConfig, BenchmarkPassReport, BenchmarkRunner, BenchmarkRunnerError,
    BenchmarkRunnerReport, CacheMode, DiskTestMode, ExecutionMode, FileLayout, RunMode,
    StreamingIoPhase, StreamingIoSample, TerminalUi, UiAction, Workload, WorkloadSize,
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
        match runner.run_workload(workload, &options.config, |_| {}, || false) {
            Ok(report) => report,
            Err(error) => return finish_scripted_error(&error, &options),
        }
    } else {
        match runner.run(&options.config, |_| {}, || false) {
            Ok(report) => report,
            Err(error) => return finish_scripted_error(&error, &options),
        }
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

fn finish_scripted_error(
    error: &BenchmarkRunnerError,
    options: &ScriptedOptions,
) -> Result<(), String> {
    if let BenchmarkRunnerError::RunFailed { partial_report, .. } = error
        && options.config.save_report
    {
        match env::current_dir() {
            Ok(launch_dir) => {
                match save_reports(
                    &launch_dir,
                    &options.config,
                    options.workload_bytes,
                    partial_report,
                ) {
                    Ok(paths) => {
                        println!(
                            "Reports saved - {}, {}",
                            paths.json.display(),
                            paths.csv.display()
                        );
                    }
                    Err(save_error) => {
                        eprintln!("Warning - failed to save partial report: {save_error}");
                    }
                }
            }
            Err(current_dir_error) => {
                eprintln!("Warning - failed to read launch directory: {current_dir_error}");
            }
        }
    }

    Err(error.to_string())
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
                "--no-batch-fsync" => config.batch_fsync = false,
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
        |json_file| {
            let mut writer = io::BufWriter::new(json_file);
            serde_json::to_writer_pretty(&mut writer, &payload).map_err(io::Error::other)?;
            writer.flush()
        },
        |csv_file| {
            let mut writer = io::BufWriter::new(csv_file);
            write_passes_csv(&mut writer, &report.passes)?;
            writer.flush()
        },
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
    let _ = std::fs::remove_file(path);
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
    done: Receiver<Result<BenchmarkRunnerReport, BenchmarkRunError>>,
}

#[derive(Debug)]
struct BenchmarkRunError {
    message: String,
    passes: Vec<BenchmarkPassReport>,
    cleanup_error: Option<String>,
}

impl BenchmarkRunError {
    fn from_runner(error: BenchmarkRunnerError) -> Self {
        let message = error.to_string();
        match error {
            BenchmarkRunnerError::RunFailed { partial_report, .. } => {
                let BenchmarkRunnerReport {
                    passes,
                    cleanup_error,
                    ..
                } = *partial_report;
                Self {
                    message,
                    passes,
                    cleanup_error,
                }
            }
            _ => Self {
                message,
                passes: Vec::new(),
                cleanup_error: None,
            },
        }
    }

    fn with_report(message: String, report: BenchmarkRunnerReport) -> Self {
        Self {
            message,
            passes: report.passes,
            cleanup_error: report.cleanup_error,
        }
    }

    #[cfg(test)]
    fn without_report(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            passes: Vec::new(),
            cleanup_error: None,
        }
    }
}

fn append_cleanup_error(mut message: String, cleanup_error: Option<&str>) -> String {
    if let Some(cleanup_error) = cleanup_error {
        message.insert_str(0, "; ");
        message.insert_str(0, cleanup_error);
        message.insert_str(0, "Cleanup Failed: ");
    }
    message
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
            .map_err(BenchmarkRunError::from_runner)
            .and_then(|report| {
                if !config.save_report {
                    return Ok(report);
                }
                let launch_dir = match env::current_dir() {
                    Ok(launch_dir) => launch_dir,
                    Err(error) => {
                        return Err(BenchmarkRunError::with_report(error.to_string(), report));
                    }
                };
                if let Err(error) = save_reports(&launch_dir, &config, None, &report) {
                    return Err(BenchmarkRunError::with_report(error, report));
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
                    let message = append_cleanup_error(message, report.cleanup_error.as_deref());
                    ui.finish_run_with_passes(message, report.passes);
                }
                Err(error) if error.passes.is_empty() => {
                    let message = append_cleanup_error(
                        format!("Error - {}", error.message),
                        error.cleanup_error.as_deref(),
                    );
                    ui.finish_run(message);
                }
                Err(error) => {
                    let message = append_cleanup_error(
                        format!("Error - {}", error.message),
                        error.cleanup_error.as_deref(),
                    );
                    ui.finish_run_with_passes(message, error.passes);
                }
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
#[path = "main_tests.rs"]
mod tests;
