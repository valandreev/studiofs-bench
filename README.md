# studiofs-bench

`studiofs-bench` is a terminal benchmark for measuring sustained sequential
throughput on a disk path or mounted filesystem path. It is aimed at
post-production style workloads where large media files are written and read in
dense streams.

It does not try to model every application workload. Use it to compare repeatable
runs on the same fixed test bench.

## Install and Build

Requirements:

- Windows 11, macOS, or Linux.
- Rust 1.95 or newer.
- A terminal large enough to show the full-screen UI.
- Enough free space in the target path for the selected workload.

Build the release binary:

```bash
cargo build --release
```

Run from the repository:

```bash
target\release\studiofs-bench.exe
```

On macOS or Linux:

```bash
./target/release/studiofs-bench
```

## Quick Start

Interactive terminal UI:

```bash
studiofs-bench
```

Scripted Windows example:

```powershell
studiofs-bench --scripted --target E:\bench-target --workload-gb 4 --mode read-write --layout single-file --cache enabled --save-report
```

Scripted macOS example:

```bash
studiofs-bench --scripted --target /Volumes/BenchTarget --workload-gb 4 --mode read-write --layout single-file --cache enabled --save-report
```

Scripted Linux example:

```bash
studiofs-bench --scripted --target /mnt/bench-target --workload-gb 4 --mode read-write --layout single-file --cache enabled --save-report
```

For a tiny smoke run:

```bash
studiofs-bench --scripted --target ./bench-target --workload-bytes 8 --mode write-only --layout single-file --cache enabled --save-report
```

## What It Measures

The benchmark creates a temporary run directory in the target path, writes the
selected workload, optionally reads it back, and reports throughput in decimal
`MB/s`.

The I/O engine is one dense sequential streaming engine. It uses an internal
8 MB block size for normal runs, fills write buffers with deterministic non-zero
data, and samples throughput after completed blocks. The final write block is
synced before the write pass completes.

## Interactive Controls

- Up and Down: select a setting.
- Left and Right: change the selected setting.
- Type text: edit the target path when `Target path` is selected.
- Backspace: delete one character from the target path.
- Enter: start the benchmark, or request stop while running.
- Esc: stop while running, or exit when idle.

Settings are locked while a run is active.

## Settings

- `Target path`: directory where benchmark files are created. The default is
  the current directory.
- `Workload size`: total data size. Interactive presets are 1 GB, 4 GB, 16 GB,
  and 64 GB. Scripted mode also accepts `--workload-gb <n>` or
  `--workload-bytes <n>`.
- `Mode`: `read/write` writes the workload and reads it once. `write only`
  writes without a read pass. `write once, read loop` writes once and repeats
  read passes until stopped.
- `Layout`: `single file` stores the workload in one file. `100 files +/-5%`
  splits it into 100 deterministic files with slight size variance. Scripted
  mode also accepts `--file-size-mb <n>` for fixed-size files.
- `Cache mode`: `enabled` uses normal file I/O. `disabled` requests best-effort
  cache-reduced I/O for the platform.
- `Execution mode`: `run once` completes one configured run. `continuous`
  repeats configured phases until stopped. Scripted mode only supports
  `run-once`.
- `Keep files`: keep the generated run directory after completion.
- `Save report`: write JSON and CSV reports in the launch directory.

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
- `--save-report`

## File Layout and Cleanup

Each run creates one directory named like:

```text
studiofs-bench-run-<process-id>-<timestamp>-<counter>
```

Workload files are named:

```text
studiofs-bench-workload-000.bin
studiofs-bench-workload-001.bin
...
```

By default, `studiofs-bench` removes only its own run directory after the run.
It does not delete other files in the target path. Enable `Keep files` or pass
`--keep-files` when you need to inspect the generated workload files.

If cleanup fails after a benchmark has completed, the UI and report keep the
completed pass data and show the cleanup error.

## Cache Behavior

`enabled` uses normal platform file I/O.

`disabled` is best-effort and platform-specific:

- Windows: opens files with the write-through file flag.
- macOS: sets `F_NOCACHE` on the file descriptor.
- Linux: calls `posix_fadvise(..., POSIX_FADV_DONTNEED)` after file I/O.
- Other targets: records that a best-effort method is unavailable.

Cache controls are not identical across operating systems. Compare runs only
when the cache mode, target path, and machine setup are the same.

## Reports

Reports are saved only when `Save report` is enabled or `--save-report` is
passed. Files are written in the launch directory and named like:

```text
studiofs-bench-report-<unix-seconds>-<counter>.json
studiofs-bench-report-<unix-seconds>-<counter>.csv
```

The JSON report contains:

- `run.workload_bytes`: explicit scripted byte size when used.
- `run.run_dir`: generated run directory.
- `run.files_kept`: whether generated files were kept.
- `run.stopped`: whether the run was stopped before normal completion.
- `run.cleanup_error`: cleanup error text, if cleanup failed.
- `platform.os` and `platform.arch`.
- `config`: selected settings.
- `cache_method`: selected platform cache method.
- `passes`: completed write/read pass summaries.

Each pass includes the phase, pass number, processed bytes, stop flag, summary
metrics, and throughput samples.

The CSV report contains one sample per row:

```text
phase,pass_number,sample_index,mb_per_second
write,1,0,123.4
read,1,0,118.9
```

## Reading Results

The UI and reports show:

- `Avg`: average sample throughput for the pass.
- `Stable`: average excluding samples that dropped below the previous sample.
- `Min`: lowest sample throughput.
- `Drops`: count of samples lower than the previous sample.

Use the raw JSON and CSV files when you need to compare pass averages or inspect
stalls.

## Validation

Compare runs only on the same fixed test bench:

- Same machine, operating system, power mode, storage target, filesystem, and
  mount point.
- Same target path, workload size, file layout, cache mode, run mode, and
  execution mode.
- Multiple saved passes before calculating averages.

For validation against a reference disk benchmark, use the contract in
`docs/contracts/reference-validation.md`. Do not compare a local disk run with a
mounted filesystem run unless that is the specific experiment.

## Development

Focused checks used during development:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo check --workspace
```

Full test suite, for maintainers to run when needed:

```bash
cargo test --workspace --all-targets
```
