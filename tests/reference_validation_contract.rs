//! Reference validation contract tests.

#[test]
fn reference_validation_contract_documents_fixed_bench_and_artifacts() {
    let contract_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("docs")
        .join("contracts")
        .join("reference-validation.md");
    let contract =
        std::fs::read_to_string(contract_path).expect("reference validation contract should exist");
    let required = [
        "Fixed test bench",
        "Variance threshold",
        "Multiple passes",
        "Windows validation",
        "macOS validation",
        "Linux validation",
        "Artifact manifest",
        "at least 3 passes",
        "10%",
        "same machine and the same target path",
        "reference-report-pass-01",
        "studiofs-bench-report-pass-01.json",
        "studiofs-bench-report-pass-01.csv",
        "reference report",
        "studiofs-bench report",
        "average write MB/s",
        "average read MB/s",
        "storage target instability",
    ];

    for term in required {
        assert!(
            contract.contains(term),
            "reference validation contract is missing `{term}`"
        );
    }
}
