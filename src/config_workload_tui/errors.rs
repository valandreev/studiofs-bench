use std::{io, path::PathBuf};

use thiserror::Error;

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
