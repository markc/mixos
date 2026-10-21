// SPDX-License-Identifier: MIT OR Apache-2.0
//! Non-blocking sync_file signal timestamps from the Linux userspace ABI.

use std::os::fd::{AsRawFd, BorrowedFd};
use std::time::Duration;

// Layouts from include/uapi/linux/sync_file.h. Names are unused ABI fields.
#[repr(C)]
#[derive(Default)]
struct FileInfo {
    name: [u8; 32],
    status: i32,
    flags: u32,
    num_fences: u32,
    pad: u32,
    fences: u64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct FenceInfo {
    names: [u8; 64],
    status: i32,
    flags: u32,
    timestamp_ns: u64,
}

// SYNC_IOC_FILE_INFO = _IOWR('>', 4, struct sync_file_info).
const REQUEST: libc::c_ulong = 0xc038_3e04;
const _: () = assert!(std::mem::size_of::<FileInfo>() == 56);
const _: () = assert!(std::mem::size_of::<FenceInfo>() == 80);

/// The latest successful signal in a sync_file, in CLOCK_MONOTONIC time.
/// One bounded query, no wait or polling. More than 16 constituent fences,
/// unsupported ioctls, active/error fences and absent timestamps omit a sample.
pub fn signalled_at(fd: BorrowedFd<'_>) -> Option<Duration> {
    let mut fences = [FenceInfo { names: [0; 64], status: 0, flags: 0, timestamp_ns: 0 }; 16];
    let mut info = FileInfo {
        num_fences: fences.len() as u32,
        fences: fences.as_mut_ptr() as u64,
        ..Default::default()
    };
    // SAFETY: both repr(C) buffers match the UAPI, are writable and live for
    // the ioctl. num_fences bounds the array the kernel may write through fences.
    let rc = unsafe { libc::ioctl(fd.as_raw_fd(), REQUEST, &mut info as *mut FileInfo) };
    if rc != 0 || info.status != 1 {
        return None;
    }
    latest(fences.get(..info.num_fences as usize)?)
}

fn latest(fences: &[FenceInfo]) -> Option<Duration> {
    if fences.iter().any(|f| f.status != 1 || f.timestamp_ns == 0) {
        return None;
    }
    fences.iter().map(|f| Duration::from_nanos(f.timestamp_ns)).max()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::AsFd;
    use std::os::unix::net::UnixStream;

    #[test]
    fn merged_fence_requires_all_successful_timestamps_and_uses_latest() {
        let good = FenceInfo { names: [0; 64], status: 1, flags: 0, timestamp_ns: 3_000_000 };
        let later = FenceInfo { timestamp_ns: 4_000_000, ..good };
        assert_eq!(latest(&[later, good]), Some(Duration::from_millis(4)));
        assert_eq!(latest(&[]), None);
        for bad in [FenceInfo { status: 0, ..good }, FenceInfo { status: -5, ..good }, FenceInfo { timestamp_ns: 0, ..good }] {
            assert_eq!(latest(&[good, bad]), None);
        }
    }

    #[test]
    fn non_sync_file_is_rejected_without_waiting() {
        let (reader, _writer) = UnixStream::pair().unwrap();
        assert_eq!(signalled_at(reader.as_fd()), None);
    }
}
