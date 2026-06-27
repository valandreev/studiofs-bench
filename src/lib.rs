//! Benchmark configuration contract shared by CLI, TUI, runner, and reports.

#![deny(missing_docs)]

mod config_workload_tui;
mod runner_streaming;

pub use config_workload_tui::{
    BenchmarkConfig, CacheControlMethod, CacheMode, ConfigError, DiskTestMode, ExecutionMode,
    FileLayout, RunMode, TerminalUi, UiAction, Workload, WorkloadError, WorkloadFile,
    WorkloadPreset, WorkloadSize,
};
pub use runner_streaming::{
    BenchmarkPassMetrics, BenchmarkPassReport, BenchmarkRunner, BenchmarkRunnerError,
    BenchmarkRunnerReport, StreamingIoEngine, StreamingIoError, StreamingIoPhase,
    StreamingIoReport, StreamingIoReportMetadata, StreamingIoSample,
};

pub(crate) use config_workload_tui::{DECIMAL_MB, DEFAULT_STREAMING_BLOCK_BYTES};
