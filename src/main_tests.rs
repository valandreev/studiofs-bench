use super::*;
use std::sync::atomic::AtomicU64;

use studiofs_bench::{BenchmarkPassMetrics, ConfigError, StreamingIoPhase};

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

fn one_pass_report(cleanup_error: Option<String>) -> BenchmarkRunnerReport {
    BenchmarkRunnerReport {
        run_dir: ".".into(),
        files_kept: false,
        cleanup_error,
        passes: vec![BenchmarkPassReport {
            phase: StreamingIoPhase::Write,
            pass_number: 1,
            bytes_processed: 1,
            stopped: false,
            metrics: BenchmarkPassMetrics {
                sample_count: 1,
                average_mb_per_second: 123.0,
                stable_mb_per_second: 123.0,
                minimum_mb_per_second: 123.0,
                drop_count: 0,
            },
            throughput_samples: Vec::new(),
        }],
        stopped: false,
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
    done_tx
        .send(Err(BenchmarkRunError::without_report("failed")))
        .unwrap();
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
fn finish_completed_run_uses_partial_report_from_run_failed_error() {
    let mut ui = TerminalUi::default();
    ui.handle_action(UiAction::Submit);
    let stop = Arc::new(AtomicBool::new(false));
    let (_sample_tx, samples) = mpsc::channel();
    let (done_tx, done) = mpsc::channel();
    let report = one_pass_report(None);
    done_tx
        .send(Err(BenchmarkRunError::from_runner(
            BenchmarkRunnerError::RunFailed {
                source: Box::new(BenchmarkRunnerError::Config(ConfigError::ZeroWorkload)),
                partial_report: Box::new(report),
            },
        )))
        .unwrap();
    let mut running = Some(RunningBenchmark {
        stop,
        samples,
        done,
    });

    let changed = finish_completed_run(&mut running, &mut ui);

    assert!(changed);
    assert_render_contains(&ui, "write pass 1: Avg 123.0");
    assert_render_contains(&ui, "Error - benchmark failed after 1 completed passes");
}

#[test]
fn finish_completed_run_shows_cleanup_error_from_run_failed_report() {
    let mut ui = TerminalUi::default();
    ui.handle_action(UiAction::Submit);
    let stop = Arc::new(AtomicBool::new(false));
    let (_sample_tx, samples) = mpsc::channel();
    let (done_tx, done) = mpsc::channel();
    let report = one_pass_report(Some(String::from("cleanup failed")));
    done_tx
        .send(Err(BenchmarkRunError::from_runner(
            BenchmarkRunnerError::RunFailed {
                source: Box::new(BenchmarkRunnerError::Config(ConfigError::ZeroWorkload)),
                partial_report: Box::new(report),
            },
        )))
        .unwrap();
    let mut running = Some(RunningBenchmark {
        stop,
        samples,
        done,
    });

    let changed = finish_completed_run(&mut running, &mut ui);

    assert!(changed);
    assert_render_contains(&ui, "Cleanup Failed");
}

#[test]
fn finish_scripted_error_keeps_primary_error_when_partial_report_save_fails() {
    let mut options = ScriptedOptions {
        config: BenchmarkConfig::for_target(PathBuf::from(".")),
        workload_bytes: None,
    };
    options.config.save_report = true;
    let error = BenchmarkRunnerError::RunFailed {
        source: Box::new(BenchmarkRunnerError::Config(ConfigError::ZeroWorkload)),
        partial_report: Box::new(stopped_report()),
    };

    let result = finish_scripted_error(&error, &options).unwrap_err();

    assert_eq!(result, error.to_string());
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
        "--no-batch-fsync",
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
    assert!(!options.config.batch_fsync);
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
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(96, 24)).unwrap();

    terminal.draw(|frame| ui.render(frame)).unwrap();

    assert!(terminal.backend().to_string().contains(expected));
}
