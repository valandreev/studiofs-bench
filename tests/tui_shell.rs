//! Terminal UI shell tests.

use std::path::PathBuf;

use ratatui::{Terminal, backend::TestBackend};
use studiofs_bench::{
    BenchmarkPassMetrics, BenchmarkPassReport, CacheMode, DiskTestMode, ExecutionMode, FileLayout,
    StreamingIoPhase, StreamingIoSample, TerminalUi, UiAction,
};

#[test]
fn terminal_ui_shows_editable_benchmark_settings() {
    let mut terminal = Terminal::new(TestBackend::new(96, 24)).unwrap();
    let ui = TerminalUi::default();

    terminal.draw(|frame| ui.render(frame)).unwrap();
    let output = terminal.backend().to_string();

    assert!(output.contains("Target path"));
    assert!(output.contains("Workload size"));
    assert!(output.contains("Mode"));
    assert!(output.contains("Layout"));
    assert!(output.contains("Cache mode"));
    assert!(output.contains("Execution mode"));
    assert!(output.contains("Keep files"));
    assert!(output.contains("Save report"));
}

#[test]
fn terminal_ui_edits_selected_settings_from_keyboard_actions() {
    let mut ui = TerminalUi::default();

    ui.handle_action(UiAction::Backspace);
    for value in "/tmp/bench".chars() {
        ui.handle_action(UiAction::InsertText(value));
    }
    for _ in 0..7 {
        ui.handle_action(UiAction::MoveDown);
        ui.handle_action(UiAction::NextValue);
    }

    let config = ui.config();

    assert_eq!(config.target_path, PathBuf::from("/tmp/bench"));
    assert_eq!(config.test_mode, DiskTestMode::WriteOnly);
    assert_eq!(config.file_layout, FileLayout::HundredFilesPlusMinusFive);
    assert_eq!(config.cache_mode, CacheMode::Disabled);
    assert_eq!(config.execution_mode, ExecutionMode::Continuous);
    assert!(config.keep_files);
    assert!(!config.save_report);
}

#[test]
fn terminal_ui_appends_target_path_characters_as_text() {
    let mut ui = TerminalUi::default();

    ui.handle_action(UiAction::Backspace);
    for value in "/tmp".chars() {
        ui.handle_action(UiAction::InsertText(value));
    }

    assert_eq!(ui.config().target_path, PathBuf::from("/tmp"));
}

#[test]
fn terminal_ui_allows_relative_paths_starting_with_dot() {
    let mut ui = TerminalUi::default();

    for value in "/bench".chars() {
        ui.handle_action(UiAction::InsertText(value));
    }

    assert_eq!(ui.config().target_path, PathBuf::from("./bench"));
}

#[test]
fn terminal_ui_ignores_setting_edits_while_running() {
    let mut ui = TerminalUi::default();
    let before = ui.config().clone();

    ui.handle_action(UiAction::Submit);
    ui.handle_action(UiAction::MoveDown);
    ui.handle_action(UiAction::NextValue);
    ui.handle_action(UiAction::InsertText('x'));
    ui.handle_action(UiAction::Backspace);

    assert_eq!(ui.config(), &before);
}

#[test]
fn terminal_ui_defaults_to_current_directory_target() {
    let ui = TerminalUi::default();

    assert_eq!(ui.config().target_path, PathBuf::from("."));
}

#[test]
fn terminal_ui_enter_and_escape_start_stop_and_exit() {
    let mut ui = TerminalUi::default();

    ui.handle_action(UiAction::Submit);
    assert!(ui.is_running());

    ui.handle_action(UiAction::Submit);
    assert!(!ui.is_running());
    assert!(!ui.should_exit());

    ui.handle_action(UiAction::Submit);
    assert!(ui.is_running());

    ui.handle_action(UiAction::Cancel);
    assert!(!ui.is_running());
    assert!(!ui.should_exit());

    ui.handle_action(UiAction::Cancel);
    assert!(ui.should_exit());
}

#[test]
fn terminal_ui_finish_run_returns_to_idle() {
    let mut ui = TerminalUi::default();

    ui.handle_action(UiAction::Submit);
    ui.finish_run("Done");

    assert!(!ui.is_running());
}

#[test]
fn terminal_ui_renders_live_progress_and_pass_summary_metrics() {
    let mut terminal = Terminal::new(TestBackend::new(96, 24)).unwrap();
    let mut ui = TerminalUi::default();
    ui.handle_action(UiAction::Submit);
    ui.observe_sample(StreamingIoSample {
        phase: StreamingIoPhase::Write,
        pass_number: 1,
        timestamp: std::time::SystemTime::UNIX_EPOCH,
        offset: 0,
        bytes_processed: 500_000_000,
        mb_per_second: 125.0,
    });
    ui.finish_run_with_passes(
        "Done",
        vec![BenchmarkPassReport {
            phase: StreamingIoPhase::Write,
            pass_number: 1,
            bytes_processed: 1_000_000_000,
            stopped: false,
            metrics: BenchmarkPassMetrics {
                sample_count: 2,
                average_mb_per_second: 110.0,
                stable_mb_per_second: 120.0,
                minimum_mb_per_second: 90.0,
                drop_count: 1,
            },
            throughput_samples: Vec::new(),
        }],
    );

    terminal.draw(|frame| ui.render(frame)).unwrap();
    let output = terminal.backend().to_string();

    assert!(output.contains("Current write"));
    assert!(output.contains("125.0 MB/s"));
    assert!(output.contains("Avg 110.0"));
    assert!(output.contains("Stable 120.0"));
    assert!(output.contains("Min 90.0"));
    assert!(output.contains("Drops 1"));
}

#[test]
fn terminal_ui_renders_completed_pass_chart_and_stability_strip() {
    let mut terminal = Terminal::new(TestBackend::new(96, 24)).unwrap();
    let mut ui = TerminalUi::default();
    ui.handle_action(UiAction::Submit);
    for mb_per_second in [120.0, 90.0, 50.0, 8.0] {
        ui.observe_sample(StreamingIoSample {
            phase: StreamingIoPhase::Write,
            pass_number: 1,
            timestamp: std::time::SystemTime::UNIX_EPOCH,
            offset: 0,
            bytes_processed: 1_000_000,
            mb_per_second,
        });
    }
    ui.observe_sample(StreamingIoSample {
        phase: StreamingIoPhase::Read,
        pass_number: 1,
        timestamp: std::time::SystemTime::UNIX_EPOCH,
        offset: 0,
        bytes_processed: 1_000_000,
        mb_per_second: 90.0,
    });

    terminal.draw(|frame| ui.render(frame)).unwrap();
    let output = terminal.backend().to_string();

    assert!(output.contains("Chart MB/s"));
    assert!(output.contains("120.0 |"));
    assert!(output.contains("Stability"));
    assert!(output.contains(".-!x"));
}

#[test]
fn terminal_ui_renders_completed_pass_chart_progress_axis() {
    let mut terminal = Terminal::new(TestBackend::new(96, 24)).unwrap();
    let mut ui = TerminalUi::default();
    ui.finish_run_with_passes(
        "Done",
        vec![pass_report_with_samples(
            StreamingIoPhase::Write,
            1,
            vec![100.0, 90.0, 80.0, 40.0],
        )],
    );

    terminal.draw(|frame| ui.render(frame)).unwrap();
    let output = terminal.backend().to_string();

    assert!(output.contains("Progress 0%"));
    assert!(output.contains("100%"));
}

#[test]
fn terminal_ui_scales_completed_pass_chart_from_rendered_points() {
    let mut terminal = Terminal::new(TestBackend::new(96, 24)).unwrap();
    let mut ui = TerminalUi::default();
    let mut samples = vec![100.0; 33];
    samples[31] = 200.0;
    ui.finish_run_with_passes(
        "Done",
        vec![pass_report_with_samples(
            StreamingIoPhase::Write,
            1,
            samples,
        )],
    );

    terminal.draw(|frame| ui.render(frame)).unwrap();
    let output = terminal.backend().to_string();

    assert!(output.contains("100.0 |"));
    assert!(!output.contains("200.0 |"));
}

#[test]
fn terminal_ui_replaces_read_chart_with_latest_continuous_read_pass() {
    let mut terminal = Terminal::new(TestBackend::new(96, 24)).unwrap();
    let mut ui = TerminalUi::default();
    ui.handle_action(UiAction::MoveDown);
    ui.handle_action(UiAction::MoveDown);
    ui.handle_action(UiAction::NextValue);
    ui.handle_action(UiAction::NextValue);
    ui.handle_action(UiAction::MoveDown);
    ui.handle_action(UiAction::MoveDown);
    ui.handle_action(UiAction::MoveDown);
    ui.handle_action(UiAction::NextValue);
    ui.finish_run_with_passes(
        "Done",
        vec![
            pass_report(StreamingIoPhase::Read, 1, 10.0),
            pass_report(StreamingIoPhase::Read, 2, 90.0),
        ],
    );

    terminal.draw(|frame| ui.render(frame)).unwrap();
    let output = terminal.backend().to_string();

    assert!(!output.contains("read pass 1"));
    assert!(output.contains("read pass 2"));
}

#[test]
fn terminal_ui_keeps_read_pass_summary_in_default_read_write_mode() {
    let mut terminal = Terminal::new(TestBackend::new(96, 24)).unwrap();
    let mut ui = TerminalUi::default();
    ui.finish_run_with_passes(
        "Done",
        vec![
            pass_report(StreamingIoPhase::Write, 1, 120.0),
            pass_report(StreamingIoPhase::Read, 1, 90.0),
        ],
    );

    terminal.draw(|frame| ui.render(frame)).unwrap();
    let output = terminal.backend().to_string();

    assert!(output.contains("read pass 1"));
}

#[test]
fn terminal_ui_archives_completed_live_pass_when_next_pass_starts() {
    let mut terminal = Terminal::new(TestBackend::new(96, 24)).unwrap();
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

    terminal.draw(|frame| ui.render(frame)).unwrap();
    let output = terminal.backend().to_string();

    assert!(output.contains("write pass 1: Avg 100.0"));
    assert!(output.contains("Current read: 80.0 MB/s"));
}

#[test]
fn terminal_ui_renders_progress_megabytes_with_fractional_precision() {
    let mut terminal = Terminal::new(TestBackend::new(96, 24)).unwrap();
    let mut ui = TerminalUi::default();
    ui.handle_action(UiAction::Submit);
    ui.observe_sample(StreamingIoSample {
        phase: StreamingIoPhase::Write,
        pass_number: 1,
        timestamp: std::time::SystemTime::UNIX_EPOCH,
        offset: 0,
        bytes_processed: 500_000,
        mb_per_second: 125.0,
    });

    terminal.draw(|frame| ui.render(frame)).unwrap();
    let output = terminal.backend().to_string();

    assert!(output.contains("0.5 /"));
}

#[test]
fn terminal_ui_keeps_progress_gauge_visible_with_live_text() {
    let mut terminal = Terminal::new(TestBackend::new(96, 24)).unwrap();
    let mut ui = TerminalUi::default();
    ui.handle_action(UiAction::Submit);
    ui.observe_sample(StreamingIoSample {
        phase: StreamingIoPhase::Write,
        pass_number: 1,
        timestamp: std::time::SystemTime::UNIX_EPOCH,
        offset: 0,
        bytes_processed: 2_000_000_000,
        mb_per_second: 125.0,
    });

    terminal.draw(|frame| ui.render(frame)).unwrap();
    let output = terminal.backend().to_string();

    assert!(output.contains("█"));
    assert!(output.contains("Current write: 125.0 MB/s"));
}

fn pass_report(
    phase: StreamingIoPhase,
    pass_number: u64,
    mb_per_second: f64,
) -> BenchmarkPassReport {
    pass_report_with_samples(phase, pass_number, vec![mb_per_second])
}

fn pass_report_with_samples(
    phase: StreamingIoPhase,
    pass_number: u64,
    throughput_samples: Vec<f64>,
) -> BenchmarkPassReport {
    let mb_per_second = throughput_samples[0];
    BenchmarkPassReport {
        phase,
        pass_number,
        bytes_processed: 1_000_000,
        stopped: false,
        metrics: BenchmarkPassMetrics {
            sample_count: 1,
            average_mb_per_second: mb_per_second,
            stable_mb_per_second: mb_per_second,
            minimum_mb_per_second: mb_per_second,
            drop_count: 0,
        },
        throughput_samples,
    }
}
