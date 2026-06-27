use std::process::Command;

#[test]
fn binary_prints_name() {
    let output = Command::new(env!("CARGO_BIN_EXE_studiofs-bench"))
        .output()
        .expect("run studiofs-bench");

    assert!(output.status.success());
    assert_eq!(output.stdout, b"studiofs-bench\n");
    assert!(output.stderr.is_empty());
}
