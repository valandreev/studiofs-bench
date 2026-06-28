use std::borrow::Cow;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

use ratatui::{
    Frame,
    layout::{Constraint, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Gauge, List, ListItem, Paragraph},
};
use serde::Serialize;
use thiserror::Error;

use crate::runner_streaming::{MetricsAccumulator, chunk_len, fill_benchmark_buffer};
use crate::{BenchmarkPassReport, StreamingIoPhase, StreamingIoSample};

pub(crate) const DECIMAL_MB: u64 = 1_000_000;
const MB_PER_GB: u64 = 1_000;
pub(crate) const DEFAULT_STREAMING_BLOCK_BYTES: usize = 8 * 1024 * 1024;
const MAX_FIXED_LAYOUT_FILES: usize = 100_000;
pub(crate) const STAMP_INTERVAL_BYTES: usize = 4 * 1024;
static RUN_DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Keyboard action understood by the terminal UI shell.
#[derive(Debug, Copy, Clone)]
pub enum UiAction {
    /// Move selection to the previous setting.
    MoveUp,
    /// Move selection to the next setting.
    MoveDown,
    /// Select the previous value for the current setting.
    PreviousValue,
    /// Select the next value for the current setting.
    NextValue,
    /// Append a character to the target path field.
    InsertText(char),
    /// Remove one character from the target path field.
    Backspace,
    /// Start or stop the benchmark.
    Submit,
    /// Stop a running benchmark or exit when idle.
    Cancel,
}

/// Full-screen terminal UI state for configuring a benchmark run.
#[derive(Debug)]
pub struct TerminalUi {
    config: BenchmarkConfig,
    selected: usize,
    running: bool,
    exit: bool,
    message: String,
    progress: Option<LivePassProgress>,
    pass_summaries: Vec<BenchmarkPassReport>,
}

impl Default for TerminalUi {
    fn default() -> Self {
        Self {
            config: BenchmarkConfig::for_target(PathBuf::from(".")),
            selected: 0,
            running: false,
            exit: false,
            message: String::from("Idle - Enter starts, Esc exits"),
            progress: None,
            pass_summaries: Vec::new(),
        }
    }
}

impl TerminalUi {
    /// Returns the currently selected benchmark config.
    #[must_use]
    pub fn config(&self) -> &BenchmarkConfig {
        &self.config
    }

    /// Returns whether the UI has a running benchmark.
    #[must_use]
    pub fn is_running(&self) -> bool {
        self.running
    }

    /// Returns whether the event loop should exit.
    #[must_use]
    pub fn should_exit(&self) -> bool {
        self.exit
    }

    /// Applies one keyboard action to the UI state.
    pub fn handle_action(&mut self, action: UiAction) {
        if self.running && !matches!(action, UiAction::Submit | UiAction::Cancel) {
            return;
        }

        match action {
            UiAction::MoveUp => {
                self.selected = self.selected.saturating_sub(1);
            }
            UiAction::MoveDown => {
                self.selected = (self.selected + 1).min(SETTING_COUNT - 1);
            }
            UiAction::PreviousValue => self.change_selected(false),
            UiAction::NextValue => self.change_selected(true),
            UiAction::InsertText(value) if self.selected == TARGET_SETTING => {
                let mut path = self.config.target_path.display().to_string();
                path.push(value);
                self.config.target_path = PathBuf::from(path);
            }
            UiAction::Backspace if self.selected == TARGET_SETTING => {
                let mut path = self.config.target_path.display().to_string();
                path.pop();
                self.config.target_path = PathBuf::from(path);
            }
            UiAction::Submit => {
                self.running = !self.running;
                self.message = if self.running {
                    self.progress = None;
                    self.pass_summaries.clear();
                    String::from("Running - Enter/Esc stops")
                } else {
                    String::from("Stopping")
                };
            }
            UiAction::Cancel if self.running => {
                self.running = false;
                self.message = String::from("Stopping");
            }
            UiAction::Cancel => {
                self.exit = true;
            }
            UiAction::InsertText(_) | UiAction::Backspace => {}
        }
    }

    /// Marks the current benchmark run as finished and displays its result.
    pub fn finish_run(&mut self, message: impl Into<String>) {
        self.running = false;
        self.message = message.into();
    }

    /// Records one live progress sample for the active pass.
    pub fn observe_sample(&mut self, sample: StreamingIoSample) {
        let total_bytes = self
            .config
            .workload_size
            .bytes()
            .unwrap_or(sample.bytes_processed)
            .max(sample.bytes_processed);
        let is_new_pass = self.progress.as_ref().is_none_or(|progress| {
            progress.phase != sample.phase || progress.pass_number != sample.pass_number
        });
        if is_new_pass {
            if let Some(progress) = self.progress.take() {
                self.pass_summaries.push(BenchmarkPassReport {
                    phase: progress.phase,
                    pass_number: progress.pass_number,
                    bytes_processed: progress.bytes_processed,
                    stopped: false,
                    metrics: progress.metrics.finish(),
                    throughput_samples: progress.throughput_samples,
                });
            }
            self.progress = Some(LivePassProgress {
                phase: sample.phase,
                pass_number: sample.pass_number,
                bytes_processed: 0,
                total_bytes,
                current_mb_per_second: 0.0,
                metrics: MetricsAccumulator::default(),
                throughput_samples: Vec::new(),
            });
        }

        if let Some(progress) = &mut self.progress {
            progress.total_bytes = progress.total_bytes.max(total_bytes);
            progress.bytes_processed = sample.bytes_processed;
            progress.current_mb_per_second = sample.mb_per_second;
            progress.metrics.add(sample.mb_per_second);
            progress.throughput_samples.push(sample.mb_per_second);
        }
    }

    /// Marks the run as finished and displays completed pass summaries.
    pub fn finish_run_with_passes(
        &mut self,
        message: impl Into<String>,
        passes: Vec<BenchmarkPassReport>,
    ) {
        self.finish_run(message);
        self.pass_summaries = passes;
    }

    /// Renders the full-screen terminal UI.
    pub fn render(&self, frame: &mut Frame<'_>) {
        let [header, settings, footer] = Layout::vertical([
            Constraint::Length(3),
            Constraint::Min(10),
            Constraint::Length(3),
        ])
        .areas(frame.area());

        let title = Paragraph::new("studiofs-bench")
            .block(Block::new().borders(Borders::BOTTOM))
            .style(Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD));
        frame.render_widget(title, header);

        let [settings, metrics] =
            Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
                .areas(settings);

        let rows = self
            .setting_rows()
            .into_iter()
            .enumerate()
            .map(|(index, (name, value))| {
                let marker = if index == self.selected { "> " } else { "  " };
                ListItem::new(Line::from(vec![
                    Span::raw(marker),
                    Span::styled(name, Style::new().add_modifier(Modifier::BOLD)),
                    Span::styled(": ", Style::new().add_modifier(Modifier::BOLD)),
                    Span::raw(value),
                ]))
            })
            .collect::<Vec<_>>();
        frame.render_widget(
            List::new(rows).block(Block::new().title("Settings").borders(Borders::ALL)),
            settings,
        );

        frame.render_widget(
            Paragraph::new(self.message.as_str()).block(Block::new().borders(Borders::TOP)),
            footer,
        );
        self.render_metrics(frame, metrics);
    }

    #[expect(
        clippy::cast_precision_loss,
        reason = "terminal progress and sizes are approximate human-facing values"
    )]
    fn render_metrics(&self, frame: &mut Frame<'_>, area: ratatui::layout::Rect) {
        let [live, summary] =
            Layout::vertical([Constraint::Length(7), Constraint::Min(4)]).areas(area);

        if let Some(progress) = &self.progress {
            let ratio = if progress.total_bytes == 0 {
                0.0
            } else {
                progress.bytes_processed as f64 / progress.total_bytes as f64
            }
            .clamp(0.0, 1.0);
            let block = Block::new().title("Progress").borders(Borders::ALL);
            let inner = block.inner(live);
            frame.render_widget(block, live);
            let [gauge_area, text_area] =
                Layout::vertical([Constraint::Length(1), Constraint::Min(4)]).areas(inner);
            frame.render_widget(
                Gauge::default()
                    .gauge_style(Style::new().fg(Color::Green))
                    .ratio(ratio),
                gauge_area,
            );
            let metrics = progress.metrics.finish();
            let text = vec![
                Line::from(format!(
                    "Current {}: {:.1} MB/s",
                    phase_label(progress.phase),
                    progress.current_mb_per_second
                )),
                Line::from(format!(
                    "Pass {} - {:.1} / {:.1} MB",
                    progress.pass_number,
                    progress.bytes_processed as f64 / DECIMAL_MB as f64,
                    progress.total_bytes as f64 / DECIMAL_MB as f64
                )),
                Line::from(format!("Avg {:.1}", metrics.average_mb_per_second)),
                Line::from(format!("Stable {:.1}", metrics.stable_mb_per_second)),
            ];
            frame.render_widget(Paragraph::new(text), text_area);
        } else {
            frame.render_widget(
                Paragraph::new("No samples yet")
                    .block(Block::new().title("Progress").borders(Borders::ALL)),
                live,
            );
        }

        let latest_read_pass = if self.config.test_mode == DiskTestMode::WriteOnceReadLoop
            && self.config.execution_mode == ExecutionMode::Continuous
        {
            self.pass_summaries
                .iter()
                .filter(|pass| pass.phase == StreamingIoPhase::Read)
                .map(|pass| pass.pass_number)
                .max()
        } else {
            None
        };
        let rows = self
            .pass_summaries
            .iter()
            .filter(move |pass| {
                pass.phase != StreamingIoPhase::Read || Some(pass.pass_number) == latest_read_pass
            })
            .flat_map(|pass| {
                let metrics = pass.metrics;
                let mut rows = vec![
                    ListItem::new(Line::from(format!(
                        "{} pass {}: Avg {:.1}",
                        phase_label(pass.phase),
                        pass.pass_number,
                        metrics.average_mb_per_second
                    ))),
                    ListItem::new(Line::from(format!(
                        "Stable {:.1}  Min {:.1}  Drops {}",
                        metrics.stable_mb_per_second,
                        metrics.minimum_mb_per_second,
                        metrics.drop_count
                    ))),
                ];
                rows.extend(pass_chart_rows(pass, summary.width.saturating_sub(4)));
                rows
            });
        frame.render_widget(
            List::new(rows).block(Block::new().title("Pass summaries").borders(Borders::ALL)),
            summary,
        );
    }

    fn setting_rows(&self) -> [(&'static str, Cow<'static, str>); SETTING_COUNT] {
        [
            (
                "Target path",
                Cow::Owned(self.config.target_path.display().to_string()),
            ),
            (
                "Workload size",
                Cow::Borrowed(workload_size_label(self.config.workload_size)),
            ),
            (
                "Mode",
                Cow::Borrowed(test_mode_label(self.config.test_mode)),
            ),
            (
                "Layout",
                Cow::Borrowed(file_layout_label(self.config.file_layout)),
            ),
            (
                "Cache mode",
                Cow::Borrowed(cache_mode_label(self.config.cache_mode)),
            ),
            (
                "Execution mode",
                Cow::Borrowed(execution_mode_label(self.config.execution_mode)),
            ),
            (
                "Keep files",
                Cow::Borrowed(bool_label(self.config.keep_files)),
            ),
            (
                "Save report",
                Cow::Borrowed(bool_label(self.config.save_report)),
            ),
        ]
    }

    fn change_selected(&mut self, next: bool) {
        match self.selected {
            WORKLOAD_SETTING => {
                self.config.workload_size = next_workload_size(self.config.workload_size, next);
            }
            MODE_SETTING => self.config.test_mode = next_test_mode(self.config.test_mode, next),
            LAYOUT_SETTING => {
                self.config.file_layout = next_file_layout(self.config.file_layout, next);
            }
            CACHE_SETTING => self.config.cache_mode = next_cache_mode(self.config.cache_mode),
            EXECUTION_MODE_SETTING => {
                self.config.execution_mode = next_execution_mode(self.config.execution_mode);
            }
            KEEP_FILES_SETTING => self.config.keep_files = !self.config.keep_files,
            SAVE_REPORT_SETTING => self.config.save_report = !self.config.save_report,
            _ => {}
        }
    }
}

const TARGET_SETTING: usize = 0;
const WORKLOAD_SETTING: usize = 1;
const MODE_SETTING: usize = 2;
const LAYOUT_SETTING: usize = 3;
const CACHE_SETTING: usize = 4;
const EXECUTION_MODE_SETTING: usize = 5;
const KEEP_FILES_SETTING: usize = 6;
const SAVE_REPORT_SETTING: usize = 7;
const SETTING_COUNT: usize = 8;

#[derive(Debug)]
struct LivePassProgress {
    phase: StreamingIoPhase,
    pass_number: u64,
    bytes_processed: u64,
    total_bytes: u64,
    current_mb_per_second: f64,
    metrics: MetricsAccumulator,
    throughput_samples: Vec<f64>,
}

fn pass_chart_rows(pass: &BenchmarkPassReport, width: u16) -> Vec<ListItem<'static>> {
    if pass.throughput_samples.is_empty() {
        return Vec::new();
    }
    let width = usize::from(width);
    if width < 16 {
        return vec![ListItem::new(Line::from("Chart MB/s: too narrow"))];
    }

    let max = pass
        .throughput_samples
        .iter()
        .copied()
        .fold(0.0_f64, f64::max)
        .max(f64::EPSILON);
    let plot_width = width.saturating_sub(8).min(32);
    let points = chart_points(&pass.throughput_samples, plot_width);
    let top = chart_row(max, max, &points);
    let mid_value = max / 2.0;
    let mid = chart_row(mid_value, mid_value, &points);
    let bottom = chart_row(0.0, 0.0, &points);
    let progress_gap = " ".repeat(points.len().saturating_sub(2));
    let strip = stability_strip(&points, max);

    vec![
        ListItem::new(Line::from("Chart MB/s")),
        ListItem::new(Line::from(top)),
        ListItem::new(Line::from(mid)),
        ListItem::new(Line::from(bottom)),
        ListItem::new(Line::from(format!("Progress 0%{progress_gap}100%"))),
        ListItem::new(Line::from(format!("Stability {strip}"))),
    ]
}

fn chart_points(samples: &[f64], width: usize) -> Vec<f64> {
    if samples.len() <= width {
        return samples.to_vec();
    }
    (0..width)
        .map(|index| {
            let sample_index = index * (samples.len() - 1) / (width - 1);
            samples[sample_index]
        })
        .collect()
}

fn chart_row(label: f64, threshold: f64, samples: &[f64]) -> String {
    let points = samples
        .iter()
        .map(|sample| if *sample >= threshold { '*' } else { ' ' })
        .collect::<String>();
    format!("{label:>5.1} |{points}")
}

fn stability_strip(samples: &[f64], max: f64) -> String {
    samples
        .iter()
        .map(|sample| {
            let ratio = sample / max;
            if ratio >= 0.85 {
                '.'
            } else if ratio >= 0.60 {
                '-'
            } else if ratio >= 0.10 {
                '!'
            } else {
                'x'
            }
        })
        .collect()
}

/// Complete benchmark settings for one configured run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BenchmarkConfig {
    /// Filesystem path where benchmark files are created.
    pub target_path: PathBuf,
    /// Total benchmark data size.
    pub workload_size: WorkloadSize,
    /// Filesystem access mode under test.
    pub run_mode: RunMode,
    /// Disk test mode executed by the runner.
    pub test_mode: DiskTestMode,
    /// Layout used for benchmark files.
    pub file_layout: FileLayout,
    /// Cache behavior expected for the run.
    pub cache_mode: CacheMode,
    /// Whether generated benchmark files remain after the run.
    pub keep_files: bool,
    /// Whether the runner should save a report.
    pub save_report: bool,
    /// Whether the runner executes once or continuously.
    pub execution_mode: ExecutionMode,
}

impl BenchmarkConfig {
    /// User-facing throughput unit.
    pub const THROUGHPUT_UNIT: &'static str = "MB/s";

    /// Creates a config with documented defaults for the required target path.
    #[must_use]
    pub fn for_target(target_path: PathBuf) -> Self {
        Self {
            target_path,
            workload_size: WorkloadSize::Preset(WorkloadPreset::FourGb),
            run_mode: RunMode::LocalFilesystem,
            test_mode: DiskTestMode::ReadWrite,
            file_layout: FileLayout::SingleFile,
            cache_mode: CacheMode::Enabled,
            keep_files: false,
            save_report: true,
            execution_mode: ExecutionMode::RunOnce,
        }
    }

    /// Rejects invalid values and cross-field combinations.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] when the target path is empty, the workload size
    /// is zero or overflows byte conversion, or a fixed file layout is invalid.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.target_path.as_os_str().is_empty() {
            return Err(ConfigError::EmptyTargetPath);
        }

        let Some(workload_mb) = self.workload_size.megabytes() else {
            return Err(ConfigError::WorkloadOverflow);
        };

        if workload_mb == 0 {
            return Err(ConfigError::ZeroWorkload);
        }

        if self.workload_size.bytes().is_none() {
            return Err(ConfigError::WorkloadOverflow);
        }

        let FileLayout::FixedFileSizeMb(file_size_mb) = self.file_layout else {
            return Ok(());
        };

        if file_size_mb == 0 {
            return Err(ConfigError::ZeroFileSize);
        }

        if file_size_mb > workload_mb {
            return Err(ConfigError::FileLayoutExceedsWorkload);
        }

        Ok(())
    }
}

/// Total benchmark workload size.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkloadSize {
    /// One of the supported named workload sizes.
    Preset(WorkloadPreset),
    /// Custom workload size in decimal gigabytes.
    CustomGb(u64),
}

impl WorkloadSize {
    /// Size in decimal gigabytes.
    #[must_use]
    pub fn gigabytes(self) -> u64 {
        match self {
            Self::Preset(preset) => preset.gigabytes(),
            Self::CustomGb(gigabytes) => gigabytes,
        }
    }

    /// Size in decimal megabytes.
    #[must_use]
    pub fn megabytes(self) -> Option<u64> {
        self.gigabytes().checked_mul(MB_PER_GB)
    }

    /// Size in decimal bytes.
    #[must_use]
    pub fn bytes(self) -> Option<u64> {
        self.megabytes()?.checked_mul(DECIMAL_MB)
    }
}

/// Supported named workload sizes.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkloadPreset {
    /// 1 GB workload.
    OneGb,
    /// 4 GB workload.
    FourGb,
    /// 16 GB workload.
    SixteenGb,
    /// 64 GB workload.
    SixtyFourGb,
}

impl WorkloadPreset {
    /// Preset size in decimal gigabytes.
    #[must_use]
    pub fn gigabytes(self) -> u64 {
        match self {
            Self::OneGb => 1,
            Self::FourGb => 4,
            Self::SixteenGb => 16,
            Self::SixtyFourGb => 64,
        }
    }
}

/// Filesystem access mode under test.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunMode {
    /// Benchmark a regular local filesystem path.
    LocalFilesystem,
    /// Benchmark a mounted filesystem path.
    MountedFilesystem,
}

/// Disk test mode executed by the benchmark runner.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiskTestMode {
    /// Write the workload, then read it back.
    ReadWrite,
    /// Write the workload without a read pass.
    WriteOnly,
    /// Write once, then keep repeating read passes until stopped.
    WriteOnceReadLoop,
}

/// File layout used for generated benchmark data.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FileLayout {
    /// Store the workload in one file.
    SingleFile,
    /// Split the workload into 100 files with slight deterministic size variance.
    HundredFilesPlusMinusFive,
    /// Split the workload into files of this decimal MB size.
    ///
    /// The final file may be smaller when the workload is not evenly divisible.
    FixedFileSizeMb(u64),
}

/// Generated benchmark workload inside one temporary run directory.
#[derive(Debug)]
pub struct Workload {
    run_dir: PathBuf,
    files: Vec<WorkloadFile>,
}

impl Workload {
    /// Creates workload files from a complete benchmark config.
    ///
    /// # Errors
    ///
    /// Returns [`WorkloadError`] when config validation or filesystem I/O fails.
    pub fn create(config: &BenchmarkConfig) -> Result<Self, WorkloadError> {
        config.validate()?;
        let total_bytes = config
            .workload_size
            .bytes()
            .ok_or(ConfigError::WorkloadOverflow)?;
        Self::create_for_bytes(&config.target_path, total_bytes, config.file_layout)
    }

    /// Creates workload files with an explicit byte size.
    ///
    /// # Errors
    ///
    /// Returns [`WorkloadError`] when the size/layout combination is invalid or filesystem I/O
    /// fails.
    pub fn create_for_bytes(
        target_path: impl AsRef<Path>,
        total_bytes: u64,
        file_layout: FileLayout,
    ) -> Result<Self, WorkloadError> {
        let target_path = target_path.as_ref();
        let file_sizes = workload_file_sizes(total_bytes, file_layout)?;
        let run_dir = create_unique_run_dir(target_path)?;
        let max_file_size = file_sizes.iter().copied().max().unwrap_or(0);
        let buffer = benchmark_buffer(max_file_size);
        let files = write_workload_files(&run_dir, file_sizes, |path, bytes| {
            write_workload_file(path, bytes, &buffer)
        })?;

        Ok(Self { run_dir, files })
    }

    /// Temporary benchmark run directory containing this workload.
    #[must_use]
    pub fn run_dir(&self) -> &Path {
        &self.run_dir
    }

    /// Files created for this workload.
    #[must_use]
    pub fn files(&self) -> &[WorkloadFile] {
        &self.files
    }

    /// Total bytes across all created workload files.
    #[must_use]
    pub fn total_bytes(&self) -> u64 {
        self.files.iter().map(|file| file.bytes).sum()
    }

    /// Removes only this workload's temporary run directory.
    ///
    /// # Errors
    ///
    /// Returns [`WorkloadError`] when removing the run directory fails.
    pub fn cleanup(self) -> Result<(), WorkloadError> {
        match std::fs::remove_dir_all(self.run_dir) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }
}

/// One generated benchmark workload file.
#[derive(Debug)]
pub struct WorkloadFile {
    /// Workload file path.
    pub path: PathBuf,
    /// Workload file size in bytes.
    pub bytes: u64,
}

fn workload_file_sizes(
    total_bytes: u64,
    file_layout: FileLayout,
) -> Result<Vec<u64>, WorkloadError> {
    if total_bytes == 0 {
        return Err(ConfigError::ZeroWorkload.into());
    }

    match file_layout {
        FileLayout::SingleFile => Ok(vec![total_bytes]),
        FileLayout::HundredFilesPlusMinusFive => hundred_file_sizes(total_bytes),
        FileLayout::FixedFileSizeMb(file_size_mb) => fixed_file_sizes(total_bytes, file_size_mb),
    }
}

fn hundred_file_sizes(total_bytes: u64) -> Result<Vec<u64>, WorkloadError> {
    const FILE_COUNT: usize = 100;
    const WEIGHT_SUM: u64 = 9_995;

    if total_bytes < FILE_COUNT as u64 {
        return Err(WorkloadError::WorkloadTooSmallForLayout);
    }

    let mut sizes = vec![1; FILE_COUNT];
    let weighted_bytes = total_bytes - FILE_COUNT as u64;
    let mut allocated = 0_u64;
    for (index, size_slot) in sizes.iter_mut().enumerate() {
        let weight = 95 + index as u64 % 11;
        let size =
            u64::try_from(u128::from(weighted_bytes) * u128::from(weight) / u128::from(WEIGHT_SUM))
                .map_err(|_| ConfigError::WorkloadOverflow)?;
        allocated += size;
        *size_slot += size;
    }

    let mut remainder = weighted_bytes - allocated;
    for size in &mut sizes {
        if remainder == 0 {
            break;
        }
        *size += 1;
        remainder -= 1;
    }

    Ok(sizes)
}

fn fixed_file_sizes(total_bytes: u64, file_size_mb: u64) -> Result<Vec<u64>, WorkloadError> {
    if file_size_mb == 0 {
        return Err(ConfigError::ZeroFileSize.into());
    }

    let Some(file_bytes) = file_size_mb.checked_mul(DECIMAL_MB) else {
        return Err(ConfigError::WorkloadOverflow.into());
    };
    if file_bytes > total_bytes {
        return Err(ConfigError::FileLayoutExceedsWorkload.into());
    }

    let capacity = usize::try_from(total_bytes.div_ceil(file_bytes))
        .map_err(|_| ConfigError::WorkloadOverflow)?;
    if capacity > MAX_FIXED_LAYOUT_FILES {
        return Err(ConfigError::WorkloadOverflow.into());
    }
    let mut sizes = vec![file_bytes; capacity];
    if let Some(last) = sizes.last_mut() {
        let remainder = total_bytes % file_bytes;
        if remainder != 0 {
            *last = remainder;
        }
    }

    Ok(sizes)
}

fn create_unique_run_dir(target_path: &Path) -> Result<PathBuf, WorkloadError> {
    if target_path.as_os_str().is_empty() {
        return Err(ConfigError::EmptyTargetPath.into());
    }

    std::fs::create_dir_all(target_path)?;
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let counter = RUN_DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = target_path.join(format!(
        "studiofs-bench-run-{}-{nanos}-{counter}",
        std::process::id()
    ));
    std::fs::create_dir(&path)?;
    Ok(path)
}

fn write_workload_files(
    run_dir: &Path,
    file_sizes: Vec<u64>,
    mut write_file: impl FnMut(&Path, u64) -> Result<(), WorkloadError>,
) -> Result<Vec<WorkloadFile>, WorkloadError> {
    let mut files = Vec::with_capacity(file_sizes.len());

    for (index, bytes) in file_sizes.into_iter().enumerate() {
        let path = run_dir.join(format!("studiofs-bench-workload-{index:03}.bin"));
        if let Err(error) = write_file(&path, bytes) {
            let _ = std::fs::remove_dir_all(run_dir);
            return Err(error);
        }
        files.push(WorkloadFile { path, bytes });
    }

    Ok(files)
}

fn benchmark_buffer(total_bytes: u64) -> Vec<u8> {
    let buffer_size = usize::try_from(total_bytes)
        .unwrap_or(DEFAULT_STREAMING_BLOCK_BYTES)
        .min(DEFAULT_STREAMING_BLOCK_BYTES);
    let mut buffer = vec![0_u8; buffer_size];
    fill_benchmark_buffer(&mut buffer);
    buffer
}

fn write_workload_file(path: &Path, file_size: u64, buffer: &[u8]) -> Result<(), WorkloadError> {
    let mut file = File::create(path)?;
    let mut written = 0_u64;
    while written < file_size {
        let chunk = chunk_len(buffer.len(), file_size - written);
        file.write_all(&buffer[..chunk])?;
        written += chunk as u64;
    }

    Ok(())
}

/// Cache behavior expected for a benchmark run.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheMode {
    /// Run with normal platform file I/O.
    Enabled,
    /// Attempt standard best-effort cache-reduced platform I/O.
    Disabled,
}

/// Platform mechanism selected for cache behavior.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheControlMethod {
    /// Normal platform file I/O.
    NormalFileIo,
    /// Windows write-through file flag.
    WriteThrough,
    /// macOS `F_NOCACHE` file descriptor flag.
    FcntlNoCache,
    /// Linux `posix_fadvise(..., POSIX_FADV_DONTNEED)` hints.
    PosixFadviseDontNeed,
    /// No standard per-file best-effort method is implemented for this target.
    BestEffortUnavailable,
}

/// Execution lifetime for the benchmark runner.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionMode {
    /// Execute one benchmark run.
    RunOnce,
    /// Continue executing until stopped by the caller.
    Continuous,
}

fn workload_size_label(value: WorkloadSize) -> &'static str {
    match value {
        WorkloadSize::Preset(WorkloadPreset::OneGb) => "1 GB",
        WorkloadSize::Preset(WorkloadPreset::FourGb) => "4 GB",
        WorkloadSize::Preset(WorkloadPreset::SixteenGb) => "16 GB",
        WorkloadSize::Preset(WorkloadPreset::SixtyFourGb) => "64 GB",
        WorkloadSize::CustomGb(_) => "custom",
    }
}

fn next_workload_size(value: WorkloadSize, next: bool) -> WorkloadSize {
    const VALUES: [WorkloadSize; 4] = [
        WorkloadSize::Preset(WorkloadPreset::OneGb),
        WorkloadSize::Preset(WorkloadPreset::FourGb),
        WorkloadSize::Preset(WorkloadPreset::SixteenGb),
        WorkloadSize::Preset(WorkloadPreset::SixtyFourGb),
    ];
    cycle(value, &VALUES, next)
}

fn test_mode_label(value: DiskTestMode) -> &'static str {
    match value {
        DiskTestMode::ReadWrite => "read/write",
        DiskTestMode::WriteOnly => "write only",
        DiskTestMode::WriteOnceReadLoop => "write once, read loop",
    }
}

fn next_test_mode(value: DiskTestMode, next: bool) -> DiskTestMode {
    const VALUES: [DiskTestMode; 3] = [
        DiskTestMode::ReadWrite,
        DiskTestMode::WriteOnly,
        DiskTestMode::WriteOnceReadLoop,
    ];
    cycle(value, &VALUES, next)
}

fn file_layout_label(value: FileLayout) -> &'static str {
    match value {
        FileLayout::SingleFile => "single file",
        FileLayout::HundredFilesPlusMinusFive => "100 files +/-5%",
        FileLayout::FixedFileSizeMb(_) => "fixed file size",
    }
}

fn next_file_layout(value: FileLayout, next: bool) -> FileLayout {
    const VALUES: [FileLayout; 2] = [
        FileLayout::SingleFile,
        FileLayout::HundredFilesPlusMinusFive,
    ];
    cycle(value, &VALUES, next)
}

fn cache_mode_label(value: CacheMode) -> &'static str {
    match value {
        CacheMode::Enabled => "enabled",
        CacheMode::Disabled => "disabled",
    }
}

fn next_cache_mode(value: CacheMode) -> CacheMode {
    match value {
        CacheMode::Enabled => CacheMode::Disabled,
        CacheMode::Disabled => CacheMode::Enabled,
    }
}

fn execution_mode_label(value: ExecutionMode) -> &'static str {
    match value {
        ExecutionMode::RunOnce => "run once",
        ExecutionMode::Continuous => "continuous",
    }
}

fn next_execution_mode(value: ExecutionMode) -> ExecutionMode {
    match value {
        ExecutionMode::RunOnce => ExecutionMode::Continuous,
        ExecutionMode::Continuous => ExecutionMode::RunOnce,
    }
}

fn bool_label(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

fn cycle<T: Copy + PartialEq>(value: T, values: &[T], next: bool) -> T {
    assert!(!values.is_empty(), "cannot cycle through an empty slice");
    let index = values.iter().position(|item| *item == value).unwrap_or(0);
    let index = if next {
        (index + 1) % values.len()
    } else {
        (index + values.len() - 1) % values.len()
    };
    values[index]
}

/// User-facing configuration validation error.
#[derive(Debug, Copy, Clone, Error, PartialEq, Eq)]
pub enum ConfigError {
    /// The target path is empty.
    #[error("target path must not be empty")]
    EmptyTargetPath,
    /// The workload size is zero.
    #[error("workload size must be greater than zero")]
    ZeroWorkload,
    /// The fixed file size is zero.
    #[error("file layout size must be greater than zero")]
    ZeroFileSize,
    /// The fixed file size is larger than the total workload.
    #[error("file layout size must not exceed total workload size")]
    FileLayoutExceedsWorkload,
    /// The workload size is too large for decimal byte representation.
    #[error("workload size is too large")]
    WorkloadOverflow,
}

/// Workload generation error.
#[derive(Debug, Error)]
pub enum WorkloadError {
    /// Benchmark configuration is invalid for workload generation.
    #[error("{0}")]
    Config(#[from] ConfigError),
    /// The requested total size cannot produce non-empty files for the layout.
    #[error("workload size is too small for the selected file layout")]
    WorkloadTooSmallForLayout,
    /// Filesystem I/O failed.
    #[error("{0}")]
    Io(#[from] std::io::Error),
}

fn phase_label(phase: StreamingIoPhase) -> &'static str {
    match phase {
        StreamingIoPhase::Write => "write",
        StreamingIoPhase::Read => "read",
    }
}

#[cfg(test)]
mod tests;
