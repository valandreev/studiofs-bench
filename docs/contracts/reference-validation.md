# Reference Validation Contract

SFS-580 validates `studiofs-bench` against a selected reference disk benchmark on
the same machine and the same target path. The reference tool is intentionally
unnamed here: product-specific reference names must stay out of UI, README,
user docs, and implementation.

## Fixed test bench

Record these fields before each validation run:

- Operating system and version.
- CPU model, memory size, and power mode.
- Storage target model, interface, filesystem, and mount point.
- Target path used by both tools.
- Workload size: 4 GB.
- File layout: single file.
- Cache mode: enabled or disabled, matching both tools as closely as possible.
- Batch fsync: enabled or disabled.
- Run mode: local filesystem or mounted filesystem.
- Execution: run once per pass.

Do not compare results across different machines, disks, workload sizes, file
layouts, cache modes, batch fsync settings, or mount points.

## Multiple passes

Run at least 3 passes for the reference benchmark and at least 3 passes for
`studiofs-bench`. Save every raw report before calculating averages.

For each tool and platform, record:

- average write MB/s across completed passes.
- average read MB/s across completed passes.
- visible stability behavior, including drops, stalls, and obvious outliers.

## Variance threshold

Validation passes when both average write MB/s and average read MB/s are within
10% of the selected reference benchmark on the same fixed bench.

If the storage target instability is visible across repeated reference passes,
record the instability and do not claim validation success from one lucky run.

## Platform Checklist

- Windows validation: pending until raw reports are attached.
- macOS validation: pending until raw reports are attached.
- Linux validation: pending until a suitable bench and raw reports are attached.

## Artifact manifest

Store artifacts under `docs/contracts/reference-validation-artifacts/<platform>/`:

- `bench.md`: fixed test bench details.
- `reference-report-pass-01.*`, `reference-report-pass-02.*`, and so on: one
  raw reference report per pass.
- `studiofs-bench-report-pass-01.json`, `studiofs-bench-report-pass-02.json`,
  and so on: one raw `studiofs-bench report` in JSON format per pass.
- `studiofs-bench-report-pass-01.csv`, `studiofs-bench-report-pass-02.csv`, and
  so on: one raw `studiofs-bench report` in CSV format per pass.
- `summary.md`: averages, variance calculation, stability notes, and result.

Use `failed`, `passed`, or `unstable-target` as the summary result. Include
likely causes for any differences outside the variance threshold.
