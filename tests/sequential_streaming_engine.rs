//! Sequential streaming engine tests.

use std::fs;

use studiofs_bench::{StreamingIoEngine, StreamingIoPhase};

#[test]
fn engine_writes_reads_and_reports_sequential_samples() {
    let dir = std::env::temp_dir().join(format!("studiofs-bench-sfs-570-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("stream.bin");

    let engine = StreamingIoEngine::with_block_size(4).unwrap();
    let mut samples = Vec::new();

    let report = engine
        .run(&path, 10, |sample| samples.push(sample), || false)
        .unwrap();

    assert_eq!(report.bytes_written, 10);
    assert_eq!(report.bytes_read, 10);
    assert_eq!(fs::metadata(&path).unwrap().len(), 10);
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

    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn engine_stops_between_blocks_without_starting_read_pass() {
    let dir = std::env::temp_dir().join(format!(
        "studiofs-bench-sfs-570-stop-{}",
        std::process::id()
    ));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("stream.bin");

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

    fs::remove_dir_all(&dir).unwrap();
}
