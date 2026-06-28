# studiofs-bench

Terminal benchmark binary for StudioFS benchmark experiments.

## Usage

Interactive terminal UI:

```bash
studiofs-bench
```

Scripted smoke run:

```bash
studiofs-bench --scripted --target ./bench-target --workload-bytes 8 --mode write-only --layout single-file --cache enabled --save-report ./bench-report
```

Scripted options:

- `--target <path>`
- `--workload-gb <n>` or `--workload-bytes <n>`
- `--run-mode local|mounted`
- `--mode read-write|write-only|write-once-read-loop`
- `--layout single-file|hundred-files-plus-minus-five`
- `--file-size-mb <n>`
- `--cache enabled|disabled`
- `--execution run-once`
- `--keep-files`
- `--save-report <path-prefix>` writes `<path-prefix>.json` and `<path-prefix>.csv`

## Default benchmark configuration

- Target path: required.
- Workload size: 4 GB, using decimal storage units.
- Run mode: local filesystem.
- File layout: single file.
- Cache mode: enabled.
- Keep files: disabled.
- Save report: enabled.
- Execution: run once.
- Throughput unit: MB/s.

## Cache control

- `enabled`: normal platform file I/O.
- `disabled`: best-effort per-file cache-reduced I/O.

Platform methods recorded in report metadata:

- Windows: write-through file flag.
- macOS: `F_NOCACHE`.
- Linux: `posix_fadvise(..., POSIX_FADV_DONTNEED)`.
- Other targets: unavailable best-effort marker.

Manual platform check:

```bash
cargo test --test config_model --test sequential_streaming_engine
```

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

Fast test suite:

```bash
cargo test --workspace --all-targets
```
