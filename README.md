# studiofs-bench

Terminal benchmark binary for StudioFS benchmark experiments.

## Development

Build:

```bash
cargo build --release
```

Checks:

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -W missing-docs
cargo clippy --workspace --all-targets -- -D warnings
cargo check --workspace
```

Workspace lint policy is inherited from the root `[workspace.lints]` tables.
Member crates opt in with `[lints] workspace = true`; keep new shared lints
low-noise and stage broad hygiene migrations separately.
