use super::{
    CacheMode, DiskTestMode, ExecutionMode, FileLayout, StreamingIoPhase, WorkloadPreset,
    WorkloadSize,
};

pub(super) fn workload_size_label(value: WorkloadSize) -> &'static str {
    match value {
        WorkloadSize::Preset(WorkloadPreset::OneGb) => "1 GB",
        WorkloadSize::Preset(WorkloadPreset::FourGb) => "4 GB",
        WorkloadSize::Preset(WorkloadPreset::SixteenGb) => "16 GB",
        WorkloadSize::Preset(WorkloadPreset::SixtyFourGb) => "64 GB",
        WorkloadSize::CustomGb(_) => "custom",
    }
}

pub(super) fn next_workload_size(value: WorkloadSize, next: bool) -> WorkloadSize {
    const VALUES: [WorkloadSize; 4] = [
        WorkloadSize::Preset(WorkloadPreset::OneGb),
        WorkloadSize::Preset(WorkloadPreset::FourGb),
        WorkloadSize::Preset(WorkloadPreset::SixteenGb),
        WorkloadSize::Preset(WorkloadPreset::SixtyFourGb),
    ];
    cycle(value, &VALUES, next)
}

pub(super) fn test_mode_label(value: DiskTestMode) -> &'static str {
    match value {
        DiskTestMode::ReadWrite => "read/write",
        DiskTestMode::WriteOnly => "write only",
        DiskTestMode::WriteOnceReadLoop => "write once, read loop",
    }
}

pub(super) fn next_test_mode(value: DiskTestMode, next: bool) -> DiskTestMode {
    const VALUES: [DiskTestMode; 3] = [
        DiskTestMode::ReadWrite,
        DiskTestMode::WriteOnly,
        DiskTestMode::WriteOnceReadLoop,
    ];
    cycle(value, &VALUES, next)
}

pub(super) fn file_layout_label(value: FileLayout) -> &'static str {
    match value {
        FileLayout::SingleFile => "single file",
        FileLayout::HundredFilesPlusMinusFive => "100 files +/-5%",
        FileLayout::FixedFileSizeMb(_) => "fixed file size",
    }
}

pub(super) fn next_file_layout(value: FileLayout, next: bool) -> FileLayout {
    const VALUES: [FileLayout; 2] = [
        FileLayout::SingleFile,
        FileLayout::HundredFilesPlusMinusFive,
    ];
    cycle(value, &VALUES, next)
}

pub(super) fn cache_mode_label(value: CacheMode) -> &'static str {
    match value {
        CacheMode::Enabled => "enabled",
        CacheMode::Disabled => "disabled",
    }
}

pub(super) fn next_cache_mode(value: CacheMode) -> CacheMode {
    match value {
        CacheMode::Enabled => CacheMode::Disabled,
        CacheMode::Disabled => CacheMode::Enabled,
    }
}

pub(super) fn execution_mode_label(value: ExecutionMode) -> &'static str {
    match value {
        ExecutionMode::RunOnce => "run once",
        ExecutionMode::Continuous => "continuous",
    }
}

pub(super) fn next_execution_mode(value: ExecutionMode) -> ExecutionMode {
    match value {
        ExecutionMode::RunOnce => ExecutionMode::Continuous,
        ExecutionMode::Continuous => ExecutionMode::RunOnce,
    }
}

pub(super) fn bool_label(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

pub(super) fn cycle<T: Copy + PartialEq>(value: T, values: &[T], next: bool) -> T {
    assert!(!values.is_empty(), "cannot cycle through an empty slice");
    let index = values.iter().position(|item| *item == value).unwrap_or(0);
    let index = if next {
        (index + 1) % values.len()
    } else {
        (index + values.len() - 1) % values.len()
    };
    values[index]
}

pub(super) fn phase_label(phase: StreamingIoPhase) -> &'static str {
    match phase {
        StreamingIoPhase::Write => "write",
        StreamingIoPhase::Read => "read",
    }
}
