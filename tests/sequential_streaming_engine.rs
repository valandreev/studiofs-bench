//! Sequential streaming engine tests.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use studiofs_bench::{CacheControlMethod, CacheMode, StreamingIoEngine, StreamingIoPhase};

#[test]
fn engine_writes_reads_and_reports_sequential_samples() {
    let dir = TestDir::new("studiofs-bench-sfs-570");
    let path = dir.path().join("stream.bin");

    let engine = StreamingIoEngine::with_block_size(4).unwrap();
    let mut samples = Vec::new();

    let report = engine
        .run(&path, 10, |sample| samples.push(sample), || false)
        .unwrap();

    assert_eq!(report.bytes_written, 10);
    assert_eq!(report.bytes_read, 10);
    assert_eq!(fs::metadata(&path).unwrap().len(), 10);
    assert_eq!(fs::read(&path).unwrap(), vec![0, 0, 0, 0, 4, 0, 0, 0, 8, 0]);
    assert_eq!(
        samples
            .iter()
            .map(|sample| (sample.phase, sample.offset, sample.bytes_processed))
            .collect::<Vec<_>>(),
        vec![
            (StreamingIoPhase::Write, 0, 4),
            (StreamingIoPhase::Write, 4, 8),
            (StreamingIoPhase::Write, 8, 10),
            (StreamingIoPhase::Read, 0, 4),
            (StreamingIoPhase::Read, 4, 8),
            (StreamingIoPhase::Read, 8, 10),
        ]
    );
}

#[test]
fn engine_stops_between_blocks_without_starting_read_pass() {
    let dir = TestDir::new("studiofs-bench-sfs-570-stop");
    let path = dir.path().join("stream.bin");

    let engine = StreamingIoEngine::with_block_size(4).unwrap();
    let mut samples = Vec::new();
    let mut checks = 0;

    let report = engine
        .run(
            &path,
            10,
            |sample| samples.push(sample),
            || {
                checks += 1;
                checks > 1
            },
        )
        .unwrap();

    assert!(report.stopped);
    assert_eq!(report.bytes_written, 4);
    assert_eq!(report.bytes_read, 0);
    assert_eq!(
        samples
            .iter()
            .map(|sample| sample.phase)
            .collect::<Vec<_>>(),
        vec![StreamingIoPhase::Write]
    );
}

#[test]
fn engine_throughput_samples_exclude_callback_delay() {
    let dir = TestDir::new("studiofs-bench-sfs-570-callback-delay");
    let path = dir.path().join("stream.bin");

    let engine = StreamingIoEngine::with_block_size(1).unwrap();
    let mut samples = Vec::new();

    engine
        .run(
            &path,
            2,
            |sample| {
                samples.push(sample);
                if samples.len() == 1 {
                    std::thread::sleep(Duration::from_millis(100));
                }
            },
            || false,
        )
        .unwrap();

    assert!(
        samples[1].mb_per_second > 0.0001,
        "callback delay contaminated throughput: {} MB/s",
        samples[1].mb_per_second
    );
}

#[test]
fn engine_records_disabled_cache_method_in_report_metadata() {
    let dir = TestDir::new("studiofs-bench-sfs-571-cache-disabled");
    let path = dir.path().join("stream.bin");

    let engine = StreamingIoEngine::with_block_size(4).unwrap();

    let report = engine
        .run_with_cache_mode(&path, 8, CacheMode::Disabled, |_| {}, || false)
        .unwrap();

    assert_eq!(
        (report.metadata.cache_mode, report.metadata.cache_method),
        (CacheMode::Disabled, expected_disabled_cache_method())
    );
}

#[test]
fn engine_stamps_offsets_inside_large_chunks() {
    let dir = TestDir::new("studiofs-bench-sfs-571-sub-block-stamps");
    let path = dir.path().join("stream.bin");

    let engine = StreamingIoEngine::with_block_size(8192).unwrap();

    engine.run(&path, 8192, |_| {}, || false).unwrap();

    let bytes = fs::read(&path).unwrap();
    assert_eq!(
        (&bytes[..8], &bytes[4096..4104]),
        (&0_u64.to_le_bytes()[..], &4096_u64.to_le_bytes()[..])
    );
}

#[cfg(windows)]
fn expected_disabled_cache_method() -> CacheControlMethod {
    CacheControlMethod::WriteThrough
}

#[cfg(target_os = "macos")]
fn expected_disabled_cache_method() -> CacheControlMethod {
    CacheControlMethod::FcntlNoCache
}

#[cfg(target_os = "linux")]
fn expected_disabled_cache_method() -> CacheControlMethod {
    CacheControlMethod::PosixFadviseDontNeed
}

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
fn expected_disabled_cache_method() -> CacheControlMethod {
    CacheControlMethod::BestEffortUnavailable
}

struct TestDir {
    path: PathBuf,
}

impl TestDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}
