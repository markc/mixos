// SPDX-License-Identifier: MIT OR Apache-2.0
//! Use a real stderr tty so the fmt layer is present, without starting a GUI.
use std::fs::File;
use std::io::Read;
use std::os::fd::FromRawFd;
use std::process::{Command, Stdio};

#[test]
fn a_terminal_error_prints_once_even_when_logging_is_off() {
    for level in ["warn", "off"] {
        let (mut master, mut slave) = (-1, -1);
        // SAFETY: both fd pointers are valid; null opts request default tty settings.
        assert_eq!(
            unsafe {
                libc::openpty(
                    &mut master,
                    &mut slave,
                    std::ptr::null_mut(),
                    std::ptr::null(),
                    std::ptr::null(),
                )
            },
            0
        );
        // SAFETY: openpty returned two owned fds; each is transferred exactly once.
        let (mut master, slave) = unsafe { (File::from_raw_fd(master), File::from_raw_fd(slave)) };
        let status = Command::new(env!("CARGO_BIN_EXE_backdrop"))
            .args([format!("--debug={level}"), "--bogus".into()])
            .env_remove("RUST_LOG")
            .stdout(Stdio::null())
            .stderr(slave)
            .status()
            .unwrap();
        assert_eq!(status.code(), Some(2));
        let mut bytes = Vec::new();
        // Linux pty masters return EIO after the last slave closes.
        if let Err(error) = master.read_to_end(&mut bytes) {
            assert_eq!(error.raw_os_error(), Some(libc::EIO));
        }
        let output = String::from_utf8_lossy(&bytes);
        assert_eq!(
            output.matches("unknown option --bogus").count(),
            1,
            "{output}"
        );
    }
}
