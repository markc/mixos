// SPDX-License-Identifier: MIT OR Apache-2.0
//! Descriptor-relative macro storage. No macro path component may be a link.
use std::ffi::CString;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::{ffi::OsStrExt, fs::OpenOptionsExt};
use std::path::{Component, Path};
use std::sync::atomic::{AtomicU64, Ordering};

pub(crate) struct Target {
    directory: File,
    name: CString,
}

fn name(path: &Path) -> io::Result<CString> {
    CString::new(path.as_os_str().as_bytes()).map_err(io::Error::other)
}

fn syscall(result: libc::c_int) -> io::Result<libc::c_int> {
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(result)
    }
}

impl Target {
    pub(crate) fn create(&self) -> io::Result<File> {
        // SAFETY: live directory and valid name; exclusive creation refuses links.
        let fd = syscall(unsafe {
            libc::openat(
                self.directory.as_raw_fd(),
                self.name.as_ptr(),
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                0o600,
            )
        })?;
        // SAFETY: new descriptor returned by openat.
        Ok(unsafe { File::from_raw_fd(fd) })
    }
    pub(crate) fn open(root: &Path, path: &Path) -> io::Result<Self> {
        let relative = path.strip_prefix(root).map_err(io::Error::other)?;
        let mut parts = relative.components().peekable();
        if !root.is_absolute() {
            return Err(io::Error::other("macro root must be absolute"));
        }
        let mut directory = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
            .open("/")?;
        // Anchor the root without following a link in its own ancestry either.
        for component in root.components() {
            let Component::Normal(component) = component else {
                if component == Component::RootDir {
                    continue;
                }
                return Err(io::Error::other("invalid macro root component"));
            };
            let component = name(Path::new(component))?;
            // SAFETY: live directory and NUL-terminated component.
            let fd = syscall(unsafe {
                libc::openat(
                    directory.as_raw_fd(),
                    component.as_ptr(),
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                )
            })?;
            // SAFETY: new descriptor returned by openat.
            directory = unsafe { File::from_raw_fd(fd) };
        }
        while let Some(part) = parts.next() {
            let Component::Normal(part) = part else {
                return Err(io::Error::other("invalid macro path component"));
            };
            let component = name(Path::new(part))?;
            if parts.peek().is_none() {
                return Ok(Self {
                    directory,
                    name: component,
                });
            }
            // SAFETY: owned directory and NUL-terminated name remain live.
            let fd = syscall(unsafe {
                libc::openat(
                    directory.as_raw_fd(),
                    component.as_ptr(),
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                )
            })?;
            // SAFETY: openat returned a new descriptor, owned here.
            directory = unsafe { File::from_raw_fd(fd) };
        }
        Err(io::Error::other("macro has no name"))
    }

    pub(crate) fn file(&self) -> io::Result<File> {
        // SAFETY: descriptor and name remain live; no symlinks or blocking FIFOs.
        let fd = syscall(unsafe {
            libc::openat(
                self.directory.as_raw_fd(),
                self.name.as_ptr(),
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
            )
        })?;
        // SAFETY: openat returned a new descriptor.
        let file = unsafe { File::from_raw_fd(fd) };
        if !file.metadata()?.is_file() {
            return Err(io::Error::other("macro is not a regular file"));
        }
        Ok(file)
    }

    pub(crate) fn read(&self) -> io::Result<Vec<u8>> {
        let mut bytes = Vec::new();
        self.file()?.read_to_end(&mut bytes)?;
        Ok(bytes)
    }

    pub(crate) fn check(&self) -> io::Result<()> {
        if self.file()?.metadata()?.permissions().readonly() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "macro is read-only",
            ));
        }
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let temporary = CString::new(format!(
            ".macro-check-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ))
        .unwrap();
        let dir = self.directory.as_raw_fd();
        // SAFETY: live directory, valid name and mode for exclusive creation.
        let fd = syscall(unsafe {
            libc::openat(
                dir,
                temporary.as_ptr(),
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_CLOEXEC,
                0o600,
            )
        })?;
        // SAFETY: new descriptor returned above.
        drop(unsafe { File::from_raw_fd(fd) });
        // SAFETY: remove only our scratch name relative to the same descriptor.
        syscall(unsafe { libc::unlinkat(dir, temporary.as_ptr(), 0) })?;
        Ok(())
    }

    pub(crate) fn move_to(&self, destination: &Self) -> io::Result<()> {
        self.file()?;
        // renameat2 publishes the complete source with no empty reservation and
        // atomically refuses existing destinations, including dangling links.
        // SAFETY: both directories and NUL-terminated names remain live.
        syscall(unsafe {
            libc::renameat2(
                self.directory.as_raw_fd(),
                self.name.as_ptr(),
                destination.directory.as_raw_fd(),
                destination.name.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        })?;
        destination.directory.sync_all()?;
        self.directory.sync_all()
    }

    pub(crate) fn remove(&self) -> io::Result<()> {
        // SAFETY: unlink only the named entry in the opened parent directory.
        syscall(unsafe { libc::unlinkat(self.directory.as_raw_fd(), self.name.as_ptr(), 0) })?;
        self.directory.sync_all()
    }

    pub(crate) fn write(&self, bytes: &[u8]) -> io::Result<()> {
        self.write_with_hook(bytes, || {})
    }

    fn write_with_hook(&self, bytes: &[u8], hook: impl FnOnce()) -> io::Result<()> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let permissions = self.file()?.metadata()?.permissions();
        if permissions.readonly() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "macro is read-only",
            ));
        }
        hook();
        let temporary = CString::new(format!(
            ".macro-save-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ))
        .unwrap();
        let dir = self.directory.as_raw_fd();
        // SAFETY: all descriptors and names remain live, mode is supplied for O_CREAT.
        let fd = syscall(unsafe {
            libc::openat(
                dir,
                temporary.as_ptr(),
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                0o600,
            )
        })?;
        // SAFETY: openat returned a new descriptor.
        let mut file = unsafe { File::from_raw_fd(fd) };
        let result = (|| {
            file.set_permissions(permissions)?;
            file.write_all(bytes)?;
            file.sync_all()?;
            // renameat replaces a swapped final symlink itself, never its target.
            // SAFETY: both names are relative to the same live directory.
            syscall(unsafe { libc::renameat(dir, temporary.as_ptr(), dir, self.name.as_ptr()) })?;
            self.directory.sync_all()
        })();
        if result.is_err() {
            // SAFETY: temporary belongs to this operation in the live directory.
            unsafe {
                libc::unlinkat(dir, temporary.as_ptr(), 0);
            }
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn swapped_parent_cannot_redirect_reads_or_saves() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("macros");
        let outside = temp.path().join("outside");
        std::fs::create_dir_all(root.join("ced")).unwrap();
        std::fs::create_dir(&outside).unwrap();
        let path = root.join("ced/safe.mix");
        std::fs::write(&path, "old").unwrap();
        std::fs::write(outside.join("safe.mix"), "untouched").unwrap();
        let target = Target::open(&root, &path).unwrap();
        std::fs::rename(root.join("ced"), root.join("held")).unwrap();
        std::os::unix::fs::symlink(&outside, root.join("ced")).unwrap();
        assert!(Target::open(&root, &path).is_err());
        assert_eq!(target.read().unwrap(), b"old");
        target.write(b"saved").unwrap();
        assert_eq!(
            std::fs::read(outside.join("safe.mix")).unwrap(),
            b"untouched"
        );
        assert_eq!(std::fs::read(root.join("held/safe.mix")).unwrap(), b"saved");
    }

    #[test]
    fn swap_after_check_never_writes_symlink_target() {
        let root = std::env::temp_dir().join(format!("macro-race-{}", std::process::id()));
        std::fs::create_dir_all(root.join("ced")).unwrap();
        let path = root.join("ced/safe.mix");
        let outside = root.with_extension("outside");
        std::fs::write(&path, "old").unwrap();
        std::fs::write(&outside, "untouched").unwrap();
        let target = Target::open(&root, &path).unwrap();
        target
            .write_with_hook(b"saved", || {
                std::fs::remove_file(&path).unwrap();
                std::os::unix::fs::symlink(&outside, &path).unwrap();
            })
            .unwrap();
        assert_eq!(std::fs::read_to_string(&outside).unwrap(), "untouched");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "saved");
        std::fs::remove_dir_all(root).unwrap();
        std::fs::remove_file(outside).unwrap();
    }
}
