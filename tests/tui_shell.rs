//! Terminal UI shell tests.

use std::path::PathBuf;

use ratatui::{Terminal, backend::TestBackend};
use studiofs_bench::{CacheMode, DiskTestMode, ExecutionMode, FileLayout, TerminalUi, UiAction};

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
    assert!(output.contains("Run mode"));
    assert!(output.contains("Keep files"));
    assert!(output.contains("Save report"));
}

#[test]
fn terminal_ui_edits_selected_settings_from_keyboard_actions() {
    let mut ui = TerminalUi::default();

    ui.handle_action(UiAction::InsertText(
        PathBuf::from("/tmp/bench").display().to_string(),
    ));
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

    for value in ["/", "t", "m", "p"] {
        ui.handle_action(UiAction::InsertText(value.to_owned()));
    }

    assert_eq!(ui.config().target_path, PathBuf::from("/tmp"));
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
