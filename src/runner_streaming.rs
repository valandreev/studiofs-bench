use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

use serde::Serialize;
use thiserror::Error;

use crate::config_workload_tui::STAMP_INTERVAL_BYTES;
use crate::{
    BenchmarkConfig, CacheControlMethod, CacheMode, ConfigError, DECIMAL_MB,
    DEFAULT_STREAMING_BLOCK_BYTES, DiskTestMode, ExecutionMode, Workload, WorkloadError,
    WorkloadFile,
};

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
        let mut metrics = MetricsAccumulator::default();
        let max_file_bytes = files.iter().map(|file| file.bytes).max().unwrap_or(0);
        let mut buffer = self.engine.buffer_for_bytes(max_file_bytes);
        let mut throughput_samples = Vec::with_capacity(sample_capacity(files, buffer.len()));
        if phase == StreamingIoPhase::Write {
            fill_benchmark_buffer(&mut buffer);
        }

        for file in files {
            if should_stop() {
                stopped = true;
                break;
            }

            let report = {
                let mut observe_sample = |mut sample: StreamingIoSample| {
                    sample.offset += bytes_processed;
                    sample.bytes_processed += bytes_processed;
                    metrics.add(sample.mb_per_second);
                    throughput_samples.push(sample.mb_per_second);
                    on_sample(sample);
                };
                match phase {
                    StreamingIoPhase::Write => StreamingIoEngine::write_with_buffer(
                        StreamingIoPass {
                            path: &file.path,
                            total_bytes: file.bytes,
                            cache_mode: config.cache_mode,
                            pass_number,
                        },
                        &mut buffer,
                        &mut observe_sample,
                        &mut *should_stop,
                    )?,
                    StreamingIoPhase::Read => StreamingIoEngine::read_with_buffer(
                        StreamingIoPass {
                            path: &file.path,
                            total_bytes: file.bytes,
                            cache_mode: config.cache_mode,
                            pass_number,
                        },
                        &mut buffer,
                        &mut observe_sample,
                        &mut *should_stop,
                    )?,
                }
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
            metrics: metrics.finish(),
            throughput_samples,
        })
    }
}

fn sample_capacity(files: &[WorkloadFile], block_size: usize) -> usize {
    let Ok(block_size) = u64::try_from(block_size) else {
        return usize::MAX;
    };
    if block_size == 0 {
        return 0;
    }

    files.iter().fold(0, |capacity, file| {
        let file_samples = usize::try_from(file.bytes.div_ceil(block_size)).unwrap_or(usize::MAX);
        capacity.saturating_add(file_samples)
    })
}

#[derive(Debug, Default)]
pub(crate) struct MetricsAccumulator {
    sample_count: u64,
    sum: f64,
    stable_sum: f64,
    stable_count: u64,
    minimum: Option<f64>,
    previous: Option<f64>,
    drop_count: u64,
}

impl MetricsAccumulator {
    pub(crate) fn add(&mut self, value: f64) {
        self.sample_count += 1;
        self.sum += value;
        self.minimum = Some(self.minimum.map_or(value, |minimum| minimum.min(value)));

        if self.previous.is_some_and(|previous| value < previous) {
            self.drop_count += 1;
        } else {
            self.stable_sum += value;
            self.stable_count += 1;
        }
        self.previous = Some(value);
    }

    #[expect(
        clippy::cast_precision_loss,
        reason = "sample averages are approximate human-facing throughput metrics"
    )]
    pub(crate) fn finish(&self) -> BenchmarkPassMetrics {
        let average = |sum: f64, count: u64| {
            if count == 0 { 0.0 } else { sum / count as f64 }
        };

        BenchmarkPassMetrics {
            sample_count: self.sample_count,
            average_mb_per_second: average(self.sum, self.sample_count),
            stable_mb_per_second: average(self.stable_sum, self.stable_count),
            minimum_mb_per_second: self.minimum.unwrap_or(0.0),
            drop_count: self.drop_count,
        }
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
#[derive(Debug, Clone, Serialize)]
pub struct BenchmarkPassReport {
    /// Phase executed by this pass.
    pub phase: StreamingIoPhase,
    /// One-based pass number within this phase.
    pub pass_number: u64,
    /// Bytes processed across workload files.
    pub bytes_processed: u64,
    /// Whether the phase stopped before all files completed.
    pub stopped: bool,
    /// Metrics calculated from samples emitted during this pass.
    pub metrics: BenchmarkPassMetrics,
    /// Throughput samples emitted during this pass, in decimal MB/s.
    pub throughput_samples: Vec<f64>,
}

/// Throughput metrics calculated from samples for one benchmark pass.
#[derive(Debug, Default, Copy, Clone, Serialize)]
pub struct BenchmarkPassMetrics {
    /// Number of samples included in the metrics.
    pub sample_count: u64,
    /// Mean sample throughput in decimal MB/s.
    pub average_mb_per_second: f64,
    /// Mean throughput excluding samples lower than the previous sample.
    pub stable_mb_per_second: f64,
    /// Lowest sample throughput in decimal MB/s.
    pub minimum_mb_per_second: f64,
    /// Count of samples lower than the previous sample.
    pub drop_count: u64,
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
        Self::write_with_buffer(
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
        Self::read_with_buffer(
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

pub(crate) fn chunk_len(block_size: usize, remaining: u64) -> usize {
    usize::try_from(remaining)
        .unwrap_or(block_size)
        .min(block_size)
}

pub(crate) fn fill_benchmark_buffer(buffer: &mut [u8]) {
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
#[derive(Debug, Copy, Clone, Serialize)]
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
mod tests;
