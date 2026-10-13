use std::fs::{File, OpenOptions};
use std::io;

use crate::CacheMode;

pub(super) fn configure_options(options: &mut OpenOptions, mode: CacheMode) -> io::Result<()> {
    if mode == CacheMode::Enabled {
        return Ok(());
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_FLAG_NO_BUFFERING: u32 = 0x2000_0000;
        options.custom_flags(FILE_FLAG_NO_BUFFERING);
    }
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_DIRECT);
    }
    #[cfg(target_os = "macos")]
    let _ = options;
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    {
        let _ = options;
        return Err(unsupported(
            "uncached file I/O is not implemented on this OS",
        ));
    }
    Ok(())
}

pub(super) fn configure_file(file: &File, mode: CacheMode) -> io::Result<()> {
    #[cfg(target_os = "macos")]
    if mode == CacheMode::Disabled {
        use std::os::fd::AsRawFd;
        // SAFETY: file owns a live descriptor; F_NOCACHE takes an integer switch.
        if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_NOCACHE, 1) } == -1 {
            return Err(io::Error::last_os_error());
        }
    }
    let _ = (file, mode);
    Ok(())
}

// One alignment for addresses, offsets and lengths satisfies both OS constraints.
pub(super) fn alignment(file: &File, mode: CacheMode) -> io::Result<usize> {
    if mode == CacheMode::Enabled {
        return Ok(1);
    }
    platform_alignment(file)
}

#[cfg(any(windows, target_os = "linux", test))]
fn checked_alignment(memory: u32, offset: u32) -> io::Result<usize> {
    if !memory.is_power_of_two() || !offset.is_power_of_two() {
        return Err(unsupported(
            "the filesystem did not provide valid direct I/O alignment",
        ));
    }
    Ok(memory.max(offset) as usize)
}

#[cfg(windows)]
fn platform_alignment(file: &File) -> io::Result<usize> {
    use std::ffi::c_void;
    use std::os::windows::io::AsRawHandle;

    #[repr(C)]
    #[derive(Default)]
    struct FileStorageInfo {
        logical_sector: u32,
        physical_atomic_sector: u32,
        physical_performance_sector: u32,
        effective_physical_sector: u32,
        flags: u32,
        sector_offset: u32,
        partition_offset: u32,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetFileInformationByHandleEx(
            handle: *mut c_void,
            class: i32,
            info: *mut c_void,
            size: u32,
        ) -> i32;
    }
    let mut info = FileStorageInfo::default();
    const FILE_STORAGE_INFO: i32 = 16;
    // SAFETY: the handle is live and info is a writable FILE_STORAGE_INFO of the stated size.
    let ok = unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            FILE_STORAGE_INFO,
            (&mut info as *mut FileStorageInfo).cast(),
            std::mem::size_of::<FileStorageInfo>() as u32,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    let memory = info
        .logical_sector
        .max(info.physical_atomic_sector)
        .max(info.physical_performance_sector)
        .max(info.effective_physical_sector);
    checked_alignment(memory, info.logical_sector)
}

#[cfg(target_os = "linux")]
fn platform_alignment(file: &File) -> io::Result<usize> {
    use std::os::fd::AsRawFd;
    let mut info = std::mem::MaybeUninit::<libc::statx>::zeroed();
    // SAFETY: file owns a live fd; AT_EMPTY_PATH addresses it, and info has the exact statx layout.
    let result = unsafe {
        libc::statx(
            file.as_raw_fd(),
            c"".as_ptr(),
            libc::AT_EMPTY_PATH,
            libc::STATX_DIOALIGN,
            info.as_mut_ptr(),
        )
    };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: statx succeeded and the storage was zero-initialized, including unreported fields.
    let info = unsafe { info.assume_init() };
    if info.stx_mask & libc::STATX_DIOALIGN == 0 {
        return Err(unsupported(
            "uncached I/O requires filesystem support for STATX_DIOALIGN (Linux 6.1+)",
        ));
    }
    checked_alignment(info.stx_dio_mem_align, info.stx_dio_offset_align)
}

#[cfg(target_os = "macos")]
fn platform_alignment(_: &File) -> io::Result<usize> {
    Ok(1)
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
fn platform_alignment(_: &File) -> io::Result<usize> {
    Err(unsupported(
        "uncached file I/O is not implemented on this OS",
    ))
}

#[cfg(any(not(target_os = "macos"), test))]
fn unsupported(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::Unsupported, message)
}

pub(super) fn rounded_len(bytes: usize, alignment: usize) -> io::Result<usize> {
    bytes
        .checked_add(alignment - 1)
        .map(|value| value / alignment * alignment)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "direct I/O buffer size overflow",
            )
        })
}

pub(super) struct AlignedBuffer {
    storage: Vec<u8>,
    start: usize,
    len: usize,
}

impl AlignedBuffer {
    pub(super) fn new(bytes: usize, alignment: usize) -> io::Result<Self> {
        let len = rounded_len(bytes, alignment)?;
        let storage_len = len.checked_add(alignment - 1).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "direct I/O buffer size overflow",
            )
        })?;
        let mut storage = Vec::new();
        storage
            .try_reserve_exact(storage_len)
            .map_err(io::Error::other)?;
        storage.resize(storage_len, 0);
        let address = storage.as_ptr() as usize;
        let start = (alignment - address % alignment) % alignment;
        Ok(Self {
            storage,
            start,
            len,
        })
    }

    pub(super) fn as_mut_slice(&mut self) -> &mut [u8] {
        &mut self.storage[self.start..self.start + self.len]
    }
}

#[cfg(test)]
mod tests {
    #[cfg(any(windows, target_os = "linux"))]
    use super::super::*;

    #[test]
    fn aligned_buffers_round_lengths_and_align_addresses() {
        for alignment in [512, 4096, 65536] {
            let mut buffer = super::AlignedBuffer::new(4097, alignment).unwrap();
            let slice = buffer.as_mut_slice();
            assert_eq!(slice.as_ptr() as usize % alignment, 0);
            assert_eq!(slice.len() % alignment, 0);
            assert!(slice.len() >= 4097);
            slice.fill(42);
            assert_eq!(buffer.as_mut_slice()[4096], 42);
        }
    }

    #[test]
    fn invalid_alignment_and_size_overflow_are_errors() {
        for (memory, offset) in [(0, 512), (512, 0), (3, 512)] {
            assert!(super::checked_alignment(memory, offset).is_err());
        }
        assert!(super::AlignedBuffer::new(usize::MAX, 4096).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn disabled_cache_rejects_unaligned_raw_write() {
        let path = std::env::temp_dir().join(format!(
            "studiofs-bench-no-buffering-{}.bin",
            std::process::id()
        ));
        let mut file = create_file(&path, CacheMode::Disabled).unwrap();
        let result = file.write(&[1, 2, 3]);
        drop(file);
        std::fs::remove_file(&path).unwrap();
        assert!(
            result.is_err(),
            "disabled cache unexpectedly accepted a non-sector-sized write"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn disabled_cache_opens_with_direct_io() {
        use std::os::fd::AsRawFd;
        let path =
            std::env::temp_dir().join(format!("studiofs-bench-direct-{}.bin", std::process::id()));
        let file = create_file(&path, CacheMode::Disabled).unwrap();
        // SAFETY: the file owns a live descriptor; F_GETFL takes no extra argument.
        let flags = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETFL) };
        drop(file);
        std::fs::remove_file(&path).unwrap();
        assert_ne!(flags & libc::O_DIRECT, 0);
    }
}
