# Cache and Write Sync Contract

## Cache mode

`enabled` uses normal synchronous file I/O with the OS cache policy unchanged.
`disabled` bypasses the OS file-data cache for both reads and writes:

- Windows: `FILE_FLAG_NO_BUFFERING`; sector sizes come from `FileStorageInfo`.
- Linux: `O_DIRECT`; buffer and request alignment come from `STATX_DIOALIGN`.
  Missing or zero alignment is an error, rather than an assumed guarantee.
- macOS: `fcntl(F_NOCACHE, 1)` must succeed.
- Unsupported platforms or targets fail. There is no cached fallback.

Direct I/O addresses, offsets, and request lengths meet the queried alignment.
The streaming block size is rounded up where necessary. Final writes are padded
and truncated to the logical file size before sync. Reads request aligned lengths
and accept an EOF-short final transfer only if it contains every requested
logical byte. Other short direct transfers fail without retrying an unaligned
remainder. Interrupted system calls retry the same aligned request.

Reports and progress count logical workload bytes. Alignment padding adds
physical work, so tiny workloads are unsuitable for sustained throughput claims.
The setting does not control device caches or caches inside remote servers and
mounted filesystem services. Successful platform calls establish the client's
I/O request mode, not the behavior of an arbitrary third-party filesystem driver.

## Write sync

With `batch_fsync = true`, write the sequence first and then sync every path
that received data, including a partial last file on cancellation. With
`batch_fsync = false`, sync each file before its final write sample is emitted.
Failures are surfaced and prevent successful completion of the pass.

Write throughput includes measured write time, any tail truncation, and all file
sync time. Preparation and callbacks are excluded. The final batch sample and
completed write average include the sync time; earlier samples are provisional.

## Platform references

- [Windows file buffering](https://learn.microsoft.com/en-us/windows/win32/fileio/file-buffering)
- [Windows file storage information](https://learn.microsoft.com/en-us/windows/win32/api/winbase/ns-winbase-file_storage_info)
- [Linux O_DIRECT](https://man7.org/linux/man-pages/man2/open.2.html)
- [Linux statx alignment](https://man7.org/linux/man-pages/man2/statx.2.html)
- [Apple fcntl](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/fcntl.2.html)
