// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The command line's local actions. They run as the person who typed them
//! and the kernel decides, exactly as `kill` and `renice` do. No Bus command
//! reaches these: acting across users or nodes is hostd's.

/// Signal names `monitord signal` takes, with or without `SIG`.
const SIGNALS: &[(&str, libc::c_int)] = &[
    ("TERM", libc::SIGTERM),
    ("KILL", libc::SIGKILL),
    ("INT", libc::SIGINT),
    ("HUP", libc::SIGHUP),
    ("QUIT", libc::SIGQUIT),
    ("STOP", libc::SIGSTOP),
    ("CONT", libc::SIGCONT),
    ("USR1", libc::SIGUSR1),
    ("USR2", libc::SIGUSR2),
];

pub fn signal_number(name: &str) -> Option<libc::c_int> {
    if let Ok(n) = name.parse::<libc::c_int>() {
        return (1..=64).contains(&n).then_some(n);
    }
    let upper = name.to_ascii_uppercase();
    let bare = upper.strip_prefix("SIG").unwrap_or(&upper);
    SIGNALS.iter().find(|(n, _)| *n == bare).map(|(_, s)| *s)
}

fn pid(pid: u32) -> std::io::Result<libc::pid_t> {
    // 0 and negative pids name process groups; this only ever names one
    // process.
    libc::pid_t::try_from(pid)
        .ok()
        .filter(|p| *p > 0)
        .ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "pid must be positive")
        })
}

pub fn signal(target: u32, signal: libc::c_int) -> std::io::Result<()> {
    let target = pid(target)?;
    // SAFETY: kill(2) with a positive pid signals that one process.
    if unsafe { libc::kill(target, signal) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

pub fn renice(target: u32, nice: i32) -> std::io::Result<()> {
    if !(-20..=19).contains(&nice) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "nice is -20 to 19",
        ));
    }
    let target = pid(target)?;
    // SAFETY: setpriority(2) on one process id.
    if unsafe { libc::setpriority(libc::PRIO_PROCESS, target as libc::id_t, nice) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signal_names_and_numbers() {
        assert_eq!(signal_number("term"), Some(libc::SIGTERM));
        assert_eq!(signal_number("SIGKILL"), Some(libc::SIGKILL));
        assert_eq!(signal_number("9"), Some(9));
        assert_eq!(signal_number("0"), None);
        assert_eq!(signal_number("BOGUS"), None);
    }

    #[test]
    fn a_group_pid_is_refused() {
        assert!(signal(0, libc::SIGTERM).is_err());
        assert!(renice(1, 99).is_err());
    }
}
