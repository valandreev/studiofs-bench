#[allow(clippy::wildcard_imports)]
use super::*;

#[test]
fn chunk_len_keeps_large_remaining_sizes_in_u64_until_after_min() {
    assert_eq!(chunk_len(8 * 1024 * 1024, 1_u64 << 32), 8 * 1024 * 1024);
}

#[test]
fn fill_benchmark_buffer_uses_non_zero_deterministic_bytes() {
    let mut buffer = [0_u8; 8];

    fill_benchmark_buffer(&mut buffer);

    assert_eq!(buffer, [117, 205, 37, 75, 132, 226, 234, 242]);
}

#[test]
fn hundred_file_sizes_keep_exact_total_for_large_workloads() {
    let sizes = hundred_file_sizes(u64::MAX).unwrap();

    assert_eq!(sizes.iter().sum::<u64>(), u64::MAX);
}

#[test]
fn hundred_file_sizes_keep_exact_total_near_minimum_size() {
    let sizes = hundred_file_sizes(101).unwrap();

    assert_eq!(sizes.iter().sum::<u64>(), 101);
    assert!(sizes.iter().all(|size| *size > 0));
}

#[test]
fn fixed_file_sizes_rejects_too_many_files() {
    let error = fixed_file_sizes(100_001 * DECIMAL_MB, 1).unwrap_err();

    assert_eq!(error.to_string(), "workload size is too large");
}

#[test]
fn cleanup_ignores_missing_run_dir() {
    let target = std::env::temp_dir().join(format!(
        "studiofs-bench-sfs-572-cleanup-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&target);
    std::fs::create_dir_all(&target).unwrap();

    let workload = Workload::create_for_bytes(&target, 1, FileLayout::SingleFile).unwrap();
    let run_dir = workload.run_dir().to_owned();
    std::fs::remove_dir_all(&run_dir).unwrap();

    assert!(workload.cleanup().is_ok());
    let _ = std::fs::remove_dir_all(&target);
}

#[test]
fn cleanup_error_includes_run_dir_path() {
    let run_dir = std::env::temp_dir().join(format!(
        "studiofs-bench-sfs-579-cleanup-file-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&run_dir);
    File::create(&run_dir).unwrap();
    let workload = Workload {
        run_dir: run_dir.clone(),
        files: Vec::new(),
    };

    let error = workload.cleanup().unwrap_err();

    assert!(
        error.to_string().contains(&run_dir.display().to_string()),
        "cleanup path not reported in error: {error}"
    );
    let _ = std::fs::remove_file(&run_dir);
}

#[test]
fn write_workload_files_removes_run_dir_when_file_write_fails() {
    let run_dir = std::env::temp_dir().join(format!(
        "studiofs-bench-sfs-572-partial-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&run_dir);
    std::fs::create_dir_all(&run_dir).unwrap();

    let error = write_workload_files(&run_dir, vec![1, 2], |path, bytes| {
        if bytes == 2 {
            return Err(std::io::Error::other("write failed").into());
        }
        File::create(path)?;
        Ok(())
    })
    .unwrap_err();

    assert_eq!(error.to_string(), "write failed");
    assert!(!run_dir.exists());
}

#[test]
fn write_workload_file_uses_supplied_buffer() {
    let path = std::env::temp_dir().join(format!(
        "studiofs-bench-sfs-572-buffer-{}.bin",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);

    write_workload_file(&path, 5, &[1, 2, 3]).unwrap();

    assert_eq!(std::fs::read(&path).unwrap(), vec![1, 2, 3, 1, 2]);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn terminal_ui_updates_unknown_total_bytes_from_later_samples() {
    let mut ui = TerminalUi::default();
    ui.config.workload_size = WorkloadSize::CustomGb(u64::MAX);

    ui.observe_sample(StreamingIoSample {
        phase: StreamingIoPhase::Write,
        pass_number: 1,
        timestamp: SystemTime::UNIX_EPOCH,
        offset: 0,
        bytes_processed: 0,
        mb_per_second: 0.0,
    });
    ui.observe_sample(StreamingIoSample {
        phase: StreamingIoPhase::Write,
        pass_number: 1,
        timestamp: SystemTime::UNIX_EPOCH,
        offset: 0,
        bytes_processed: 10 * DECIMAL_MB,
        mb_per_second: 100.0,
    });

    assert_eq!(ui.progress.unwrap().total_bytes, 10 * DECIMAL_MB);
}

#[test]
fn chart_points_handles_single_column_width() {
    assert_eq!(chart_points(&[1.0, 2.0], 1).as_ref(), &[1.0]);
}
