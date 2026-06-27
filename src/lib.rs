//! Benchmark configuration contract shared by CLI, TUI, runner, and reports.

#![deny(missing_docs)]

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime};

use serde::Serialize;
use thiserror::Error;

const DECIMAL_MB: u64 = 1_000_000;
const MB_PER_GB: u64 = 1_000;
const DEFAULT_STREAMING_BLOCK_BYTES: usize = 8 * 1024 * 1024;
const MAX_FIXED_LAYOUT_FILES: usize = 100_000;
const STAMP_INTERVAL_BYTES: usize = 4 * 1024;
static RUN_DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

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
            (u128::from(weighted_bytes) * u128::from(weight) / u128::from(WEIGHT_SUM)) as u64;
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

/// Runs benchmark workloads according to [`BenchmarkConfig`].
#[derive(Debug, Copy, Clone, Default)]
pub struct BenchmarkRunner {
    engine: StreamingIoEngine,
}

impl BenchmarkRunner {
    /// Creates a runner with a custom streaming block size.
    ///
    /// # Errors
    ///
    /// Returns [`StreamingIoError::ZeroBlockSize`] when `block_size` is zero.
    pub fn with_block_size(block_size: usize) -> Result<Self, StreamingIoError> {
        Ok(Self {
            engine: StreamingIoEngine::with_block_size(block_size)?,
        })
    }

    /// Creates and runs a workload from `config`.
    ///
    /// # Errors
    ///
    /// Returns [`BenchmarkRunnerError`] when config validation, workload creation, or benchmark
    /// I/O fails.
    pub fn run(
        self,
        config: &BenchmarkConfig,
        on_sample: impl FnMut(StreamingIoSample),
        should_stop: impl FnMut() -> bool,
    ) -> Result<BenchmarkRunnerReport, BenchmarkRunnerError> {
        let workload = Workload::create(config)?;
        self.run_workload(workload, config, on_sample, should_stop)
    }

    /// Runs an existing workload.
    ///
    /// This is useful for tests and for callers that need to prepare the run directory before the
    /// timed benchmark passes.
    ///
    /// # Errors
    ///
    /// Returns [`BenchmarkRunnerError`] when config validation or benchmark I/O fails.
    pub fn run_workload(
        self,
        workload: Workload,
        config: &BenchmarkConfig,
        mut on_sample: impl FnMut(StreamingIoSample),
        mut should_stop: impl FnMut() -> bool,
    ) -> Result<BenchmarkRunnerReport, BenchmarkRunnerError> {
        config.validate()?;
        let run_dir = workload.run_dir().to_owned();
        let mut report = BenchmarkRunnerReport {
            run_dir,
            files_kept: config.keep_files,
            cleanup_error: None,
            passes: Vec::new(),
            stopped: false,
        };

        let run_result = self.run_passes(
            workload.files(),
            config,
            &mut report,
            &mut on_sample,
            &mut should_stop,
        );

        if !config.keep_files
            && let Err(error) = workload.cleanup()
        {
            report.cleanup_error = Some(error.to_string());
        }

        run_result?;
        Ok(report)
    }

    fn run_passes(
        self,
        files: &[WorkloadFile],
        config: &BenchmarkConfig,
        report: &mut BenchmarkRunnerReport,
        on_sample: &mut impl FnMut(StreamingIoSample),
        should_stop: &mut impl FnMut() -> bool,
    ) -> Result<(), BenchmarkRunnerError> {
        match config.test_mode {
            DiskTestMode::ReadWrite => self.run_phase_loop(
                files,
                &[StreamingIoPhase::Write, StreamingIoPhase::Read],
                config,
                report,
                on_sample,
                should_stop,
            )?,
            DiskTestMode::WriteOnly => self.run_phase_loop(
                files,
                &[StreamingIoPhase::Write],
                config,
                report,
                on_sample,
                should_stop,
            )?,
            DiskTestMode::WriteOnceReadLoop => {
                let write_pass = self.run_files(
                    files,
                    StreamingIoPhase::Write,
                    1,
                    config,
                    on_sample,
                    should_stop,
                )?;
                report.stopped |= write_pass.stopped;
                report.passes.push(write_pass);

                if !report.stopped {
                    self.run_phase_loop(
                        files,
                        &[StreamingIoPhase::Read],
                        config,
                        report,
                        on_sample,
                        should_stop,
                    )?;
                }
            }
        }

        Ok(())
    }

    fn run_phase_loop(
        self,
        files: &[WorkloadFile],
        phases: &[StreamingIoPhase],
        config: &BenchmarkConfig,
        report: &mut BenchmarkRunnerReport,
        on_sample: &mut impl FnMut(StreamingIoSample),
        should_stop: &mut impl FnMut() -> bool,
    ) -> Result<(), BenchmarkRunnerError> {
        let mut pass_number = 1;
        loop {
            for phase in phases {
                let pass =
                    self.run_files(files, *phase, pass_number, config, on_sample, should_stop)?;
                report.stopped |= pass.stopped;
                report.passes.push(pass);
                if report.stopped {
                    break;
                }
            }

            if config.execution_mode == ExecutionMode::RunOnce || report.stopped || should_stop() {
                break;
            }
            pass_number += 1;
        }

        Ok(())
    }

    fn run_files(
        self,
        files: &[WorkloadFile],
        phase: StreamingIoPhase,
        pass_number: u64,
        config: &BenchmarkConfig,
        on_sample: &mut impl FnMut(StreamingIoSample),
        should_stop: &mut impl FnMut() -> bool,
    ) -> Result<BenchmarkPassReport, BenchmarkRunnerError> {
        let mut bytes_processed = 0;
        let mut stopped = false;
        let max_file_bytes = files.iter().map(|file| file.bytes).max().unwrap_or(0);
        let mut buffer = self.engine.buffer_for_bytes(max_file_bytes);
        if phase == StreamingIoPhase::Write {
            fill_benchmark_buffer(&mut buffer);
        }

        for file in files {
            if should_stop() {
                stopped = true;
                break;
            }

            let report = match phase {
                StreamingIoPhase::Write => self.engine.write_with_buffer(
                    StreamingIoPass {
                        path: &file.path,
                        total_bytes: file.bytes,
                        cache_mode: config.cache_mode,
                        pass_number,
                    },
                    &mut buffer,
                    &mut *on_sample,
                    &mut *should_stop,
                )?,
                StreamingIoPhase::Read => self.engine.read_with_buffer(
                    StreamingIoPass {
                        path: &file.path,
                        total_bytes: file.bytes,
                        cache_mode: config.cache_mode,
                        pass_number,
                    },
                    &mut buffer,
                    &mut *on_sample,
                    &mut *should_stop,
                )?,
            };
            bytes_processed += match phase {
                StreamingIoPhase::Write => report.bytes_written,
                StreamingIoPhase::Read => report.bytes_read,
            };
            stopped = report.stopped;
            if stopped {
                break;
            }
        }

        Ok(BenchmarkPassReport {
            phase,
            pass_number,
            bytes_processed,
            stopped,
        })
    }
}

/// Summary returned by the benchmark runner.
#[derive(Debug, Clone, Serialize)]
pub struct BenchmarkRunnerReport {
    /// Benchmark-created run directory.
    pub run_dir: PathBuf,
    /// Whether generated files were kept after the run.
    pub files_kept: bool,
    /// Cleanup error text, if cleanup failed after benchmark results were collected.
    pub cleanup_error: Option<String>,
    /// Reports for completed phase passes.
    pub passes: Vec<BenchmarkPassReport>,
    /// Whether the caller stopped the run before a phase completed.
    pub stopped: bool,
}

/// Summary for one benchmark runner phase pass.
#[derive(Debug, Copy, Clone, Serialize)]
pub struct BenchmarkPassReport {
    /// Phase executed by this pass.
    pub phase: StreamingIoPhase,
    /// One-based pass number within this phase.
    pub pass_number: u64,
    /// Bytes processed across workload files.
    pub bytes_processed: u64,
    /// Whether the phase stopped before all files completed.
    pub stopped: bool,
}

/// Benchmark runner error.
#[derive(Debug, Error)]
pub enum BenchmarkRunnerError {
    /// Benchmark configuration is invalid.
    #[error("{0}")]
    Config(#[from] ConfigError),
    /// Workload generation failed.
    #[error("{0}")]
    Workload(#[from] WorkloadError),
    /// Streaming I/O failed.
    #[error("{0}")]
    StreamingIo(#[from] StreamingIoError),
}

/// Sequential streaming write/read engine.
#[derive(Debug, Copy, Clone)]
pub struct StreamingIoEngine {
    block_size: usize,
}

impl Default for StreamingIoEngine {
    fn default() -> Self {
        Self {
            block_size: DEFAULT_STREAMING_BLOCK_BYTES,
        }
    }
}

impl StreamingIoEngine {
    /// Creates an engine with a custom internal block size.
    ///
    /// # Errors
    ///
    /// Returns [`StreamingIoError::ZeroBlockSize`] when `block_size` is zero.
    pub fn with_block_size(block_size: usize) -> Result<Self, StreamingIoError> {
        if block_size == 0 {
            return Err(StreamingIoError::ZeroBlockSize);
        }

        Ok(Self { block_size })
    }

    /// Runs one sequential write pass followed by one sequential read pass.
    ///
    /// Samples are emitted with `pass_number` set to `1` because this engine
    /// executes one write/read pass pair per call.
    ///
    /// # Errors
    ///
    /// Returns [`StreamingIoError::Io`] when creating, opening, reading,
    /// writing, or syncing the benchmark file fails.
    pub fn run(
        self,
        path: impl AsRef<std::path::Path>,
        total_bytes: u64,
        mut on_sample: impl FnMut(StreamingIoSample),
        mut should_stop: impl FnMut() -> bool,
    ) -> Result<StreamingIoReport, StreamingIoError> {
        self.run_with_cache_mode(
            path,
            total_bytes,
            CacheMode::Enabled,
            &mut on_sample,
            &mut should_stop,
        )
    }

    /// Runs one sequential write/read pass with the requested cache behavior.
    ///
    /// # Errors
    ///
    /// Returns [`StreamingIoError::Io`] when creating, opening, reading,
    /// writing, or syncing the benchmark file fails.
    pub fn run_with_cache_mode(
        self,
        path: impl AsRef<std::path::Path>,
        total_bytes: u64,
        cache_mode: CacheMode,
        mut on_sample: impl FnMut(StreamingIoSample),
        mut should_stop: impl FnMut() -> bool,
    ) -> Result<StreamingIoReport, StreamingIoError> {
        let path = path.as_ref();
        let mut report = self.write_with_cache_mode(
            path,
            total_bytes,
            cache_mode,
            1,
            &mut on_sample,
            &mut should_stop,
        )?;

        if report.bytes_written < total_bytes {
            report.stopped = true;
            return Ok(report);
        }

        let read_report = self.read_with_cache_mode(
            path,
            total_bytes,
            cache_mode,
            1,
            &mut on_sample,
            &mut should_stop,
        )?;
        report.bytes_read = read_report.bytes_read;
        report.stopped = read_report.stopped;

        Ok(report)
    }

    /// Runs one sequential write pass.
    ///
    /// # Errors
    ///
    /// Returns [`StreamingIoError::Io`] when creating, writing, or syncing the file fails.
    pub fn write_with_cache_mode(
        self,
        path: impl AsRef<std::path::Path>,
        total_bytes: u64,
        cache_mode: CacheMode,
        pass_number: u64,
        mut on_sample: impl FnMut(StreamingIoSample),
        mut should_stop: impl FnMut() -> bool,
    ) -> Result<StreamingIoReport, StreamingIoError> {
        let path = path.as_ref();
        let mut buffer = self.buffer_for_bytes(total_bytes);
        fill_benchmark_buffer(&mut buffer);
        self.write_with_buffer(
            StreamingIoPass {
                path,
                total_bytes,
                cache_mode,
                pass_number,
            },
            &mut buffer,
            &mut on_sample,
            &mut should_stop,
        )
    }

    fn write_with_buffer(
        self,
        pass: StreamingIoPass<'_>,
        buffer: &mut [u8],
        on_sample: &mut impl FnMut(StreamingIoSample),
        should_stop: &mut impl FnMut() -> bool,
    ) -> Result<StreamingIoReport, StreamingIoError> {
        if pass.total_bytes == 0 {
            return Ok(empty_streaming_report(pass.cache_mode));
        }
        let mut report = empty_streaming_report(pass.cache_mode);

        let mut output = create_file(pass.path, pass.cache_mode)?;
        report.bytes_written = stream_write(
            &mut output,
            buffer,
            pass.total_bytes,
            pass.pass_number,
            on_sample,
            should_stop,
        )?;

        after_cache_io(&output, pass.cache_mode);
        report.stopped = report.bytes_written < pass.total_bytes;

        Ok(report)
    }

    /// Runs one sequential read pass.
    ///
    /// # Errors
    ///
    /// Returns [`StreamingIoError::Io`] when opening or reading the file fails.
    pub fn read_with_cache_mode(
        self,
        path: impl AsRef<std::path::Path>,
        total_bytes: u64,
        cache_mode: CacheMode,
        pass_number: u64,
        mut on_sample: impl FnMut(StreamingIoSample),
        mut should_stop: impl FnMut() -> bool,
    ) -> Result<StreamingIoReport, StreamingIoError> {
        let path = path.as_ref();
        let mut buffer = self.buffer_for_bytes(total_bytes);
        self.read_with_buffer(
            StreamingIoPass {
                path,
                total_bytes,
                cache_mode,
                pass_number,
            },
            &mut buffer,
            &mut on_sample,
            &mut should_stop,
        )
    }

    fn read_with_buffer(
        self,
        pass: StreamingIoPass<'_>,
        buffer: &mut [u8],
        on_sample: &mut impl FnMut(StreamingIoSample),
        should_stop: &mut impl FnMut() -> bool,
    ) -> Result<StreamingIoReport, StreamingIoError> {
        if pass.total_bytes == 0 {
            return Ok(empty_streaming_report(pass.cache_mode));
        }
        let mut report = empty_streaming_report(pass.cache_mode);

        let mut input = open_file(pass.path, pass.cache_mode)?;
        report.bytes_read = stream_read(
            &mut input,
            buffer,
            pass.total_bytes,
            pass.pass_number,
            on_sample,
            should_stop,
        )?;
        after_cache_io(&input, pass.cache_mode);
        report.stopped = report.bytes_read < pass.total_bytes;

        Ok(report)
    }

    fn buffer_for_bytes(self, total_bytes: u64) -> Vec<u8> {
        let buffer_size = usize::try_from(total_bytes)
            .unwrap_or(self.block_size)
            .min(self.block_size);
        vec![0_u8; buffer_size]
    }
}

#[derive(Debug, Copy, Clone)]
struct StreamingIoPass<'a> {
    path: &'a std::path::Path,
    total_bytes: u64,
    cache_mode: CacheMode,
    pass_number: u64,
}

fn empty_streaming_report(cache_mode: CacheMode) -> StreamingIoReport {
    let mut report = StreamingIoReport::default();
    report.metadata.cache_mode = cache_mode;
    report.metadata.cache_method = match cache_mode {
        CacheMode::Enabled => CacheControlMethod::NormalFileIo,
        CacheMode::Disabled => disabled_cache_method(),
    };
    report
}

fn create_file(path: &std::path::Path, mode: CacheMode) -> Result<File, StreamingIoError> {
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    apply_open_options(&mut options, mode);
    let file = options.open(path)?;
    apply_file_options(&file, mode);
    Ok(file)
}

fn open_file(path: &std::path::Path, mode: CacheMode) -> Result<File, StreamingIoError> {
    let mut options = OpenOptions::new();
    options.read(true);
    apply_open_options(&mut options, mode);
    let file = options.open(path)?;
    apply_file_options(&file, mode);
    Ok(file)
}

#[cfg(windows)]
fn apply_open_options(options: &mut OpenOptions, mode: CacheMode) {
    use std::os::windows::fs::OpenOptionsExt;

    const FILE_FLAG_WRITE_THROUGH: u32 = 0x8000_0000;

    if mode == CacheMode::Disabled {
        options.custom_flags(FILE_FLAG_WRITE_THROUGH);
    }
}

#[cfg(not(windows))]
fn apply_open_options(_: &mut OpenOptions, _: CacheMode) {}

#[cfg(target_os = "macos")]
fn apply_file_options(file: &File, mode: CacheMode) {
    use std::os::fd::AsRawFd;

    if mode == CacheMode::Disabled {
        // SAFETY: `file` owns a valid file descriptor and `F_NOCACHE` expects an int flag.
        // Errors are ignored because disabled cache mode is best-effort.
        let _ = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_NOCACHE, 1) };
    }
}

#[cfg(not(target_os = "macos"))]
fn apply_file_options(_: &File, _: CacheMode) {}

#[cfg(target_os = "linux")]
fn after_cache_io(file: &File, mode: CacheMode) {
    use std::os::fd::AsRawFd;

    if mode == CacheMode::Disabled {
        // SAFETY: `file` owns a valid file descriptor. This is an advisory best-effort hint.
        // Errors are ignored because disabled cache mode is best-effort.
        let _ = unsafe { libc::posix_fadvise(file.as_raw_fd(), 0, 0, libc::POSIX_FADV_DONTNEED) };
    }
}

#[cfg(not(target_os = "linux"))]
fn after_cache_io(_: &File, _: CacheMode) {}

#[cfg(windows)]
fn disabled_cache_method() -> CacheControlMethod {
    CacheControlMethod::WriteThrough
}

#[cfg(target_os = "macos")]
fn disabled_cache_method() -> CacheControlMethod {
    CacheControlMethod::FcntlNoCache
}

#[cfg(target_os = "linux")]
fn disabled_cache_method() -> CacheControlMethod {
    CacheControlMethod::PosixFadviseDontNeed
}

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
fn disabled_cache_method() -> CacheControlMethod {
    CacheControlMethod::BestEffortUnavailable
}

fn stream_write(
    output: &mut File,
    buffer: &mut [u8],
    total_bytes: u64,
    pass_number: u64,
    on_sample: &mut impl FnMut(StreamingIoSample),
    should_stop: &mut impl FnMut() -> bool,
) -> Result<u64, StreamingIoError> {
    let mut elapsed_io = Duration::ZERO;
    let mut processed = 0;

    while processed < total_bytes {
        if should_stop() {
            break;
        }

        let offset = processed;
        let chunk = chunk_len(buffer.len(), total_bytes - processed);
        let is_final_chunk = processed + chunk as u64 == total_bytes;
        stamp_block_offset(&mut buffer[..chunk], offset);
        let io_start = Instant::now();
        output.write_all(&buffer[..chunk])?;
        if is_final_chunk {
            output.sync_all()?;
        }
        elapsed_io += io_start.elapsed();
        processed += chunk as u64;
        on_sample(sample(
            StreamingIoPhase::Write,
            pass_number,
            offset,
            processed,
            elapsed_io,
        ));
    }

    Ok(processed)
}

fn stream_read(
    input: &mut File,
    buffer: &mut [u8],
    total_bytes: u64,
    pass_number: u64,
    on_sample: &mut impl FnMut(StreamingIoSample),
    should_stop: &mut impl FnMut() -> bool,
) -> Result<u64, StreamingIoError> {
    let mut elapsed_io = Duration::ZERO;
    let mut processed = 0;

    while processed < total_bytes {
        if should_stop() {
            break;
        }

        let offset = processed;
        let chunk = chunk_len(buffer.len(), total_bytes - processed);
        let io_start = Instant::now();
        input.read_exact(&mut buffer[..chunk])?;
        elapsed_io += io_start.elapsed();
        processed += chunk as u64;
        on_sample(sample(
            StreamingIoPhase::Read,
            pass_number,
            offset,
            processed,
            elapsed_io,
        ));
    }

    Ok(processed)
}

fn chunk_len(block_size: usize, remaining: u64) -> usize {
    usize::try_from(remaining)
        .unwrap_or(block_size)
        .min(block_size)
}

fn fill_benchmark_buffer(buffer: &mut [u8]) {
    let mut state = 0x1234_5678_u32;

    for byte in buffer {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        *byte = (state >> 24) as u8;
    }
}

#[expect(
    clippy::cast_precision_loss,
    reason = "throughput is an approximate human-facing metric"
)]
fn sample(
    phase: StreamingIoPhase,
    pass_number: u64,
    offset: u64,
    bytes_processed: u64,
    elapsed_io: Duration,
) -> StreamingIoSample {
    let elapsed = elapsed_io.as_secs_f64().max(f64::EPSILON);

    StreamingIoSample {
        phase,
        pass_number,
        timestamp: SystemTime::now(),
        offset,
        bytes_processed,
        mb_per_second: bytes_processed as f64 / DECIMAL_MB as f64 / elapsed,
    }
}

/// Sequential streaming phase.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamingIoPhase {
    /// Sustained sequential write phase.
    Write,
    /// Sustained sequential read phase.
    Read,
}

/// Structured progress sample emitted after a block completes.
#[derive(Debug, Clone, Serialize)]
pub struct StreamingIoSample {
    /// Phase that emitted the sample.
    pub phase: StreamingIoPhase,
    /// One-based pass number within this phase.
    pub pass_number: u64,
    /// Wall-clock timestamp when the sample was emitted.
    pub timestamp: SystemTime,
    /// Byte offset for the completed block.
    pub offset: u64,
    /// Cumulative bytes processed in the current phase.
    pub bytes_processed: u64,
    /// Current cumulative throughput in decimal MB/s.
    pub mb_per_second: f64,
}

/// Summary returned by one sequential streaming run.
#[derive(Debug, Default, Copy, Clone, Serialize)]
pub struct StreamingIoReport {
    /// Report metadata for the run.
    pub metadata: StreamingIoReportMetadata,
    /// Bytes written during the write pass.
    pub bytes_written: u64,
    /// Bytes read during the read pass.
    pub bytes_read: u64,
    /// Whether the caller requested a clean stop between blocks.
    pub stopped: bool,
}

fn stamp_block_offset(chunk: &mut [u8], offset: u64) {
    for stamp_offset in (0..chunk.len()).step_by(STAMP_INTERVAL_BYTES) {
        let stamp = (offset + stamp_offset as u64).to_le_bytes();
        let stamp_end = (stamp_offset + stamp.len()).min(chunk.len());
        chunk[stamp_offset..stamp_end].copy_from_slice(&stamp[..stamp_end - stamp_offset]);
    }
}

/// Metadata describing selected benchmark run behavior.
#[derive(Debug, Copy, Clone, Serialize)]
pub struct StreamingIoReportMetadata {
    /// Requested cache mode.
    pub cache_mode: CacheMode,
    /// Platform cache-control method selected for the request.
    pub cache_method: CacheControlMethod,
}

impl Default for StreamingIoReportMetadata {
    fn default() -> Self {
        Self {
            cache_mode: CacheMode::Enabled,
            cache_method: CacheControlMethod::NormalFileIo,
        }
    }
}

/// Sequential streaming engine error.
#[derive(Debug, Error)]
pub enum StreamingIoError {
    /// Block size must be non-zero.
    #[error("streaming block size must be greater than zero")]
    ZeroBlockSize,
    /// Filesystem I/O failed.
    #[error("{0}")]
    Io(#[from] std::io::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_len_keeps_large_remaining_sizes_in_u64_until_after_min() {
        assert_eq!(chunk_len(8 * 1024 * 1024, 1_u64 << 32), 8 * 1024 * 1024);
    }

    #[test]
    fn fill_benchmark_buffer_uses_non_zero_deterministic_bytes() {
        let mut buffer = [0_u8; 8];

        fill_benchmark_buffer(&mut buffer);

        assert_eq!(buffer, [117, 205, 37, 75, 132, 226, 234, 242]);
    }

    #[test]
    fn hundred_file_sizes_keep_exact_total_for_large_workloads() {
        let sizes = hundred_file_sizes(u64::MAX).unwrap();

        assert_eq!(sizes.iter().sum::<u64>(), u64::MAX);
    }

    #[test]
    fn hundred_file_sizes_keep_exact_total_near_minimum_size() {
        let sizes = hundred_file_sizes(101).unwrap();

        assert_eq!(sizes.iter().sum::<u64>(), 101);
        assert!(sizes.iter().all(|size| *size > 0));
    }

    #[test]
    fn fixed_file_sizes_rejects_too_many_files() {
        let error = fixed_file_sizes(100_001 * DECIMAL_MB, 1).unwrap_err();

        assert_eq!(error.to_string(), "workload size is too large");
    }

    #[test]
    fn cleanup_ignores_missing_run_dir() {
        let target = std::env::temp_dir().join(format!(
            "studiofs-bench-sfs-572-cleanup-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&target);
        std::fs::create_dir_all(&target).unwrap();

        let workload = Workload::create_for_bytes(&target, 1, FileLayout::SingleFile).unwrap();
        let run_dir = workload.run_dir().to_owned();
        std::fs::remove_dir_all(&run_dir).unwrap();

        assert!(workload.cleanup().is_ok());
        let _ = std::fs::remove_dir_all(&target);
    }

    #[test]
    fn write_workload_files_removes_run_dir_when_file_write_fails() {
        let run_dir = std::env::temp_dir().join(format!(
            "studiofs-bench-sfs-572-partial-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&run_dir);
        std::fs::create_dir_all(&run_dir).unwrap();

        let error = write_workload_files(&run_dir, vec![1, 2], |path, bytes| {
            if bytes == 2 {
                return Err(std::io::Error::other("write failed").into());
            }
            File::create(path)?;
            Ok(())
        })
        .unwrap_err();

        assert_eq!(error.to_string(), "write failed");
        assert!(!run_dir.exists());
    }

    #[test]
    fn write_workload_file_uses_supplied_buffer() {
        let path = std::env::temp_dir().join(format!(
            "studiofs-bench-sfs-572-buffer-{}.bin",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);

        write_workload_file(&path, 5, &[1, 2, 3]).unwrap();

        assert_eq!(std::fs::read(&path).unwrap(), vec![1, 2, 3, 1, 2]);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn streaming_io_error_exposes_io_source() {
        let error =
            StreamingIoError::from(std::io::Error::new(std::io::ErrorKind::NotFound, "missing"));

        assert!(std::error::Error::source(&error).is_some());
    }
}
