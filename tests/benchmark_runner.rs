//! Benchmark runner mode tests.

use std::cell::Cell;
use std::path::{Path, PathBuf};

use studiofs_bench::{
    BenchmarkConfig, BenchmarkRunner, DiskTestMode, ExecutionMode, FileLayout, StreamingIoPhase,
    Workload,
};

#[test]
fn runner_runs_read_write_once_and_cleans_files_by_default() {
    let target = TestDir::new("studiofs-bench-sfs-573-read-write");
    let mut config = BenchmarkConfig::for_target(target.path().to_owned());
    config.test_mode = DiskTestMode::ReadWrite;
    let workload = Workload::create_for_bytes(target.path(), 6, FileLayout::SingleFile).unwrap();
    let run_dir = workload.run_dir().to_owned();
    let mut phases = Vec::new();

    let report = BenchmarkRunner::with_block_size(3)
        .unwrap()
        .run_workload(
            workload,
            &config,
            |sample| phases.push(sample.phase),
            || false,
        )
        .unwrap();

    assert_eq!(
        phases,
        vec![
            StreamingIoPhase::Write,
            StreamingIoPhase::Write,
            StreamingIoPhase::Read,
            StreamingIoPhase::Read
        ]
    );
    assert!(!run_dir.exists());
    assert!(!report.files_kept);
    assert!(report.cleanup_error.is_none());
}

#[test]
fn runner_keeps_files_for_write_only_when_requested() {
    let target = TestDir::new("studiofs-bench-sfs-573-write-only");
    let mut config = BenchmarkConfig::for_target(target.path().to_owned());
    config.test_mode = DiskTestMode::WriteOnly;
    config.keep_files = true;
    let workload = Workload::create_for_bytes(target.path(), 4, FileLayout::SingleFile).unwrap();
    let run_dir = workload.run_dir().to_owned();
    let mut phases = Vec::new();

    let report = BenchmarkRunner::with_block_size(2)
        .unwrap()
        .run_workload(
            workload,
            &config,
            |sample| phases.push(sample.phase),
            || false,
        )
        .unwrap();

    assert_eq!(
        phases,
        vec![StreamingIoPhase::Write, StreamingIoPhase::Write]
    );
    assert!(run_dir.exists());
    assert_eq!(report.run_dir, run_dir);
    assert!(report.files_kept);
}

#[test]
fn runner_write_once_read_loop_does_not_rewrite_between_read_passes() {
    let target = TestDir::new("studiofs-bench-sfs-573-read-loop");
    let mut config = BenchmarkConfig::for_target(target.path().to_owned());
    config.test_mode = DiskTestMode::WriteOnceReadLoop;
    config.execution_mode = ExecutionMode::Continuous;
    let workload =
        Workload::create_for_bytes(target.path(), 100, FileLayout::HundredFilesPlusMinusFive)
            .unwrap();
    let sample_count = Cell::new(0);
    let write_count = Cell::new(0);
    let read_count = Cell::new(0);

    BenchmarkRunner::with_block_size(200)
        .unwrap()
        .run_workload(
            workload,
            &config,
            |sample| {
                sample_count.set(sample_count.get() + 1);
                match sample.phase {
                    StreamingIoPhase::Write => write_count.set(write_count.get() + 1),
                    StreamingIoPhase::Read => read_count.set(read_count.get() + 1),
                }
            },
            || sample_count.get() >= 201,
        )
        .unwrap();

    assert_eq!(write_count.get(), 100);
    assert!(read_count.get() > 100);
}

#[test]
fn runner_stops_between_files_without_truncating_next_file() {
    let target = TestDir::new("studiofs-bench-sfs-573-stop-between-files");
    let mut config = BenchmarkConfig::for_target(target.path().to_owned());
    config.test_mode = DiskTestMode::WriteOnly;
    config.keep_files = true;
    let workload =
        Workload::create_for_bytes(target.path(), 100, FileLayout::HundredFilesPlusMinusFive)
            .unwrap();
    let second_file = workload.files()[1].path.clone();
    let sample_count = Cell::new(0);

    let report = BenchmarkRunner::with_block_size(1)
        .unwrap()
        .run_workload(
            workload,
            &config,
            |_| sample_count.set(sample_count.get() + 1),
            || sample_count.get() >= 1,
        )
        .unwrap();

    assert!(report.stopped);
    assert_eq!(std::fs::metadata(second_file).unwrap().len(), 1);
}

struct TestDir {
    path: PathBuf,
}

impl TestDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}
