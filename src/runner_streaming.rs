use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

#[cfg(test)]
use std::cell::Cell;
#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};

use serde::Serialize;
use thiserror::Error;

use crate::config_workload_tui::STAMP_INTERVAL_BYTES;
use crate::{
    BenchmarkConfig, CacheControlMethod, CacheMode, ConfigError, DECIMAL_MB,
    DEFAULT_STREAMING_BLOCK_BYTES, DiskTestMode, ExecutionMode, Workload, WorkloadError,
    WorkloadFile,
};

mod cache_io;

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

        if let Err(error) = run_result {
            return Err(BenchmarkRunnerError::RunFailed {
                source: Box::new(error),
                partial_report: Box::new(report),
            });
        }
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
        let mut elapsed_io = Duration::ZERO;
        let mut touched_paths = Vec::new();
        let mut last_write_sample = None;
        let mut deferred_final_sample = None;
        let max_file_bytes = files.iter().map(|file| file.bytes).max().unwrap_or(0);
        let mut buffer = self.engine.buffer_for_bytes(max_file_bytes);
        let mut throughput_samples = Vec::with_capacity(sample_capacity(files, buffer.len()));
        if phase == StreamingIoPhase::Write {
            fill_benchmark_buffer(&mut buffer);
        }

        for (file_index, file) in files.iter().enumerate() {
            if should_stop() {
                stopped = true;
                break;
            }

            let mut file_elapsed_io = Duration::ZERO;
            let report = {
                let mut observe_sample = |mut sample: StreamingIoSample| {
                    let file_bytes_processed = sample.bytes_processed;
                    sample.offset += bytes_processed;
                    sample.bytes_processed += bytes_processed;
                    sample.elapsed_io += elapsed_io;
                    sample.mb_per_second =
                        sample_throughput(sample.bytes_processed, sample.elapsed_io);
                    file_elapsed_io = sample.elapsed_io - elapsed_io;
                    if phase == StreamingIoPhase::Write
                        && config.batch_fsync
                        && file_index == files.len() - 1
                        && file_bytes_processed == file.bytes
                    {
                        deferred_final_sample = Some(sample);
                    } else {
                        last_write_sample = Some(sample);
                        metrics.add(sample.mb_per_second);
                        throughput_samples.push(sample.mb_per_second);
                        on_sample(sample);
                    }
                };
                match phase {
                    StreamingIoPhase::Write => StreamingIoEngine::write_with_buffer(
                        StreamingIoPass {
                            path: &file.path,
                            total_bytes: file.bytes,
                            cache_mode: config.cache_mode,
                            pass_number,
                            sync_on_complete: !config.batch_fsync,
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
                            sync_on_complete: false,
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

            elapsed_io += file_elapsed_io;
            if phase == StreamingIoPhase::Write && config.batch_fsync && report.bytes_written > 0 {
                touched_paths.push(file.path.as_path());
            }
            if stopped {
                break;
            }
        }

        if phase == StreamingIoPhase::Write && config.batch_fsync {
            for path in touched_paths {
                let sync_started = Instant::now();
                sync_path(path)?;
                elapsed_io += measured_sync_elapsed(sync_started);
            }
            let final_sample =
                deferred_final_sample.or(if stopped { last_write_sample } else { None });
            if let Some(mut sample) = final_sample {
                sample.elapsed_io = elapsed_io;
                sample.timestamp = SystemTime::now();
                sample.mb_per_second = sample_throughput(sample.bytes_processed, elapsed_io);
                metrics.add(sample.mb_per_second);
                throughput_samples.push(sample.mb_per_second);
                on_sample(sample);
            }
        }

        let mut metrics = metrics.finish();
        if bytes_processed > 0 {
            metrics.average_mb_per_second = sample_throughput(bytes_processed, elapsed_io);
        }

        Ok(BenchmarkPassReport {
            phase,
            pass_number,
            bytes_processed,
            stopped,
            metrics,
            throughput_samples,
        })
    }
}

fn sample_capacity(files: &[WorkloadFile], block_size: usize) -> usize {
    const MAX_SAMPLE_CAPACITY: usize = 16_384;

    let Ok(block_size) = u64::try_from(block_size) else {
        return 0;
    };
    if block_size == 0 {
        return 0;
    }

    let capacity = files.iter().fold(0_usize, |capacity, file| {
        let file_samples = usize::try_from(file.bytes.div_ceil(block_size)).unwrap_or(usize::MAX);
        capacity.saturating_add(file_samples)
    });
    capacity.min(MAX_SAMPLE_CAPACITY)
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
    /// Total processed bytes divided by measured I/O time for a completed pass.
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
    /// Benchmark failed after collecting a partial report.
    #[error(
        "benchmark failed after {} completed passes: {source}",
        partial_report.passes.len()
    )]
    RunFailed {
        /// Error that stopped the run.
        #[source]
        source: Box<BenchmarkRunnerError>,
        /// Report accumulated before the failure.
        partial_report: Box<BenchmarkRunnerReport>,
    },
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
    /// Returns [`StreamingIoError::PathIo`] when creating, opening, reading,
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
    /// Returns [`StreamingIoError::PathIo`] when creating, opening, reading,
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
    /// Returns [`StreamingIoError::PathIo`] when creating, writing, or syncing the file fails.
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
                sync_on_complete: true,
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
        let alignment = cache_io::alignment(&output, pass.cache_mode)
            .map_err(|error| path_io_error(pass.path, error))?;
        let mut aligned;
        let buffer = if alignment > 1 {
            aligned = cache_io::AlignedBuffer::new(buffer.len(), alignment)
                .map_err(|error| path_io_error(pass.path, error))?;
            let buffer = aligned.as_mut_slice();
            fill_benchmark_buffer(buffer);
            buffer
        } else {
            buffer
        };
        report.bytes_written =
            stream_write(pass, &mut output, buffer, alignment, on_sample, should_stop)?;
        report.stopped = report.bytes_written < pass.total_bytes;

        Ok(report)
    }

    /// Runs one sequential read pass.
    ///
    /// # Errors
    ///
    /// Returns [`StreamingIoError::PathIo`] when opening or reading the file fails.
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
                sync_on_complete: false,
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
        let alignment = cache_io::alignment(&input, pass.cache_mode)
            .map_err(|error| path_io_error(pass.path, error))?;
        let mut aligned;
        let buffer = if alignment > 1 {
            aligned = cache_io::AlignedBuffer::new(buffer.len(), alignment)
                .map_err(|error| path_io_error(pass.path, error))?;
            aligned.as_mut_slice()
        } else {
            buffer
        };
        report.bytes_read =
            stream_read(pass, &mut input, buffer, alignment, on_sample, should_stop)?;
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
    sync_on_complete: bool,
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
    cache_io::configure_options(&mut options, mode).map_err(|error| path_io_error(path, error))?;
    let file = options
        .open(path)
        .map_err(|error| path_io_error(path, error))?;
    cache_io::configure_file(&file, mode).map_err(|error| path_io_error(path, error))?;
    Ok(file)
}

fn open_file(path: &std::path::Path, mode: CacheMode) -> Result<File, StreamingIoError> {
    let mut options = OpenOptions::new();
    options.read(true);
    cache_io::configure_options(&mut options, mode).map_err(|error| path_io_error(path, error))?;
    let file = options
        .open(path)
        .map_err(|error| path_io_error(path, error))?;
    cache_io::configure_file(&file, mode).map_err(|error| path_io_error(path, error))?;
    Ok(file)
}

fn path_io_error(path: &Path, source: io::Error) -> StreamingIoError {
    StreamingIoError::PathIo {
        path: path.to_owned(),
        source,
    }
}

fn sync_path(path: &Path) -> Result<(), StreamingIoError> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(|error| path_io_error(path, error))?;
    sync_file(path, &file)
}

fn sync_file(path: &Path, file: &File) -> Result<(), StreamingIoError> {
    note_sync_call(path);
    file.sync_all().map_err(|error| path_io_error(path, error))
}

#[cfg(windows)]
fn disabled_cache_method() -> CacheControlMethod {
    CacheControlMethod::NoBuffering
}

#[cfg(target_os = "macos")]
fn disabled_cache_method() -> CacheControlMethod {
    CacheControlMethod::FcntlNoCache
}

#[cfg(target_os = "linux")]
fn disabled_cache_method() -> CacheControlMethod {
    CacheControlMethod::DirectIo
}

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
fn disabled_cache_method() -> CacheControlMethod {
    CacheControlMethod::Unsupported
}

fn stream_write(
    pass: StreamingIoPass<'_>,
    output: &mut File,
    buffer: &mut [u8],
    alignment: usize,
    on_sample: &mut impl FnMut(StreamingIoSample),
    should_stop: &mut impl FnMut() -> bool,
) -> Result<u64, StreamingIoError> {
    let mut elapsed_io = Duration::ZERO;
    let mut processed = 0;

    while processed < pass.total_bytes {
        if should_stop() {
            break;
        }

        let offset = processed;
        let chunk = chunk_len(buffer.len(), pass.total_bytes - processed);
        let is_final_chunk = processed + chunk as u64 == pass.total_bytes;
        let transfer_len = cache_io::rounded_len(chunk, alignment)
            .map_err(|error| path_io_error(pass.path, error))?;
        stamp_block_offset(&mut buffer[..chunk], offset);
        buffer[chunk..transfer_len].fill(0);
        let io_start = Instant::now();
        if alignment == 1 {
            output
                .write_all(&buffer[..chunk])
                .map_err(|error| path_io_error(pass.path, error))?;
        } else {
            let written = retry_interrupted(|| output.write(&buffer[..transfer_len]))
                .map_err(|error| path_io_error(pass.path, error))?;
            if written != transfer_len {
                return Err(path_io_error(
                    pass.path,
                    io::Error::new(io::ErrorKind::WriteZero, "short direct I/O write"),
                ));
            }
            if is_final_chunk && transfer_len != chunk {
                output
                    .set_len(pass.total_bytes)
                    .map_err(|error| path_io_error(pass.path, error))?;
            }
        }
        if pass.sync_on_complete && is_final_chunk {
            sync_file(pass.path, output)?;
        }
        elapsed_io += io_start.elapsed();
        processed += chunk as u64;
        on_sample(sample(
            StreamingIoPhase::Write,
            pass.pass_number,
            offset,
            processed,
            elapsed_io,
        ));
    }

    Ok(processed)
}

#[cfg(test)]
static SYNC_CALL_COUNT: AtomicUsize = AtomicUsize::new(0);
#[cfg(test)]
static SYNC_OBSERVATION_DIRECTORY: std::sync::Mutex<Option<PathBuf>> = std::sync::Mutex::new(None);
#[cfg(test)]
static SYNC_PATH_OBSERVATIONS: std::sync::Mutex<Vec<PathBuf>> = std::sync::Mutex::new(Vec::new());
#[cfg(test)]
static SYNC_FILE_LENGTH_OBSERVATIONS: std::sync::Mutex<Vec<Vec<(PathBuf, u64)>>> =
    std::sync::Mutex::new(Vec::new());
#[cfg(test)]
std::thread_local! {
    static SYNC_ELAPSED_OVERRIDE: Cell<Option<Duration>> = const { Cell::new(None) };
}

#[cfg(test)]
fn note_sync_call(path: &Path) {
    SYNC_CALL_COUNT.fetch_add(1, Ordering::Relaxed);
    if SYNC_OBSERVATION_DIRECTORY.lock().unwrap().as_deref() == path.parent() {
        let mut file_lengths = Vec::new();
        if let Some(directory) = path.parent()
            && let Ok(entries) = std::fs::read_dir(directory)
        {
            for entry in entries.flatten() {
                if let Ok(metadata) = entry.metadata()
                    && metadata.is_file()
                {
                    file_lengths.push((entry.path(), metadata.len()));
                }
            }
        }
        file_lengths.sort_by(|left, right| left.0.cmp(&right.0));
        SYNC_PATH_OBSERVATIONS.lock().unwrap().push(path.to_owned());
        SYNC_FILE_LENGTH_OBSERVATIONS
            .lock()
            .unwrap()
            .push(file_lengths);
    }
}

#[cfg(not(test))]
fn note_sync_call(_: &Path) {}

#[cfg(test)]
fn reset_sync_call_count() {
    SYNC_CALL_COUNT.store(0, Ordering::Relaxed);
    SYNC_PATH_OBSERVATIONS.lock().unwrap().clear();
    SYNC_FILE_LENGTH_OBSERVATIONS.lock().unwrap().clear();
}

#[cfg(test)]
fn sync_call_count() -> usize {
    SYNC_CALL_COUNT.load(Ordering::Relaxed)
}

#[cfg(test)]
fn set_sync_observation_directory(directory: Option<PathBuf>) {
    *SYNC_OBSERVATION_DIRECTORY.lock().unwrap() = directory;
}

#[cfg(test)]
fn set_sync_elapsed_override(elapsed: Option<Duration>) {
    SYNC_ELAPSED_OVERRIDE.with(|override_duration| override_duration.set(elapsed));
}

fn measured_sync_elapsed(started: Instant) -> Duration {
    #[cfg(test)]
    {
        if let Some(elapsed) = SYNC_ELAPSED_OVERRIDE.with(Cell::get) {
            return elapsed;
        }
    }
    started.elapsed()
}

fn stream_read(
    pass: StreamingIoPass<'_>,
    input: &mut File,
    buffer: &mut [u8],
    alignment: usize,
    on_sample: &mut impl FnMut(StreamingIoSample),
    should_stop: &mut impl FnMut() -> bool,
) -> Result<u64, StreamingIoError> {
    let StreamingIoPass {
        path,
        total_bytes,
        pass_number,
        ..
    } = pass;
    let mut elapsed_io = Duration::ZERO;
    let mut processed = 0;

    while processed < total_bytes {
        if should_stop() {
            break;
        }

        let offset = processed;
        let chunk = chunk_len(buffer.len(), total_bytes - processed);
        let transfer_len =
            cache_io::rounded_len(chunk, alignment).map_err(|error| path_io_error(path, error))?;
        let io_start = Instant::now();
        if alignment == 1 {
            input
                .read_exact(&mut buffer[..chunk])
                .map_err(|error| path_io_error(path, error))?;
        } else {
            let read = retry_interrupted(|| input.read(&mut buffer[..transfer_len]))
                .map_err(|error| path_io_error(path, error))?;
            if read < chunk {
                return Err(path_io_error(
                    path,
                    io::Error::new(io::ErrorKind::UnexpectedEof, "short direct I/O read"),
                ));
            }
        }
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

fn retry_interrupted<T>(mut operation: impl FnMut() -> io::Result<T>) -> io::Result<T> {
    loop {
        match operation() {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            result => return result,
        }
    }
}

pub(crate) fn fill_benchmark_buffer(buffer: &mut [u8]) {
    let mut state = 0x1234_5678_u32;

    for byte in buffer {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        *byte = (state >> 24) as u8;
    }
}

fn sample(
    phase: StreamingIoPhase,
    pass_number: u64,
    offset: u64,
    bytes_processed: u64,
    elapsed_io: Duration,
) -> StreamingIoSample {
    StreamingIoSample {
        phase,
        pass_number,
        timestamp: SystemTime::now(),
        offset,
        bytes_processed,
        mb_per_second: sample_throughput(bytes_processed, elapsed_io),
        elapsed_io,
    }
}

#[expect(
    clippy::cast_precision_loss,
    reason = "throughput is an approximate human-facing metric"
)]
fn sample_throughput(bytes_processed: u64, elapsed_io: Duration) -> f64 {
    bytes_processed as f64 / DECIMAL_MB as f64 / elapsed_io.as_secs_f64().max(f64::EPSILON)
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
    /// Cumulative time spent in benchmark I/O, excluding sample callbacks.
    pub elapsed_io: Duration,
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
    /// Filesystem I/O failed for a benchmark path.
    #[error("I/O failed for {}: {source}", path.display())]
    PathIo {
        /// Path involved in the failed operation.
        path: PathBuf,
        /// Source I/O error.
        #[source]
        source: io::Error,
    },
}

#[cfg(test)]
mod tests;
