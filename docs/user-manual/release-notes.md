# Release Notes

## Unreleased

### Fixed

- Cache-disabled runs bypass the OS file-data cache with Windows
  `FILE_FLAG_NO_BUFFERING`, Linux `O_DIRECT`, or checked macOS `F_NOCACHE`.
  Unsupported targets fail instead of silently using cached I/O. Linux requires
  a filesystem that reports `STATX_DIOALIGN` on Linux 6.1 or newer.
- Uncached transfers handle small files and unaligned tails while preserving
  requested file sizes and logical byte counts.
- Batch fsync syncs all written files after the write sequence and includes
  their sync time in the final write sample and write pass average.

## 0.1.0 - First Working Version

First working `studiofs-bench` release.

### Added

- Terminal UI for configuring and running benchmark passes.
- Scripted mode for repeatable command-line runs.
- Sequential streaming write/read engine for sustained disk or mount-point
  throughput.
- Workload layouts for one file, 100 varied files, and scripted fixed-size
  files.
- Run modes for local filesystem paths and mounted filesystem paths.
- Benchmark modes for read/write, write-only, and write-once/read-loop runs.
- Best-effort cache-reduced mode for Windows, macOS, and Linux.
- Batch fsync by default, with per-file fsync available through `Batch fsync`
  off or `--no-batch-fsync`.
- Safe cleanup by default, with `Keep files` / `--keep-files` for inspection.
- Optional JSON and CSV reports with pass metrics and raw throughput samples.
- Reference validation contract for comparing saved runs on a fixed test bench.

### Notes

- Results are intended for comparisons on the same machine and target path.
- The benchmark does not claim to represent every application workload.
- Scripted mode supports one-shot runs; use the interactive UI for continuous
  runs.
