#[allow(clippy::wildcard_imports)]
use super::*;

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
