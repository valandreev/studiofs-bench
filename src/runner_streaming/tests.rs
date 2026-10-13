#[allow(clippy::wildcard_imports)]
use super::*;
use crate::FileLayout;

static SYNC_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn runner_batch_fsync_syncs_each_written_file_after_all_writes() {
    let _guard = SYNC_TEST_LOCK.lock().unwrap();
    let target = TestDir::new("studiofs-bench-batch-fsync-default");
    let mut config = BenchmarkConfig::for_target(target.path.clone());
    config.test_mode = DiskTestMode::WriteOnly;
    config.file_layout = FileLayout::HundredFilesPlusMinusFive;
    let workload =
        Workload::create_for_bytes(&target.path, 100, FileLayout::HundredFilesPlusMinusFive)
            .unwrap();
    let expected_paths = workload
        .files()
        .iter()
        .map(|file| file.path.clone())
        .collect::<Vec<_>>();
    let mut expected_file_lengths = workload
        .files()
        .iter()
        .map(|file| (file.path.clone(), file.bytes))
        .collect::<Vec<_>>();
    expected_file_lengths.sort_by(|left, right| left.0.cmp(&right.0));
    let observation_directory = expected_paths[0].parent().unwrap().to_owned();

    reset_sync_call_count();
    set_sync_observation_directory(Some(observation_directory));
    let result = BenchmarkRunner::with_block_size(200).unwrap().run_workload(
        workload,
        &config,
        |_| {},
        || false,
    );
    set_sync_observation_directory(None);
    let report = result.unwrap();

    let sync_paths = SYNC_PATH_OBSERVATIONS.lock().unwrap().clone();
    let file_length_snapshots = SYNC_FILE_LENGTH_OBSERVATIONS.lock().unwrap().clone();
    assert_eq!(sync_call_count(), expected_paths.len());
    assert_eq!(sync_paths, expected_paths);
    assert_eq!(file_length_snapshots.len(), expected_paths.len());
    assert!(
        file_length_snapshots
            .iter()
            .all(|snapshot| snapshot == &expected_file_lengths)
    );
    let mut unique_paths = sync_paths.clone();
    unique_paths.sort();
    unique_paths.dedup();
    assert_eq!(unique_paths.len(), sync_paths.len());
    assert_eq!(report.passes[0].bytes_processed, 100);
}

#[test]
fn runner_includes_each_batch_sync_in_final_sample_and_average() {
    let _guard = SYNC_TEST_LOCK.lock().unwrap();
    let target = TestDir::new("studiofs-bench-batch-fsync-timing");
    let mut config = BenchmarkConfig::for_target(target.path.clone());
    config.test_mode = DiskTestMode::WriteOnly;
    config.file_layout = FileLayout::HundredFilesPlusMinusFive;
    let workload =
        Workload::create_for_bytes(&target.path, 100, FileLayout::HundredFilesPlusMinusFive)
            .unwrap();
    let file_count = workload.files().len();
    let sync_time = Duration::from_millis(250);
    let mut samples = Vec::new();

    reset_sync_call_count();
    set_sync_elapsed_override(Some(sync_time));
    let result = BenchmarkRunner::with_block_size(200).unwrap().run_workload(
        workload,
        &config,
        |sample| samples.push(sample),
        || false,
    );
    set_sync_elapsed_override(None);
    let report = result.unwrap();

    let final_sample = samples.last().unwrap();
    let expected_minimum_sync_time = sync_time * u32::try_from(file_count).unwrap();
    assert_eq!(sync_call_count(), file_count);
    assert_eq!(final_sample.bytes_processed, 100);
    assert!(final_sample.elapsed_io >= expected_minimum_sync_time);
    assert_eq!(report.passes[0].throughput_samples.len(), samples.len());
    assert_eq!(
        report.passes[0].throughput_samples.last(),
        Some(&final_sample.mb_per_second)
    );
    assert_eq!(
        report.passes[0].metrics.average_mb_per_second,
        sample_throughput(100, final_sample.elapsed_io)
    );
}

#[test]
fn runner_batch_fsync_syncs_the_touched_file_when_stopped() {
    let _guard = SYNC_TEST_LOCK.lock().unwrap();
    let target = TestDir::new("studiofs-bench-batch-fsync-stop");
    let mut config = BenchmarkConfig::for_target(target.path.clone());
    config.test_mode = DiskTestMode::WriteOnly;
    config.file_layout = FileLayout::HundredFilesPlusMinusFive;
    let workload =
        Workload::create_for_bytes(&target.path, 100, FileLayout::HundredFilesPlusMinusFive)
            .unwrap();
    let sample_count = std::cell::Cell::new(0);

    reset_sync_call_count();
    let report = BenchmarkRunner::with_block_size(1)
        .unwrap()
        .run_workload(
            workload,
            &config,
            |_| sample_count.set(sample_count.get() + 1),
            || sample_count.get() > 0,
        )
        .unwrap();

    assert!(report.stopped);
    assert_eq!(report.passes[0].bytes_processed, 1);
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
