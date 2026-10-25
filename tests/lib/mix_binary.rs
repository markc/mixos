// SPDX-License-Identifier: MIT OR Apache-2.0
//! Select a test interpreter independently of the libtest executable.
use std::path::{Path, PathBuf};

pub fn executable(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(path_c) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    // SAFETY: path_c is NUL-terminated and alive for this effective-ID check.
    path.is_file()
        && unsafe {
            libc::faccessat(libc::AT_FDCWD, path_c.as_ptr(), libc::X_OK, libc::AT_EACCESS)
                == 0
        }
}

/// An explicit selection fails closed. Otherwise use the first executable
/// `mix` on PATH, just as a child process would; absence can be skipped.
pub fn select() -> Result<Option<PathBuf>, String> {
    if let Some(path) = std::env::var_os("MIXOS_E2E_MIX_BIN") {
        let path = PathBuf::from(path);
        if !executable(&path) {
            return Err(format!("MIXOS_E2E_MIX_BIN is not an executable file: {}", path.display()));
        }
        return path.canonicalize().map(Some).map_err(|e| e.to_string());
    }
    let Some(search_path) = std::env::var_os("PATH") else {
        return Ok(None);
    };
    for dir in std::env::split_paths(&search_path) {
        let path = dir.join("mix");
        if executable(&path) {
            return path.canonicalize().map(Some).map_err(|e| e.to_string());
        }
    }
    Ok(None)
}
