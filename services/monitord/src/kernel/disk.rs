// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Disk activity from `/proc/diskstats`, and how full each mounted
//! filesystem is from `/proc/self/mounts` and `statvfs`.
use super::Root;
use serde::Serialize;
use std::collections::BTreeSet;

/// The cumulative counters of one whole disk.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counters {
    pub reads: u64,
    pub sectors_read: u64,
    pub writes: u64,
    pub sectors_written: u64,
    /// Milliseconds the disk had I/O in flight.
    pub busy_ms: u64,
}

/// `/proc/diskstats` sectors are always 512 bytes, whatever the device's
/// own sector size.
pub const SECTOR: u64 = 512;

/// Pseudo devices that are not disks a person thinks of.
fn ignored(name: &str) -> bool {
    ["loop", "ram", "fd", "sr"]
        .iter()
        .any(|prefix| name.starts_with(prefix))
}

/// Every line of `/proc/diskstats`, by device name.
pub fn parse(text: &str) -> Vec<(String, Counters)> {
    text.lines()
        .filter_map(|line| {
            let f: Vec<&str> = line.split_whitespace().collect();
            let n = |i: usize| f.get(i)?.parse::<u64>().ok();
            Some((
                (*f.get(2)?).to_owned(),
                Counters {
                    reads: n(3)?,
                    sectors_read: n(5)?,
                    writes: n(7)?,
                    sectors_written: n(9)?,
                    busy_ms: n(12)?,
                },
            ))
        })
        .collect()
}

/// Whole disks only: a partition has no directory of its own under
/// `/sys/block`, and is counted in its disk.
pub fn disks(root: &Root) -> Vec<(String, Counters)> {
    let Some(text) = root.read("proc/diskstats") else {
        return Vec::new();
    };
    parse(&text)
        .into_iter()
        .filter(|(name, _)| !ignored(name) && root.join(&format!("sys/block/{name}")).exists())
        .collect()
}

/// The disk's model, when the kernel knows it.
pub fn model(root: &Root, name: &str) -> Option<String> {
    root.read_trimmed(&format!("sys/block/{name}/device/model"))
        .filter(|m| !m.is_empty())
}

/// Whether the disk spins (`queue/rotational`).
pub fn rotational(root: &Root, name: &str) -> Option<bool> {
    root.read_u64(&format!("sys/block/{name}/queue/rotational"))
        .map(|v| v == 1)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Filesystem {
    pub mount: String,
    pub device: String,
    pub kind: String,
    pub total: u64,
    pub used: u64,
    pub available: u64,
}

/// Filesystems that hold data on a disk. tmpfs, overlays, cgroup and the
/// other virtual ones are left out.
const REAL: &[&str] = &[
    "ext2", "ext3", "ext4", "xfs", "btrfs", "zfs", "f2fs", "bcachefs", "vfat", "exfat", "ntfs3",
    "ntfs", "jfs", "reiserfs", "nilfs2", "fuseblk",
];

/// `(device, mount point, type)` for each real filesystem in a mounts
/// table, once per device: a btrfs with ten subvolumes mounted is one
/// filesystem, reported at its first mount.
pub fn mounts(text: &str) -> Vec<(String, String, String)> {
    let mut seen = BTreeSet::new();
    text.lines()
        .filter_map(|line| {
            let mut f = line.split_whitespace();
            let device = f.next()?;
            let mount = unescape(f.next()?);
            let kind = f.next()?;
            (REAL.contains(&kind) && seen.insert(device.to_owned()))
                .then(|| (device.to_owned(), mount, kind.to_owned()))
        })
        .collect()
}

/// The mounts table writes a space as `\040`, and likewise tab, newline
/// and backslash.
fn unescape(field: &str) -> String {
    field
        .replace("\\040", " ")
        .replace("\\011", "\t")
        .replace("\\012", "\n")
        .replace("\\134", "\\")
}

/// Space on each real filesystem, measured under the root.
pub fn filesystems(root: &Root) -> Vec<Filesystem> {
    let Some(text) = root.read("proc/self/mounts") else {
        return Vec::new();
    };
    mounts(&text)
        .into_iter()
        .filter_map(|(device, mount, kind)| {
            let (total, available, free) = statvfs(&root.join(&mount))?;
            Some(Filesystem {
                mount,
                device,
                kind,
                total,
                used: total.saturating_sub(free),
                available,
            })
        })
        .collect()
}

/// `(total, available to unprivileged users, free)` bytes.
fn statvfs(path: &std::path::Path) -> Option<(u64, u64, u64)> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut st = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: `c` is a valid NUL-terminated path and `st` is written by
    // the call before it is read.
    let st = unsafe {
        if libc::statvfs(c.as_ptr(), st.as_mut_ptr()) != 0 {
            return None;
        }
        st.assume_init()
    };
    let unit = st.f_frsize as u64;
    Some((
        st.f_blocks as u64 * unit,
        st.f_bavail as u64 * unit,
        st.f_bfree as u64 * unit,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diskstats_reads_the_counters() {
        let stats = parse(
            " 259       0 nvme0n1 1000 5 20000 300 2000 10 40000 500 0 750 900 0 0 0 0\n\
             259       1 nvme0n1p1 10 0 200 3 20 0 400 5 0 7 9\n",
        );
        assert_eq!(stats.len(), 2);
        let (name, c) = &stats[0];
        assert_eq!(name, "nvme0n1");
        assert_eq!(c.sectors_read, 20000);
        assert_eq!(c.sectors_written, 40000);
        assert_eq!(c.busy_ms, 750);
    }

    #[test]
    fn each_device_is_one_filesystem_and_virtual_ones_are_skipped() {
        let found = mounts(
            "/dev/nvme0n1p2 / btrfs rw 0 0\n\
             tmpfs /tmp tmpfs rw 0 0\n\
             /dev/nvme0n1p2 /home btrfs rw 0 0\n\
             /dev/sda1 /mnt/my\\040disk ext4 rw 0 0\n",
        );
        assert_eq!(
            found,
            vec![
                ("/dev/nvme0n1p2".into(), "/".into(), "btrfs".into()),
                ("/dev/sda1".into(), "/mnt/my disk".into(), "ext4".into()),
            ]
        );
    }

    #[test]
    fn loop_and_ram_devices_are_not_disks() {
        assert!(ignored("loop0") && ignored("ram1"));
        assert!(!ignored("nvme0n1") && !ignored("sda") && !ignored("zram0"));
    }
}
