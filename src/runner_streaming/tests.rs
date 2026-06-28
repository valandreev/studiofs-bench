#[allow(clippy::wildcard_imports)]
use super::*;
use crate::FileLayout;

static SYNC_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn runner_batches_fsync_across_write_file_sequence_by_default() {
    let _guard = SYNC_TEST_LOCK.lock().unwrap();
    let target = TestDir::new("studiofs-bench-batch-fsync-default");
    let mut config = BenchmarkConfig::for_target(target.path.clone());
    config.test_mode = DiskTestMode::WriteOnly;
    config.file_layout = FileLayout::HundredFilesPlusMinusFive;
    let workload =
        Workload::create_for_bytes(&target.path, 100, FileLayout::HundredFilesPlusMinusFive)
            .unwrap();

    reset_sync_call_count();
    BenchmarkRunner::with_block_size(200)
        .unwrap()
        .run_workload(workload, &config, |_| {}, || false)
        .unwrap();

    assert_eq!(sync_call_count(), 1);
}

#[test]
fn runner_can_sync_each_written_file_for_legacy_mode() {
    let _guard = SYNC_TEST_LOCK.lock().unwrap();
    let target = TestDir::new("studiofs-bench-batch-fsync-off");
    let mut config = BenchmarkConfig::for_target(target.path.clone());
    config.test_mode = DiskTestMode::WriteOnly;
    config.file_layout = FileLayout::HundredFilesPlusMinusFive;
    config.batch_fsync = false;
    let workload =
        Workload::create_for_bytes(&target.path, 100, FileLayout::HundredFilesPlusMinusFive)
            .unwrap();

    reset_sync_call_count();
    BenchmarkRunner::with_block_size(200)
        .unwrap()
        .run_workload(workload, &config, |_| {}, || false)
        .unwrap();

    assert_eq!(sync_call_count(), 100);
}

#[test]
fn streaming_io_error_exposes_io_source() {
    let error =
        StreamingIoError::from(std::io::Error::new(std::io::ErrorKind::NotFound, "missing"));

    assert!(std::error::Error::source(&error).is_some());
}

#[test]
fn sample_capacity_caps_preallocation() {
    let files = [WorkloadFile {
        path: "huge.bin".into(),
        bytes: u64::MAX,
    }];

    assert_eq!(sample_capacity(&files, 1), 16_384);
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
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}
