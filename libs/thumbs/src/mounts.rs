// SPDX-License-Identifier: MIT OR Apache-2.0
//! Whether a path is on a network or FUSE filesystem, from the kernel's
//! mount table (`/proc/self/mountinfo`) alone, so such a file is refused
//! before anything touches its mount. Asking the mount itself (`stat`,
//! `statfs`, `open`) is a round trip to the server or the FUSE daemon, and
//! a hung one never answers.
//!
//! The classification is deliberately coarse: a path is remote when ANY
//! remote mount (stacked or not) is mounted at a component of its resolved
//! path, and when the table cannot be read or parsed every path is. It
//! does not have to be perfect: whatever slips past it is still touched
//! only on the generator's bounded probe thread.

use std::collections::VecDeque;
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// One mount: where it is and its filesystem type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mount {
    pub point: PathBuf,
    pub fstype: String,
}

/// Filesystem types (mountinfo's names) whose files are not thumbnailed by
/// default: network filesystems, where every read is a round trip; FUSE of
/// every kind (`fuse.sshfs`, `fuse.rclone`, and `fuseblk` too: ntfs-3g's
/// daemon is userspace and can stall, and `statfs` cannot tell it from
/// other FUSE); and `autofs`, whose lookups can trigger a network mount.
/// The `remote` switch of a request lets all of them through.
pub fn remote_fstype(fstype: &str) -> bool {
    fstype == "fuse"
        || fstype == "fuseblk"
        || fstype.starts_with("fuse.")
        || fstype.starts_with("fuseblk.")
        || matches!(
            fstype,
            "nfs"
                | "nfs4"
                | "cifs"
                | "smb3"
                | "smbfs"
                | "9p"
                | "afs"
                | "ceph"
                | "glusterfs"
                | "lustre"
                | "gpfs"
                | "orangefs"
                | "pvfs2"
                | "coda"
                | "ncpfs"
                | "ncp"
                | "davfs"
                | "sshfs"
                | "beegfs"
                | "autofs"
        )
}

/// Parse mountinfo bytes: the mount point is field 5 (octal escapes such as
/// `\040` undone, other bytes kept as they are, UTF-8 or not), the type is
/// the first field after the `-` separator. `None` when any line is
/// malformed: a mount the parser could not read might be the remote one.
pub fn parse(bytes: &[u8]) -> Option<Vec<Mount>> {
    bytes
        .split(|b| *b == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| {
            let fields: Vec<&[u8]> = line.split(|b| *b == b' ').collect();
            let point = fields.get(4)?;
            let dash = fields.iter().position(|f| *f == b"-")?;
            let fstype = fields.get(dash + 1)?;
            (dash >= 6 && point.starts_with(b"/")).then(|| Mount {
                point: unescape(point),
                fstype: String::from_utf8_lossy(fstype).into_owned(),
            })
        })
        .collect()
}

/// Undo mountinfo's `\ooo` octal escapes (space, tab, newline, backslash).
fn unescape(bytes: &[u8]) -> PathBuf {
    use std::os::unix::ffi::OsStringExt;
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let octal = bytes.get(i + 1..i + 4).filter(|d| {
            bytes[i] == b'\\' && d.iter().all(|b| (b'0'..=b'7').contains(b)) && d[0] <= b'3'
        });
        match octal {
            Some(d) => {
                out.push((d[0] - b'0') * 64 + (d[1] - b'0') * 8 + (d[2] - b'0'));
                i += 4;
            }
            None => {
                out.push(bytes[i]);
                i += 1;
            }
        }
    }
    PathBuf::from(OsString::from_vec(out))
}

/// Whether a remote mount is mounted at `path` or any of its ancestors.
/// Lexical: `path` is not resolved or touched.
fn under_remote(mounts: &[Mount], path: &Path) -> bool {
    mounts
        .iter()
        .any(|m| remote_fstype(&m.fstype) && path.starts_with(&m.point))
}

/// Reads mountinfo bytes (`None`: unreadable).
type Reader = dyn Fn() -> Option<Vec<u8>> + Send + Sync;

enum Source {
    /// Read afresh for every question (`/proc/self/mountinfo`): a mount
    /// made a moment ago counts.
    Live(Box<Reader>),
    /// A fixed table (tests); `None` is an unreadable one.
    Fixed(Option<Arc<Vec<Mount>>>),
}

/// The mount table, read without touching any mount.
pub struct Mounts {
    source: Source,
}

/// Set once the unreadable-table warning has been logged.
static WARNED: AtomicBool = AtomicBool::new(false);

impl Mounts {
    /// This process's mounts, from `/proc/self/mountinfo`.
    pub fn proc() -> Mounts {
        Mounts::live(|| std::fs::read("/proc/self/mountinfo").ok())
    }

    /// A table read afresh from `read` for every question.
    pub fn live(read: impl Fn() -> Option<Vec<u8>> + Send + Sync + 'static) -> Mounts {
        Mounts {
            source: Source::Live(Box::new(read)),
        }
    }

    /// A fixed table, from mountinfo text (tests); malformed text is an
    /// unreadable table.
    pub fn fixed(text: &str) -> Mounts {
        Mounts {
            source: Source::Fixed(parse(text.as_bytes()).map(Arc::new)),
        }
    }

    /// The table, or `None` when it cannot be read or parsed.
    fn table(&self) -> Option<Arc<Vec<Mount>>> {
        match &self.source {
            Source::Fixed(table) => table.clone(),
            Source::Live(read) => {
                let table = read()
                    .and_then(|bytes| parse(&bytes))
                    .filter(|table| !table.is_empty());
                if table.is_none() && !WARNED.swap(true, Ordering::Relaxed) {
                    tracing::warn!(
                        "the mount table is unreadable: every file counts as remote, no thumbnails"
                    );
                }
                table.map(Arc::new)
            }
        }
    }

    /// Whether `path` is on a network, FUSE or automount filesystem,
    /// passes through one (a local mount nested in a remote one is reached
    /// by a lookup in the remote directory), or could not be placed (no
    /// table, a relative path, a symlink loop). Every prefix of the path is
    /// classified from the table before it is looked at, and only
    /// components outside every remote mount are ever `lstat`ed, so a
    /// symlink from a local folder into a remote mount is followed in the
    /// table, never into the mount.
    pub fn is_remote(&self, path: &Path) -> bool {
        let Some(table) = self.table() else {
            return true;
        };
        let remote = |p: &Path| under_remote(&table, p);
        if !path.is_absolute() {
            return true;
        }
        let mut pending: VecDeque<OsString> = VecDeque::new();
        push_components(&mut pending, path, false);
        let mut resolved = PathBuf::from("/");
        if remote(&resolved) {
            return true;
        }
        let mut hops = 0;
        while let Some(part) = pending.pop_front() {
            if part == ".." {
                resolved.pop();
                continue;
            }
            let candidate = resolved.join(&part);
            if remote(&candidate) {
                return true;
            }
            match std::fs::symlink_metadata(&candidate) {
                Ok(meta) if meta.file_type().is_symlink() => {
                    hops += 1;
                    let Ok(target) = std::fs::read_link(&candidate) else {
                        return true;
                    };
                    if hops > 40 {
                        return true;
                    }
                    if target.is_absolute() {
                        resolved = PathBuf::from("/");
                    }
                    push_components(&mut pending, &target, true);
                }
                // A missing component: the open after this fails on its own.
                _ => resolved = candidate,
            }
        }
        false
    }
}

/// Queue `path`'s components (`..` kept, `.` and the root dropped), at the
/// front when `front`, in order.
fn push_components(pending: &mut VecDeque<OsString>, path: &Path, front: bool) {
    let parts: Vec<OsString> = path
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s.to_owned()),
            Component::ParentDir => Some("..".into()),
            _ => None,
        })
        .collect();
    if front {
        for part in parts.into_iter().rev() {
            pending.push_front(part);
        }
    } else {
        pending.extend(parts);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A mountinfo line for `point` of type `fstype`.
    fn line(id: u32, point: &str, fstype: &str) -> String {
        format!("{id} 1 0:{id} / {point} rw,relatime shared:{id} - {fstype} src rw\n")
    }

    #[test]
    fn mountinfo_is_parsed_and_any_remote_prefix_counts() {
        let text = [
            line(1, "/", "ext4"),
            line(2, "/mnt/nas", "nfs4"),
            line(3, "/mnt/nas/scratch", "ext4"),
            line(4, "/home/user/My\\040Drive", "fuse.rclone"),
            line(5, "/media/usb", "fuseblk"),
            // A local mount stacked over a remote one, mounted later.
            line(6, "/mnt/share", "cifs"),
            line(7, "/mnt/share", "tmpfs"),
            // ...and a remote one stacked over a local one: the table's
            // order does not say which is on top, so both count.
            line(8, "/mnt/moved", "ext4"),
            line(9, "/mnt/moved", "nfs"),
            line(10, "/net", "autofs"),
            // No optional fields.
            "11 1 0:11 / /srv/ceph rw - ceph src rw\n".into(),
        ]
        .concat();
        let mounts = parse(text.as_bytes()).unwrap();
        assert_eq!(mounts.len(), 11);
        assert_eq!(mounts[3].point, PathBuf::from("/home/user/My Drive"));
        assert_eq!(unescape(b"a\\011b\\134c\\012"), PathBuf::from("a\tb\\c\n"));

        let fixed = Mounts::fixed(&text);
        assert!(fixed.is_remote(Path::new("/mnt/nas/a.png")));
        assert!(fixed.is_remote(Path::new("/home/user/My Drive/a.png")));
        assert!(fixed.is_remote(Path::new("/srv/ceph/x/y")));
        // A local mount reached through a remote one is refused: the walk
        // to it looks `scratch` up in the NFS directory.
        assert!(fixed.is_remote(Path::new("/mnt/nas/scratch/a.png")));
        assert!(fixed.is_remote(Path::new("/media/usb/a.png")), "fuseblk");
        assert!(fixed.is_remote(Path::new("/mnt/share/a.png")), "stacked");
        assert!(fixed.is_remote(Path::new("/mnt/moved/a.png")), "stacked");
        assert!(fixed.is_remote(Path::new("/net/server/a.png")), "autofs");
        assert!(
            !fixed.is_remote(Path::new("/mnt/nasty/a.png")),
            "component-wise"
        );
        assert!(fixed.is_remote(Path::new("relative.png")));
    }

    /// A mount point that is not UTF-8 is kept byte for byte, so a remote
    /// mount there is still found.
    #[test]
    fn non_utf8_mount_points_are_kept() {
        use std::os::unix::ffi::OsStrExt;
        let mut text = line(1, "/", "ext4").into_bytes();
        text.extend(b"2 1 0:2 / /mnt/caf\xe9 rw - nfs src rw\n");
        let mounts = parse(&text).unwrap();
        assert_eq!(mounts[1].point.as_os_str().as_bytes(), b"/mnt/caf\xe9");
        assert!(under_remote(
            &mounts,
            Path::new(std::ffi::OsStr::from_bytes(b"/mnt/caf\xe9/a.png"))
        ));
    }

    /// No table, or one that does not parse: every path is remote.
    #[test]
    fn an_unreadable_table_refuses_everything() {
        assert!(parse(b"1 1 0:1 / / rw - ext4 src rw\ngarbage\n").is_none());
        let garbled = Mounts::fixed("garbage\n");
        assert!(garbled.is_remote(Path::new("/home/user/a.png")));
        let none = Mounts {
            source: Source::Fixed(None),
        };
        assert!(none.is_remote(Path::new("/home/user/a.png")));
    }

    #[test]
    fn remote_types_are_named() {
        for t in [
            "nfs",
            "nfs4",
            "cifs",
            "smb3",
            "9p",
            "afs",
            "ceph",
            "glusterfs",
            "lustre",
            "gpfs",
            "orangefs",
            "coda",
            "ncpfs",
            "fuse",
            "fuseblk",
            "fuse.sshfs",
            "fuse.gvfsd-fuse",
            "autofs",
        ] {
            assert!(remote_fstype(t), "{t}");
        }
        for t in ["ext4", "btrfs", "xfs", "zfs", "tmpfs", "overlay", "vfat"] {
            assert!(!remote_fstype(t), "{t}");
        }
    }

    /// A symlink from a local folder into a remote mount is followed in the
    /// table: the remote side is refused without being looked at (it does
    /// not even exist here).
    #[test]
    fn symlinks_into_a_remote_mount_are_followed_in_the_table() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let remote = root.join("remote");
        let text = [
            line(1, "/", "ext4"),
            line(2, remote.to_str().unwrap(), "fuse.sshfs"),
        ]
        .concat();
        let mounts = Mounts::fixed(&text);
        std::fs::create_dir(root.join("local")).unwrap();
        std::os::unix::fs::symlink(&remote, root.join("link")).unwrap();
        std::os::unix::fs::symlink("../remote/deep", root.join("local/rel")).unwrap();
        assert!(mounts.is_remote(&root.join("link/a.png")));
        assert!(mounts.is_remote(&root.join("local/rel/a.png")));
        assert!(mounts.is_remote(&root.join("local/../remote/a.png")));
        assert!(!mounts.is_remote(&root.join("local/a.png")));
        // A loop is refused, not followed forever.
        std::os::unix::fs::symlink(root.join("loop"), root.join("loop")).unwrap();
        assert!(mounts.is_remote(&root.join("loop/a.png")));
    }

    /// The live table is read for every question: a remote mount made
    /// between two requests counts at once, and a table that turns
    /// unreadable refuses everything.
    #[test]
    fn a_mount_made_between_requests_counts() {
        use std::sync::Mutex;
        let table = Arc::new(Mutex::new(Some(line(1, "/", "ext4"))));
        let source = table.clone();
        let mounts = Mounts::live(move || {
            source
                .lock()
                .unwrap()
                .as_ref()
                .map(|t| t.as_bytes().to_vec())
        });
        let file = Path::new("/nonexistent-thumbs-test/mnt/a.png");
        assert!(!mounts.is_remote(file));
        *table.lock().unwrap() = Some(
            [
                line(1, "/", "ext4"),
                line(2, "/nonexistent-thumbs-test/mnt", "fuse.sshfs"),
            ]
            .concat(),
        );
        assert!(mounts.is_remote(file));
        *table.lock().unwrap() = None;
        assert!(mounts.is_remote(Path::new("/nonexistent-thumbs-test/a.png")));
    }

    #[test]
    fn this_process_has_a_mount_table() {
        let mounts = Mounts::proc();
        if std::fs::metadata("/proc/self/mountinfo").is_ok() {
            assert!(!mounts.table().unwrap().is_empty());
        }
    }
}
