// SPDX-License-Identifier: MIT OR Apache-2.0
//! Deterministic path labels for snapshots of real temporary filesystem data.
//! This feature changes rendering only; filesystem operations retain real paths.
use std::cell::RefCell;
use std::path::{Path, PathBuf};

thread_local! {
    static DISPLAY_ROOT: RefCell<Option<(PathBuf, PathBuf)>> = const { RefCell::new(None) };
}

/// Render a real temporary home using a stable display prefix on this thread.
/// Nested scopes and panics restore the previous prefix.
pub fn with_display_root<R>(real: &Path, display: &Path, render: impl FnOnce() -> R) -> R {
    struct Restore(Option<(PathBuf, PathBuf)>);
    impl Drop for Restore {
        fn drop(&mut self) {
            DISPLAY_ROOT.with(|root| {
                let _ = root.replace(self.0.take());
            });
        }
    }
    let _restore =
        Restore(DISPLAY_ROOT.with(|root| root.replace(Some((real.into(), display.into())))));
    render()
}

pub(crate) fn display_path(path: &Path) -> PathBuf {
    DISPLAY_ROOT.with(|root| {
        if let Some((real, display)) = root.borrow().as_ref()
            && let Ok(relative) = path.strip_prefix(real)
        {
            if relative.as_os_str().is_empty() {
                return display.clone();
            }
            return display.join(relative);
        }
        path.into()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn display_prefix_is_component_scoped_and_restored_after_nested_panic() {
        let path = Path::new("/real/home/notes.txt");
        with_display_root(Path::new("/real/home"), Path::new("/fixture/home"), || {
            assert_eq!(display_path(Path::new("/real/home")).to_string_lossy(), "/fixture/home");
            assert_eq!(display_path(path), Path::new("/fixture/home/notes.txt"));
            assert_eq!(
                display_path(Path::new("/real/home-other")),
                Path::new("/real/home-other")
            );
            let failed = std::panic::catch_unwind(|| {
                with_display_root(Path::new("/real"), Path::new("/nested"), || {
                    panic!("fixture failure")
                })
            });
            assert!(failed.is_err());
            assert_eq!(display_path(path), Path::new("/fixture/home/notes.txt"));
        });
        assert_eq!(display_path(path), path);
    }
}
