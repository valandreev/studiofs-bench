#[allow(clippy::wildcard_imports)]
use super::*;

#[test]
fn streaming_io_error_exposes_io_source() {
    let error =
        StreamingIoError::from(std::io::Error::new(std::io::ErrorKind::NotFound, "missing"));

    assert!(std::error::Error::source(&error).is_some());
}
