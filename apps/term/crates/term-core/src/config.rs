// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The settings a pane starts with. The frontend fills them from the
//! desktop profile's `terminal.*` preferences (settingsd), and from these
//! defaults while there is none.
use serde::Serialize;
use std::{
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

const PROBE_TIMEOUT: Duration = Duration::from_secs(1);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Cursor {
    Block,
    #[default]
    Underline,
    /// A thin vertical bar at the left of the cell.
    Bar,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Config {
    pub font_px: f32,
    pub scrollback: usize,
    pub cursor: Cursor,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            // The size every toolkit app uses (the Ui role), not the Terminal role.
            font_px: design::default_typography(design::TypographyRole::Ui).font_size as f32,
            // Preserve the history limit previously passed to Crosswords::new.
            scrollback: 1000,
            cursor: Cursor::Underline,
        }
    }
}

/// What a new pane is started with. Changing it never touches a running
/// pane: its history capacity and `TERM` are fixed when its shell starts.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Settings {
    #[serde(flatten)]
    pub config: Config,
    #[serde(rename = "TERM")]
    pub term: String,
}

pub fn valid_font(px: f32) -> bool {
    (6.0..=48.0).contains(&px)
}

// VERIFY: term-selection — query the actual database without invoking a shell.
// infocmp handles TERMINFO, TERMINFO_DIRS, ~/.terminfo and system databases.
// Missing tools, missing entries and failed probes all keep the portable TERM.
pub fn selected_term() -> &'static str {
    term_for_probe(
        Command::new("infocmp")
            .args(["-x", "xterm-rio"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .is_ok_and(|mut child| probe_succeeded(&mut child, PROBE_TIMEOUT)),
    )
}

fn probe_succeeded(child: &mut Child, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) => {}
            Err(_) => break,
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        std::thread::sleep(remaining.min(Duration::from_millis(10)));
    }
    // Also clean up on wait errors. kill may race an exit; wait still reaps it.
    let _ = child.kill();
    let _ = child.wait();
    false
}

fn term_for_probe(available: bool) -> &'static str {
    if available {
        "xterm-rio"
    } else {
        "xterm-256color"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_font_size_is_the_ui_design_role() {
        assert_eq!(
            Config::default().font_px,
            design::default_typography(design::TypographyRole::Ui).font_size as f32
        );
        assert!(valid_font(Config::default().font_px));
        assert!(valid_font(6.0) && valid_font(48.0));
        assert!(!valid_font(5.9) && !valid_font(48.1));
        assert!(!valid_font(f32::NAN));
        assert!(!valid_font(f32::INFINITY));
    }

    #[test]
    fn term_selection_success_and_failure() {
        assert_eq!(term_for_probe(true), "xterm-rio");
        assert_eq!(term_for_probe(false), "xterm-256color");
    }

    #[test]
    #[ignore = "child process fixture for probe_timeout_selects_fallback_and_reaps"]
    fn stalled_probe_child() {
        std::thread::sleep(Duration::from_secs(60));
    }

    #[test]
    fn probe_timeout_selects_fallback_and_reaps() {
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "config::tests::stalled_probe_child", "--ignored"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let start = Instant::now();
        assert_eq!(
            term_for_probe(probe_succeeded(&mut child, Duration::from_millis(50))),
            "xterm-256color"
        );
        assert!(start.elapsed() < Duration::from_secs(1));
        assert!(!child.try_wait().unwrap().unwrap().success());
        // Child caches its exit status; waitpid proves the OS child was reaped.
        // SAFETY: a null status pointer is allowed; WNOHANG cannot block.
        assert_eq!(
            unsafe { libc::waitpid(child.id() as i32, std::ptr::null_mut(), libc::WNOHANG) },
            -1
        );
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ECHILD)
        );
    }
}
