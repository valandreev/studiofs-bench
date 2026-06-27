//! Benchmark configuration contract shared by CLI, TUI, runner, and reports.

#![deny(missing_docs)]

use std::error::Error;
use std::fmt;
use std::path::PathBuf;

use serde::Serialize;

const DECIMAL_MB: u64 = 1_000_000;
const MB_PER_GB: u64 = 1_000;

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
    pub fn gigabytes(self) -> u64 {
        match self {
            Self::Preset(preset) => preset.gigabytes(),
            Self::CustomGb(gigabytes) => gigabytes,
        }
    }

    /// Size in decimal megabytes.
    pub fn megabytes(self) -> Option<u64> {
        self.gigabytes().checked_mul(MB_PER_GB)
    }

    /// Size in decimal bytes.
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
