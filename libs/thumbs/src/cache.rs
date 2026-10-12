// SPDX-License-Identifier: MIT OR Apache-2.0
//! The shared thumbnail cache of the freedesktop Thumbnail Managing Standard:
//! `$XDG_CACHE_HOME/thumbnails/<size>/<md5 of the file URI>.png`, valid while
//! its `Thumb::URI` and `Thumb::MTime` keys match the file. Every desktop
//! reads and writes the same cache, so a thumbnail another file manager made
//! shows here at once, and the reverse.

use std::ffi::OsString;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use md5::{Digest, Md5};

use crate::png;

/// The standard's sizes: the largest side, in px.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Size {
    Normal,
    Large,
    XLarge,
    XXLarge,
}

impl Size {
    pub const ALL: [Size; 4] = [Size::Normal, Size::Large, Size::XLarge, Size::XXLarge];

    pub fn px(self) -> u32 {
        match self {
            Size::Normal => 128,
            Size::Large => 256,
            Size::XLarge => 512,
            Size::XXLarge => 1024,
        }
    }

    pub fn dir(self) -> &'static str {
        match self {
            Size::Normal => "normal",
            Size::Large => "large",
            Size::XLarge => "x-large",
            Size::XXLarge => "xx-large",
        }
    }

    /// The smallest size at least `px` (the largest for anything bigger).
    pub fn at_least(px: u32) -> Size {
        Size::ALL
            .into_iter()
            .find(|s| s.px() >= px)
            .unwrap_or(Size::XXLarge)
    }
}

/// The cache root from `$XDG_CACHE_HOME` (when absolute) or `$HOME/.cache`.
pub fn root_from(xdg_cache_home: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
    let base = xdg_cache_home
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| {
            home.map(PathBuf::from)
                .filter(|p| p.is_absolute())
                .map(|h| h.join(".cache"))
        })?;
    Some(base.join("thumbnails"))
}

/// The cache root for this process's environment.
pub fn root() -> Option<PathBuf> {
    root_from(std::env::var_os("XDG_CACHE_HOME"), std::env::var_os("HOME"))
}

/// A local file's URI as the standard names it: `file://` and the absolute
/// path, its bytes percent-escaped except RFC 3986's unreserved and
/// path-safe characters (what GLib and COSMIC Files write). `None` for a
/// relative path.
pub fn uri(path: &Path) -> Option<String> {
    use std::os::unix::ffi::OsStrExt;
    if !path.is_absolute() {
        return None;
    }
    let mut out = String::from("file://");
    for &b in path.as_os_str().as_bytes() {
        let safe = b.is_ascii_alphanumeric() || b"-._~!$&'()*+,;=:@/".contains(&b);
        if safe {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    Some(out)
}

/// The cache file name for `uri`: its MD5 in lowercase hex, `.png`.
pub fn name(uri: &str) -> String {
    let digest = Md5::digest(uri.as_bytes());
    let mut out = String::with_capacity(36);
    for b in digest {
        out.push_str(&format!("{b:02x}"));
    }
    out.push_str(".png");
    out
}

/// Seconds since the epoch, as `Thumb::MTime` records it.
pub fn mtime_secs(modified: SystemTime) -> Option<u64> {
    modified
        .duration_since(SystemTime::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs())
}

/// What the cache holds for a file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Lookup {
    /// A valid thumbnail at least the size asked for.
    Hit(PathBuf),
    /// A valid failure record: generating again would fail the same way.
    Failed,
    Miss,
}

/// The cache, rooted at a directory (normally [`root`]).
#[derive(Clone, Debug)]
pub struct Cache {
    root: PathBuf,
    /// The application that writes through this cache: its directory under
    /// `fail/`, the `Software` key of what it stores, and the mark on its
    /// temporary files ([`is_temporary`]).
    app: String,
}

impl Cache {
    /// The cache at `root`, written as `app`: a plain name (lowercase ASCII
    /// letters, digits and `-`, starting with a letter or digit), since it
    /// becomes part of file names. Panics on any other name: the caller's
    /// constant is wrong.
    pub fn new(root: PathBuf, app: &str) -> Cache {
        assert!(
            valid_app(app),
            "thumbnail cache app name {app:?} is not a plain name"
        );
        Cache {
            root,
            app: app.to_owned(),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn app(&self) -> &str {
        &self.app
    }

    pub fn path(&self, uri: &str, size: Size) -> PathBuf {
        self.root.join(size.dir()).join(name(uri))
    }

    pub fn fail_path(&self, uri: &str) -> PathBuf {
        self.root.join("fail").join(&self.app).join(name(uri))
    }

    /// Files inside the cache are never thumbnailed (the standard's rule).
    pub fn holds(&self, path: &Path) -> bool {
        path.starts_with(&self.root)
    }

    /// The thumbnail for `file` (modified at `mtime` seconds, `file_size`
    /// bytes long) at `size` or any larger size, else a failure record, else
    /// a miss. Reads at most a thumbnail's first chunks; never decodes
    /// pixels.
    pub fn lookup(&self, file: &Path, mtime: u64, file_size: u64, size: Size) -> Lookup {
        let Some(uri) = uri(file) else {
            return Lookup::Miss;
        };
        let valid = |path: &Path| {
            read_regular(path, MAX_ENTRY)
                .ok()
                .is_some_and(|bytes| matches_file(&bytes, &uri, mtime, file_size))
        };
        for candidate in Size::ALL.into_iter().filter(|s| *s >= size) {
            let path = self.path(&uri, candidate);
            if valid(&path) {
                return Lookup::Hit(path);
            }
        }
        if valid(&self.fail_path(&uri)) {
            return Lookup::Failed;
        }
        Lookup::Miss
    }

    /// Store `png` (a thumbnailer's output) as `file`'s thumbnail at `size`,
    /// with the standard's keys. Returns the cache path.
    pub fn store(
        &self,
        file: &Path,
        mtime: u64,
        file_size: u64,
        mime: Option<&str>,
        size: Size,
        png: &[u8],
    ) -> io::Result<PathBuf> {
        let uri = uri(file).ok_or_else(|| io::Error::other("not an absolute path"))?;
        let mtime = mtime.to_string();
        let file_size = file_size.to_string();
        let mut keys = vec![
            ("Thumb::URI", uri.as_str()),
            ("Thumb::MTime", mtime.as_str()),
            ("Thumb::Size", file_size.as_str()),
            ("Software", self.app.as_str()),
        ];
        if let Some(mime) = mime {
            keys.push(("Thumb::Mimetype", mime));
        }
        let tagged = png::with_text(png, &keys)
            .ok_or_else(|| io::Error::other("the thumbnailer wrote no valid PNG"))?;
        // An entry past MAX_ENTRY would never be read back (by lookup or the
        // display): refused, as the output's fault.
        if tagged.len() as u64 > MAX_ENTRY {
            return Err(io::Error::new(
                io::ErrorKind::FileTooLarge,
                "too large once tagged",
            ));
        }
        let path = self.path(&uri, size);
        write_private(&path, &tagged, &self.app)?;
        Ok(path)
    }

    /// Record that `file` (at `mtime`) could not be thumbnailed.
    pub fn store_failure(&self, file: &Path, mtime: u64) -> io::Result<()> {
        let uri = uri(file).ok_or_else(|| io::Error::other("not an absolute path"))?;
        let mtime = mtime.to_string();
        let tagged = png::with_text(
            &png::blank(),
            &[
                ("Thumb::URI", uri.as_str()),
                ("Thumb::MTime", mtime.as_str()),
                ("Software", self.app.as_str()),
            ],
        )
        .ok_or_else(|| io::Error::other("failure record"))?;
        write_private(&self.fail_path(&uri), &tagged, &self.app)
    }
}

/// The largest cache entry read: an xx-large (1024 px) RGBA thumbnail is a
/// few MiB at worst, so anything past this is not a thumbnail.
pub const MAX_ENTRY: u64 = 16 * 1024 * 1024;

/// Read `path` if it is a regular file (never through a symlink, never
/// blocking on a FIFO or device), and at most `max` bytes (refused past it).
/// The cache is shared with every other program of the user; nothing in it
/// is trusted to be what its name says.
pub fn read_regular(path: &Path, max: u64) -> io::Result<Vec<u8>> {
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_NONBLOCK)
        .open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() {
        return Err(io::Error::other("not a regular file"));
    }
    if meta.len() > max {
        return Err(io::Error::other("too large"));
    }
    let mut bytes = Vec::with_capacity(meta.len() as usize);
    file.take(max + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > max {
        return Err(io::Error::other("too large"));
    }
    Ok(bytes)
}

/// A name no other writer uses: process, a per-process counter and the
/// clock. Callers still create it exclusively (`create_new`).
pub fn unique(prefix: &str) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    format!(
        "{prefix}.{}.{}.{nanos}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

/// Whether a cached thumbnail's keys name `uri` at `mtime`, and, when the
/// entry records `Thumb::Size` (it is optional), at `file_size` bytes.
fn matches_file(bytes: &[u8], uri: &str, mtime: u64, file_size: u64) -> bool {
    png::text(bytes, "Thumb::URI").as_deref() == Some(uri)
        && png::text(bytes, "Thumb::MTime").and_then(|m| m.trim().parse::<u64>().ok())
            == Some(mtime)
        && png::text(bytes, "Thumb::Size")
            .is_none_or(|s| s.trim().parse::<u64>().ok() == Some(file_size))
}

/// Whether `app` is a plain name: lowercase ASCII letters, digits and `-`,
/// starting with a letter or digit.
pub(crate) fn valid_app(app: &str) -> bool {
    app.bytes()
        .next()
        .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        && app
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// A temporary file's name as [`write_private`] makes it, marked as `app`'s
/// own: `.<entry>.<app>-<pid>-<n>.tmp`.
fn temporary_name(entry: &str, app: &str) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    format!(
        ".{entry}.{app}-{}-{}.tmp",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

/// Whether `name` is exactly `app`'s temporary of a cache entry:
/// `.<32 lowercase hex>.png.<app>-<digits>-<digits>.tmp`. Anything else,
/// including temporaries of older builds or other programs, is left alone.
pub(crate) fn is_temporary(name: &str, app: &str) -> bool {
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    let Some(inner) = name.strip_prefix('.').and_then(|n| n.strip_suffix(".tmp")) else {
        return false;
    };
    let Some((entry, tag)) = inner.split_once(&format!(".{app}-")) else {
        return false;
    };
    let Some((pid, n)) = tag.split_once('-') else {
        return false;
    };
    let hex = entry.strip_suffix(".png").is_some_and(|h| {
        h.len() == 32
            && h.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    });
    hex && digits(pid) && digits(n)
}

impl Cache {
    /// Remove what an earlier run of this app left in the directories it
    /// writes: its temporary files ([`is_temporary`]) last modified more
    /// than `age` ago. A younger one may belong to a run still writing it.
    /// No directory below the root that is a symlink is entered.
    pub fn sweep_temporaries(&self, age: std::time::Duration) {
        let ours = |name: &str| is_temporary(name, &self.app);
        for size in Size::ALL {
            sweep_dir(&self.root, &[size.dir()], age, &ours, false);
        }
        sweep_dir(&self.root, &["fail", &self.app], age, &ours, false);
    }
}

/// In `base`/`below…` (`base` opened `O_NOFOLLOW`, then each of `below`
/// opened relative to its parent's descriptor, `O_NOFOLLOW` too: a
/// symlink anywhere on the way is not entered), remove each entry `ours`
/// names that was last modified more than `age` ago: regular files, or
/// directories (with what they hold) when `dirs`. Everything is looked at
/// and removed relative to the opened directory (`fstatat`, `unlinkat`), so
/// no path is re-resolved on the way, and no symlink is followed.
pub(crate) fn sweep_dir(
    base: &Path,
    below: &[&str],
    age: std::time::Duration,
    ours: &dyn Fn(&str) -> bool,
    dirs: bool,
) {
    use nix::sys::stat::{SFlag, fstatat};
    use nix::unistd::{UnlinkatFlags, unlinkat};
    use std::os::fd::AsRawFd;
    let Ok(mut handle) = open_dir(None, base) else {
        return;
    };
    for part in below {
        let Ok(next) = open_dir(Some(handle.as_raw_fd()), *part) else {
            return;
        };
        handle = next;
    }
    let fd = handle.as_raw_fd();
    let names: Vec<std::ffi::CString> = handle
        .iter()
        .flatten()
        .map(|e| e.file_name().to_owned())
        .filter(|n| n.to_str().is_ok_and(ours))
        .collect();
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    for name in names {
        let Ok(stat) = fstatat(
            Some(fd),
            name.as_c_str(),
            nix::fcntl::AtFlags::AT_SYMLINK_NOFOLLOW,
        ) else {
            continue;
        };
        let modified = u64::try_from(stat.st_mtime).unwrap_or(u64::MAX);
        if now.saturating_sub(modified) <= age.as_secs() {
            continue;
        }
        let kind = SFlag::from_bits_truncate(stat.st_mode) & SFlag::S_IFMT;
        if dirs && kind == SFlag::S_IFDIR {
            remove_tree_at(fd, &name, 8);
        } else if !dirs && kind == SFlag::S_IFREG {
            let _ = unlinkat(Some(fd), name.as_c_str(), UnlinkatFlags::NoRemoveDir);
        }
    }
}

/// Open a directory for reading, never through a symlink.
fn open_dir<P: ?Sized + nix::NixPath>(
    parent: Option<std::os::fd::RawFd>,
    path: &P,
) -> nix::Result<nix::dir::Dir> {
    use nix::fcntl::OFlag;
    nix::dir::Dir::openat(
        parent,
        path,
        OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
        nix::sys::stat::Mode::empty(),
    )
}

/// Remove directory `name` in `parent` and what it holds, relative to
/// opened descriptors: symlinks inside are removed, never followed. At most
/// `depth` levels down; anything deeper is left (and so is the directory).
fn remove_tree_at(parent: std::os::fd::RawFd, name: &std::ffi::CStr, depth: u32) {
    use nix::sys::stat::{SFlag, fstatat};
    use nix::unistd::{UnlinkatFlags, unlinkat};
    use std::os::fd::AsRawFd;
    if let Ok(mut handle) = open_dir(Some(parent), name) {
        let fd = handle.as_raw_fd();
        let names: Vec<std::ffi::CString> = handle
            .iter()
            .flatten()
            .map(|e| e.file_name().to_owned())
            .filter(|n| n.as_bytes() != b"." && n.as_bytes() != b"..")
            .collect();
        for child in names {
            let is_dir = fstatat(
                Some(fd),
                child.as_c_str(),
                nix::fcntl::AtFlags::AT_SYMLINK_NOFOLLOW,
            )
            .is_ok_and(|s| SFlag::from_bits_truncate(s.st_mode) & SFlag::S_IFMT == SFlag::S_IFDIR);
            if is_dir {
                if depth > 0 {
                    remove_tree_at(fd, &child, depth - 1);
                }
            } else {
                let _ = unlinkat(Some(fd), child.as_c_str(), UnlinkatFlags::NoRemoveDir);
            }
        }
    }
    let _ = unlinkat(Some(parent), name, UnlinkatFlags::RemoveDir);
}

/// Write `bytes` to `path` atomically: directories 0700, the file 0600
/// (the standard's modes; a thumbnail shows a private file's content).
/// No fsync: the cache is disposable.
fn write_private(path: &Path, bytes: &[u8], app: &str) -> io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    let dir = path
        .parent()
        .ok_or_else(|| io::Error::other("cache path has no directory"))?;
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| io::Error::other("cache path has no file name"))?;
    // A fresh name per write, created exclusively: two workers storing the
    // same thumbnail never share a temporary file, and `create_new` never
    // follows a planted symlink.
    let (tmp, mut file) = (|| {
        for _ in 0..8 {
            let tmp = dir.join(temporary_name(file_name, app));
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&tmp)
            {
                Ok(file) => return Ok((tmp, file)),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
        Err(io::Error::other("no free temporary name"))
    })()?;
    let written = file.write_all(bytes);
    drop(file);
    match written.and_then(|()| fs::rename(&tmp, path)) {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = fs::remove_file(&tmp);
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_follow_the_standard() {
        // Checked against a real shared-cache entry written by another
        // file manager.
        assert_eq!(
            name("file:///tmp/spe-zoom-2.png"),
            "8e7be5832d3cfcb4fbfa18e4c5d812a4.png"
        );
        assert_eq!(
            uri(Path::new("/home/user/My Files/a(1)[2],x#%é.png")).unwrap(),
            "file:///home/user/My%20Files/a(1)%5B2%5D,x%23%25%C3%A9.png"
        );
        assert!(uri(Path::new("relative/a.png")).is_none());
        assert_eq!(Size::at_least(100), Size::Normal);
        assert_eq!(Size::at_least(200), Size::Large);
        assert_eq!(Size::at_least(5000), Size::XXLarge);
    }

    #[test]
    fn the_root_honours_xdg_then_home() {
        assert_eq!(
            root_from(Some("/c".into()), Some("/h".into())),
            Some(PathBuf::from("/c/thumbnails"))
        );
        assert_eq!(
            root_from(Some("relative".into()), Some("/h".into())),
            Some(PathBuf::from("/h/.cache/thumbnails"))
        );
        assert_eq!(root_from(None, None), None);
    }

    #[test]
    fn store_then_lookup_until_the_file_changes() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path().join("thumbnails"), "dopus");
        let file = dir.path().join("photo.jpg");
        fs::write(&file, b"pixels").unwrap();
        assert_eq!(cache.lookup(&file, 100, 6, Size::Normal), Lookup::Miss);

        let stored = cache
            .store(
                &file,
                100,
                6,
                Some("image/jpeg"),
                Size::Large,
                &png::blank(),
            )
            .unwrap();
        assert_eq!(
            fs::metadata(&stored).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let bytes = fs::read(&stored).unwrap();
        assert_eq!(png::text(&bytes, "Thumb::Size").as_deref(), Some("6"));
        // A larger thumbnail serves a smaller request, never the reverse.
        assert_eq!(
            cache.lookup(&file, 100, 6, Size::Normal),
            Lookup::Hit(stored.clone())
        );
        assert_eq!(
            cache.lookup(&file, 100, 6, Size::Large),
            Lookup::Hit(stored)
        );
        assert_eq!(cache.lookup(&file, 100, 6, Size::XLarge), Lookup::Miss);
        // A newer file invalidates it; so does another size at the same
        // mtime (the entry records Thumb::Size).
        assert_eq!(cache.lookup(&file, 101, 6, Size::Normal), Lookup::Miss);
        assert_eq!(cache.lookup(&file, 100, 7, Size::Normal), Lookup::Miss);

        // Two writers of the same entry use separate temporary files.
        let again = cache
            .store(&file, 100, 6, None, Size::Large, &png::blank())
            .unwrap();
        assert!(
            fs::read_dir(again.parent().unwrap()).unwrap().all(|e| !e
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".tmp"))
        );
        // Only regular files are read, through no symlink.
        let link = dir.path().join("link.png");
        std::os::unix::fs::symlink(&again, &link).unwrap();
        assert!(read_regular(&link, MAX_ENTRY).is_err());
        assert!(read_regular(dir.path(), MAX_ENTRY).is_err());
        assert!(read_regular(&again, 4).is_err());
        assert!(read_regular(&again, MAX_ENTRY).is_ok());

        // Output 10 bytes under MAX_ENTRY passes, but not once tagged: it
        // is refused, unstored.
        let mut big = png::blank();
        let iend = big.split_off(big.len() - 12);
        let len = MAX_ENTRY as usize - 10 - big.len() - 12 - iend.len();
        let body = vec![0u8; len];
        let mut crc = crc32fast::Hasher::new();
        crc.update(b"zTXt");
        crc.update(&body);
        big.extend((len as u32).to_be_bytes());
        big.extend(b"zTXt");
        big.extend(&body);
        big.extend(crc.finalize().to_be_bytes());
        big.extend(iend);
        assert_eq!(big.len() as u64, MAX_ENTRY - 10);
        assert!(png::is_whole(&big));
        let refused = cache
            .store(&file, 300, 6, None, Size::Normal, &big)
            .unwrap_err();
        assert_eq!(refused.kind(), io::ErrorKind::FileTooLarge);
        assert_eq!(cache.lookup(&file, 300, 6, Size::Normal), Lookup::Miss);

        cache.store_failure(&file, 101).unwrap();
        assert_eq!(cache.lookup(&file, 101, 6, Size::Normal), Lookup::Failed);
        assert_eq!(cache.lookup(&file, 102, 6, Size::Normal), Lookup::Miss);
        assert!(cache.holds(&cache.fail_path("file:///x")));
        assert!(!cache.holds(&file));
    }

    #[test]
    fn only_old_temporaries_of_the_app_are_swept() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path().join("thumbnails"), "dopus");
        let normal = cache.root().join("normal");
        let fail = cache.root().join("fail/dopus");
        fs::create_dir_all(&normal).unwrap();
        fs::create_dir_all(&fail).unwrap();
        let day_ago = SystemTime::now() - std::time::Duration::from_secs(2 * 86_400);
        let make = |path: &Path, old: bool| {
            let file = fs::File::create(path).unwrap();
            if old {
                file.set_modified(day_ago).unwrap();
            }
        };
        let entry = name("file:///a");
        // What write_private makes is exactly the swept shape.
        assert!(is_temporary(&temporary_name(&entry, "dopus"), "dopus"));
        let old_tmp = normal.join(format!(".{entry}.dopus-12-3.tmp"));
        let new_tmp = normal.join(format!(".{entry}.dopus-12-4.tmp"));
        let old_fail_tmp = fail.join(format!(".{entry}.dopus-12-5.tmp"));
        let foreign = [
            ".goutputstream-XYZ".to_owned(),
            ".other-app.png.backup.tmp".to_owned(),
            // An older dopus build's shape: ambiguous, left alone.
            format!(".{entry}.1.2.3.tmp"),
            format!(".{entry}.dopus-x-1.tmp"),
            format!(".ABC{}.png.dopus-1-1.tmp", &entry[3..32]),
            // Another app's temporary, even in a shared directory.
            format!(".{entry}.ced-12-3.tmp"),
            entry.clone(),
        ];
        make(&old_tmp, true);
        make(&new_tmp, false);
        make(&old_fail_tmp, true);
        for f in &foreign {
            make(&normal.join(f), true);
        }
        // An old temporary that is a directory, or a symlink, stays.
        let old_dir = normal.join(format!(".{entry}.dopus-12-6.tmp"));
        fs::create_dir(&old_dir).unwrap();
        fs::File::open(&old_dir)
            .unwrap()
            .set_modified(day_ago)
            .unwrap();
        cache.sweep_temporaries(std::time::Duration::from_secs(86_400));
        assert!(!old_tmp.exists());
        assert!(!old_fail_tmp.exists());
        assert!(new_tmp.exists(), "a younger one may still be written");
        assert!(old_dir.exists(), "not a regular file");
        for f in &foreign {
            assert!(normal.join(f).exists(), "{f} is not this app's");
        }
    }

    /// A size directory that is a symlink is not entered.
    #[test]
    fn a_symlinked_cache_directory_is_not_swept() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path().join("thumbnails"), "dopus");
        let elsewhere = dir.path().join("elsewhere");
        fs::create_dir_all(&elsewhere).unwrap();
        fs::create_dir_all(cache.root()).unwrap();
        std::os::unix::fs::symlink(&elsewhere, cache.root().join("normal")).unwrap();
        let victim = elsewhere.join(format!(".{}.dopus-1-1.tmp", name("file:///a")));
        let file = fs::File::create(&victim).unwrap();
        file.set_modified(SystemTime::now() - std::time::Duration::from_secs(2 * 86_400))
            .unwrap();
        // An ancestor of a swept directory that is a symlink: `fail` points
        // at a tree whose `dopus/` holds an old temporary.
        let other = dir.path().join("other");
        fs::create_dir_all(other.join("dopus")).unwrap();
        std::os::unix::fs::symlink(&other, cache.root().join("fail")).unwrap();
        let through = other.join(format!("dopus/.{}.dopus-1-2.tmp", name("file:///a")));
        let file = fs::File::create(&through).unwrap();
        file.set_modified(SystemTime::now() - std::time::Duration::from_secs(2 * 86_400))
            .unwrap();
        cache.sweep_temporaries(std::time::Duration::from_secs(86_400));
        assert!(victim.exists());
        assert!(through.exists(), "never through a symlinked ancestor");
    }
}
