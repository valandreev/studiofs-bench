//! Benchmark configuration contract shared by CLI, TUI, runner, and reports.

#![deny(missing_docs)]

use std::error::Error;
use std::fmt;
use std::fs::File;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

use serde::Serialize;

const DECIMAL_MB: u64 = 1_000_000;
const MB_PER_GB: u64 = 1_000;
const DEFAULT_STREAMING_BLOCK_BYTES: usize = 8 * 1024 * 1024;

/// Complete benchmark settings for one configured run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BenchmarkConfig {
    /// Filesystem path where benchmark files are created.
    pub target_path: PathBuf,
    /// Total benchmark data size.
    pub workload_size: WorkloadSize,
    /// Filesystem access mode under test.
    pub run_mode: RunMode,
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
            file_layout: FileLayout::SingleFile,
            cache_mode: CacheMode::Warm,
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

/// File layout used for generated benchmark data.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FileLayout {
    /// Store the workload in one file.
    SingleFile,
    /// Split the workload into files of this decimal MB size.
    ///
    /// The final file may be smaller when the workload is not evenly divisible.
    FixedFileSizeMb(u64),
}

/// Cache behavior expected for a benchmark run.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheMode {
    /// Run with cold-cache expectations.
    Cold,
    /// Run with warm-cache expectations.
    Warm,
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
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum ConfigError {
    /// The target path is empty.
    EmptyTargetPath,
    /// The workload size is zero.
    ZeroWorkload,
    /// The fixed file size is zero.
    ZeroFileSize,
    /// The fixed file size is larger than the total workload.
    FileLayoutExceedsWorkload,
    /// The workload size is too large for decimal byte representation.
    WorkloadOverflow,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyTargetPath => f.write_str("target path must not be empty"),
            Self::ZeroWorkload => f.write_str("workload size must be greater than zero"),
            Self::ZeroFileSize => f.write_str("file layout size must be greater than zero"),
            Self::FileLayoutExceedsWorkload => {
                f.write_str("file layout size must not exceed total workload size")
            }
            Self::WorkloadOverflow => f.write_str("workload size is too large"),
        }
    }
}

impl Error for ConfigError {}

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
        let path = path.as_ref();
        let buffer_size = usize::try_from(total_bytes)
            .unwrap_or(self.block_size)
            .min(self.block_size);
        let mut buffer = vec![0_u8; buffer_size];
        fill_benchmark_buffer(&mut buffer);
        let mut report = StreamingIoReport::default();

        let mut output = File::create(path)?;
        report.bytes_written = stream_write(
            &mut output,
            &buffer,
            total_bytes,
            &mut on_sample,
            &mut should_stop,
        )?;
        drop(output);

        if report.bytes_written < total_bytes {
            report.stopped = true;
            return Ok(report);
        }

        let mut input = File::open(path)?;
        report.bytes_read = stream_read(
            &mut input,
            &mut buffer,
            total_bytes,
            &mut on_sample,
            &mut should_stop,
        )?;
        report.stopped = report.bytes_read < total_bytes;

        Ok(report)
    }
}

fn stream_write(
    output: &mut File,
    buffer: &[u8],
    total_bytes: u64,
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
        let io_start = Instant::now();
        output.write_all(&buffer[..chunk])?;
        if is_final_chunk {
            output.sync_all()?;
        }
        elapsed_io += io_start.elapsed();
        processed += chunk as u64;
        on_sample(sample(
            StreamingIoPhase::Write,
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
    offset: u64,
    bytes_processed: u64,
    elapsed_io: Duration,
) -> StreamingIoSample {
    let elapsed = elapsed_io.as_secs_f64().max(f64::EPSILON);

    StreamingIoSample {
        phase,
        pass_number: 1,
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
    /// One-based pass number.
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
    /// Bytes written during the write pass.
    pub bytes_written: u64,
    /// Bytes read during the read pass.
    pub bytes_read: u64,
    /// Whether the caller requested a clean stop between blocks.
    pub stopped: bool,
}

/// Sequential streaming engine error.
#[derive(Debug)]
pub enum StreamingIoError {
    /// Block size must be non-zero.
    ZeroBlockSize,
    /// Filesystem I/O failed.
    Io(std::io::Error),
}

impl fmt::Display for StreamingIoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroBlockSize => f.write_str("streaming block size must be greater than zero"),
            Self::Io(error) => write!(f, "{error}"),
        }
    }
}

impl Error for StreamingIoError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::ZeroBlockSize => None,
            Self::Io(error) => Some(error),
        }
    }
}

impl From<std::io::Error> for StreamingIoError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
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
    fn streaming_io_error_exposes_io_source() {
        let error =
            StreamingIoError::from(std::io::Error::new(std::io::ErrorKind::NotFound, "missing"));

        assert!(std::error::Error::source(&error).is_some());
    }
}
