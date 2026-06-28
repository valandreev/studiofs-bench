//! Binary smoke tests.

use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_TEST_DIR: AtomicU64 = AtomicU64::new(0);

#[test]
fn binary_prints_name() {
    let output = Command::new(env!("CARGO_BIN_EXE_studiofs-bench"))
        .output()
        .expect("run studiofs-bench");

    assert!(output.status.success());
    assert_eq!(output.stdout, b"studiofs-bench\n");
    assert!(output.stderr.is_empty());
}

#[test]
fn scripted_mode_runs_tiny_benchmark_and_saves_reports() {
    let dir = TestDir::new("studiofs-bench-sfs-578-scripted");
    let report = dir.path().join("reports").join("report");

    let output = Command::new(env!("CARGO_BIN_EXE_studiofs-bench"))
        .arg("--target")
        .arg(dir.path())
        .arg("--scripted")
        .arg("--workload-bytes")
        .arg("8")
        .arg("--mode")
        .arg("write-only")
        .arg("--layout")
        .arg("single-file")
        .arg("--cache")
        .arg("enabled")
        .arg("--save-report")
        .arg(&report)
        .output()
        .expect("run scripted studiofs-bench");

    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("Done - 1 passes"));
    assert!(output.stderr.is_empty());
    assert!(report.with_extension("json").exists());
    assert!(report.with_extension("csv").exists());
}

struct TestDir {
    path: std::path::PathBuf,
}

impl TestDir {
    fn new(name: &str) -> Self {
        let id = NEXT_TEST_DIR.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("{name}-{}-{id}", std::process::id()));
        std::fs::remove_dir_all(&path).ok();
        std::fs::create_dir_all(&path).unwrap();
        Self { path }
    }

    fn path(&self) -> &std::path::Path {
        &self.path
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.path).ok();
    }
}
