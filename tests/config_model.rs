//! Benchmark configuration model tests.

use std::path::PathBuf;

use studiofs_bench::{
    BenchmarkConfig, CacheMode, ConfigError, ExecutionMode, FileLayout, RunMode, WorkloadPreset,
    WorkloadSize,
};

#[test]
fn default_config_uses_documented_benchmark_contract() {
    let config = BenchmarkConfig::for_target(PathBuf::from("E:/bench-target"));

    assert_eq!(config.target_path, PathBuf::from("E:/bench-target"));
    assert_eq!(config.workload_size.gigabytes(), 4);
    assert_eq!(config.workload_size.megabytes(), Some(4_000));
    assert_eq!(config.run_mode, RunMode::LocalFilesystem);
    assert_eq!(config.file_layout, FileLayout::SingleFile);
    assert_eq!(config.cache_mode, CacheMode::Enabled);
    assert!(!config.keep_files);
    assert!(config.save_report);
    assert_eq!(config.execution_mode, ExecutionMode::RunOnce);
    assert_eq!(BenchmarkConfig::THROUGHPUT_UNIT, "MB/s");
    assert_eq!(config.validate(), Ok(()));
}

#[test]
fn workload_size_overflows_decimal_unit_conversions() {
    let workload_size = WorkloadSize::CustomGb(u64::MAX);

    assert_eq!(workload_size.megabytes(), None);
    assert_eq!(workload_size.bytes(), None);
}

#[test]
fn validate_rejects_workload_that_overflows_bytes() {
    let mut config = BenchmarkConfig::for_target(PathBuf::from("E:/bench-target"));
    config.workload_size = WorkloadSize::CustomGb(u64::MAX / 1_000);

    assert_eq!(config.validate(), Err(ConfigError::WorkloadOverflow));
}

#[test]
fn validate_rejects_empty_target_path() {
    let config = BenchmarkConfig::for_target(PathBuf::new());

    assert_eq!(config.validate(), Err(ConfigError::EmptyTargetPath));
}

#[test]
fn validate_rejects_zero_workload() {
    let mut config = BenchmarkConfig::for_target(PathBuf::from("E:/bench-target"));
    config.workload_size = WorkloadSize::CustomGb(0);

    assert_eq!(config.validate(), Err(ConfigError::ZeroWorkload));
}

#[test]
fn validate_rejects_zero_file_size() {
    let mut config = BenchmarkConfig::for_target(PathBuf::from("E:/bench-target"));
    config.file_layout = FileLayout::FixedFileSizeMb(0);

    assert_eq!(config.validate(), Err(ConfigError::ZeroFileSize));
}

#[test]
fn validate_rejects_fixed_file_layout_larger_than_workload() {
    let mut config = BenchmarkConfig::for_target(PathBuf::from("E:/bench-target"));
    config.workload_size = WorkloadSize::Preset(WorkloadPreset::OneGb);
    config.file_layout = FileLayout::FixedFileSizeMb(2_000);

    let error = config.validate().unwrap_err();

    assert_eq!(
        error.to_string(),
        "file layout size must not exceed total workload size"
    );
}

#[test]
fn config_serializes_report_ready_values() {
    let mut config = BenchmarkConfig::for_target(PathBuf::from("E:/bench-target"));
    config.workload_size = WorkloadSize::CustomGb(16);
    config.run_mode = RunMode::MountedFilesystem;
    config.cache_mode = CacheMode::Disabled;
    config.keep_files = true;
    config.save_report = false;
    config.execution_mode = ExecutionMode::Continuous;

    let value = serde_json::to_value(&config).unwrap();

    assert_eq!(value["workload_size"]["custom_gb"], 16);
    assert_eq!(value["run_mode"], "mounted_filesystem");
    assert_eq!(value["file_layout"], "single_file");
    assert_eq!(value["cache_mode"], "disabled");
    assert_eq!(value["keep_files"], true);
    assert_eq!(value["save_report"], false);
    assert_eq!(value["execution_mode"], "continuous");
}
