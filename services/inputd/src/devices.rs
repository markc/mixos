// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The real devices behind the [`crate::grab`] traits: an evdev keyboard,
//! the uinput re-emitter, and the sysfs attribute naming the active VT.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::io::{AsRawFd, RawFd};
use std::path::Path;

use inputd_schema::{SideModifiers, codes};

use crate::grab::{Console, EV_KEY, EV_SYN, Emit, GrabDevice};

pub const EV_REL: u16 = 2;
/// The highest key code (`KEY_MAX`).
pub const KEY_MAX: u16 = 0x2ff;

/// One `struct input_event` on 64-bit Linux: a timeval (two i64), type,
/// code and value.
pub const EVENT_SIZE: usize = 24;

/// Where the kernel names the active VT (`tty4`), with `sysfs_notify` on
/// every switch.
pub const CONSOLE_ACTIVE: &str = "/sys/class/tty/tty0/active";

// ioctl numbers, asm-generic encoding. EVIOCGRAB = _IOW('E', 0x90, int);
// EVIOCGKEY(96) = _IOC(_IOC_READ, 'E', 0x18, 96); UI_DEV_SETUP =
// _IOW('U', 3, struct uinput_setup) (92 bytes); UI_SET_*BIT = _IOW('U',
// 100..102, int).
const EVIOCGRAB: libc::c_ulong = 0x4004_4590;
const KEY_BITS_LEN: usize = KEY_MAX as usize / 8 + 1;
const EVIOCGKEY: libc::c_ulong = 0x8000_4518 | ((KEY_BITS_LEN as libc::c_ulong) << 16);
const UI_SET_EVBIT: libc::c_ulong = 0x4004_5564;
const UI_SET_KEYBIT: libc::c_ulong = 0x4004_5565;
const UI_SET_RELBIT: libc::c_ulong = 0x4004_5566;
const UI_DEV_SETUP: libc::c_ulong = 0x405c_5503;
const UI_DEV_CREATE: libc::c_ulong = 0x5501;
const UI_DEV_DESTROY: libc::c_ulong = 0x5502;

const BUS_USB: u16 = 0x03;

fn check(ret: libc::c_int) -> io::Result<()> {
    if ret < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// Decode one raw event into `(type, code, value)`.
pub fn decode(event: &[u8; EVENT_SIZE]) -> (u16, u16, i32) {
    (
        u16::from_ne_bytes([event[16], event[17]]),
        u16::from_ne_bytes([event[18], event[19]]),
        i32::from_ne_bytes([event[20], event[21], event[22], event[23]]),
    )
}

/// Encode `(type, code, value)` with a zero timestamp; the kernel stamps
/// uinput events itself.
pub fn encode(kind: u16, code: u16, value: i32) -> [u8; EVENT_SIZE] {
    let mut event = [0u8; EVENT_SIZE];
    event[16..18].copy_from_slice(&kind.to_ne_bytes());
    event[18..20].copy_from_slice(&code.to_ne_bytes());
    event[20..24].copy_from_slice(&value.to_ne_bytes());
    event
}

/// An evdev keyboard node, opened non-blocking.
pub struct EvdevDevice {
    file: File,
}

impl EvdevDevice {
    pub fn open(path: &str) -> io::Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open(path)?;
        Ok(Self { file })
    }

    pub fn fd(&self) -> RawFd {
        self.file.as_raw_fd()
    }

    /// Every event readable now (empty when none is).
    pub fn read_events(&mut self) -> io::Result<Vec<(u16, u16, i32)>> {
        let mut events = Vec::new();
        let mut buffer = [0u8; EVENT_SIZE * 64];
        loop {
            match self.file.read(&mut buffer) {
                Ok(0) => return Err(io::Error::from(io::ErrorKind::UnexpectedEof)),
                Ok(n) => {
                    for chunk in buffer[..n].chunks_exact(EVENT_SIZE) {
                        events.push(decode(chunk.try_into().expect("event-sized chunk")));
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(events),
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => return Err(error),
            }
        }
    }
}

impl GrabDevice for EvdevDevice {
    fn grab(&mut self, on: bool) -> io::Result<()> {
        // SAFETY: an int argument to an ioctl on an fd this struct owns.
        check(unsafe { libc::ioctl(self.fd(), EVIOCGRAB, libc::c_int::from(on)) })
    }

    fn held_modifiers(&mut self) -> io::Result<SideModifiers> {
        let mut bits = [0u8; KEY_BITS_LEN];
        // SAFETY: EVIOCGKEY writes at most KEY_BITS_LEN bytes, the size
        // encoded in the request, into this caller-owned buffer.
        check(unsafe { libc::ioctl(self.fd(), EVIOCGKEY, bits.as_mut_ptr()) })?;
        let down = |code: u16| bits[usize::from(code / 8)] & (1 << (code % 8)) != 0;
        let mut mods = SideModifiers::NONE;
        for code in [
            codes::KEY_LEFTCTRL,
            codes::KEY_RIGHTCTRL,
            codes::KEY_LEFTSHIFT,
            codes::KEY_RIGHTSHIFT,
            codes::KEY_LEFTALT,
            codes::KEY_RIGHTALT,
            codes::KEY_LEFTMETA,
            codes::KEY_RIGHTMETA,
        ] {
            mods.set(code, down(code));
        }
        Ok(mods)
    }

    fn drain(&mut self) -> io::Result<()> {
        self.read_events().map(drop)
    }
}

/// An owned uinput device, destroyed on drop (and by the kernel when the
/// process exits, which also releases every key it holds).
pub struct UinputDevice {
    file: File,
}

impl UinputDevice {
    /// The re-emitter for the grab path: every key code.
    pub fn keyboard() -> io::Result<Self> {
        Self::create(b"inputd virtual keyboard", 1, &[])
    }

    /// The injector for `input.key` and `input.pointer.*`: keys, buttons
    /// and relative axes.
    pub fn pointer() -> io::Result<Self> {
        const REL_X: u16 = 0x00;
        const REL_Y: u16 = 0x01;
        const REL_HWHEEL: u16 = 0x06;
        const REL_WHEEL: u16 = 0x08;
        Self::create(b"inputd virtual pointer", 2, &[REL_X, REL_Y, REL_WHEEL, REL_HWHEEL])
    }

    fn create(name: &[u8], product: u16, axes: &[u16]) -> io::Result<Self> {
        let file = OpenOptions::new()
            .write(true)
            .custom_flags(libc::O_CLOEXEC)
            .open("/dev/uinput")?;
        let fd = file.as_raw_fd();
        let mut setup = UinputSetup {
            id: InputId {
                bustype: BUS_USB,
                vendor: 0x1d5b,
                product,
                version: 1,
            },
            name: [0u8; 80],
            ff_effects_max: 0,
        };
        setup.name[..name.len()].copy_from_slice(name);
        // SAFETY: int arguments, and a pointer to a correctly laid out
        // uinput_setup that lives across the call, on an fd we own.
        unsafe {
            check(libc::ioctl(fd, UI_SET_EVBIT, libc::c_int::from(EV_SYN)))?;
            check(libc::ioctl(fd, UI_SET_EVBIT, libc::c_int::from(EV_KEY)))?;
            for code in 0..=KEY_MAX {
                check(libc::ioctl(fd, UI_SET_KEYBIT, libc::c_int::from(code)))?;
            }
            if !axes.is_empty() {
                check(libc::ioctl(fd, UI_SET_EVBIT, libc::c_int::from(EV_REL)))?;
                for &code in axes {
                    check(libc::ioctl(fd, UI_SET_RELBIT, libc::c_int::from(code)))?;
                }
            }
            check(libc::ioctl(
                fd,
                UI_DEV_SETUP,
                &setup as *const UinputSetup as *const libc::c_void,
            ))?;
            check(libc::ioctl(fd, UI_DEV_CREATE))?;
        }
        Ok(Self { file })
    }

    #[cfg(test)]
    pub fn with_test_file(file: File) -> Self {
        Self { file }
    }

    pub fn write_event(&mut self, event: &[u8; EVENT_SIZE]) -> io::Result<()> {
        self.file.write_all(event)
    }
}

impl Emit for UinputDevice {
    fn emit(&mut self, kind: u16, code: u16, value: i32) -> io::Result<()> {
        self.write_event(&encode(kind, code, value))
    }
}

impl Drop for UinputDevice {
    fn drop(&mut self) {
        // SAFETY: a no-argument ioctl on an fd we own; best effort, the fd
        // close removes the device anyway.
        unsafe {
            libc::ioctl(self.file.as_raw_fd(), UI_DEV_DESTROY);
        }
    }
}

#[repr(C)]
struct InputId {
    bustype: u16,
    vendor: u16,
    product: u16,
    version: u16,
}

#[repr(C)]
struct UinputSetup {
    id: InputId,
    name: [u8; 80],
    ff_effects_max: u32,
}

/// The active-VT attribute. The caller polls [`SysfsConsole::fd`] for
/// `POLLPRI | POLLERR` and calls [`Console::active`] on every wake, which
/// re-reads from the start (and re-arms the notification).
pub struct SysfsConsole {
    file: File,
}

impl SysfsConsole {
    pub fn open(path: &Path) -> io::Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_CLOEXEC)
            .open(path)?;
        Ok(Self { file })
    }

    pub fn fd(&self) -> RawFd {
        self.file.as_raw_fd()
    }
}

impl Console for SysfsConsole {
    fn active(&mut self) -> io::Result<Option<u16>> {
        self.file.rewind()?;
        let mut text = String::new();
        self.file.read_to_string(&mut text)?;
        Ok(parse_active(&text))
    }
}

/// `tty4\n` -> 4. Anything else (a serial console, an empty read) -> None.
pub fn parse_active(text: &str) -> Option<u16> {
    let number = text.trim().strip_prefix("tty")?;
    if number.is_empty() || !number.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    number.parse().ok().filter(|&vt| vt > 0)
}

/// The first `/dev/input/eventN` whose device Name in
/// `/proc/bus/input/devices` is `name`, so a unit can name keyd's virtual
/// keyboard without a fixed node (which shifts across restarts).
pub fn device_by_name(devices: &str, name: &str) -> Option<String> {
    for block in devices.split("\n\n") {
        let matches = block.lines().any(|line| {
            line.strip_prefix("N: Name=")
                .is_some_and(|rest| rest.trim().trim_matches('"') == name)
        });
        if !matches {
            continue;
        }
        for line in block.lines() {
            if let Some(rest) = line.strip_prefix("H: Handlers=")
                && let Some(node) = rest.split_whitespace().find(|token| {
                    token
                        .strip_prefix("event")
                        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
                })
            {
                return Some(format!("/dev/input/{node}"));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_encode_and_decode() {
        let event = encode(EV_KEY, 106, -3);
        assert_eq!(&event[..16], &[0; 16]);
        assert_eq!(decode(&event), (EV_KEY, 106, -3));
    }

    #[test]
    fn ioctl_numbers_match_the_kernel_encoding() {
        // _IOC(dir, type, nr, size) = dir<<30 | size<<16 | type<<8 | nr.
        let ioc = |dir: u64, kind: u8, nr: u8, size: u64| {
            (dir << 30 | size << 16 | u64::from(kind) << 8 | u64::from(nr)) as libc::c_ulong
        };
        assert_eq!(EVIOCGRAB, ioc(1, b'E', 0x90, 4));
        assert_eq!(EVIOCGKEY, ioc(2, b'E', 0x18, 96));
        assert_eq!(UI_DEV_SETUP, ioc(1, b'U', 3, std::mem::size_of::<UinputSetup>() as u64));
        assert_eq!(UI_SET_EVBIT, ioc(1, b'U', 100, 4));
        assert_eq!(UI_SET_KEYBIT, ioc(1, b'U', 101, 4));
        assert_eq!(UI_SET_RELBIT, ioc(1, b'U', 102, 4));
    }

    #[test]
    fn active_console_text_parses() {
        assert_eq!(parse_active("tty4\n"), Some(4));
        assert_eq!(parse_active("tty12"), Some(12));
        for bad in ["", "\n", "tty", "tty0", "ttyS0", "tty-1", "pts/3", "4"] {
            assert_eq!(parse_active(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn the_console_is_re_read_from_the_start_on_each_wake() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("active");
        std::fs::write(&path, "tty1\n").unwrap();
        let mut console = SysfsConsole::open(&path).unwrap();
        assert_eq!(console.active().unwrap(), Some(1));
        // Same open file: the next read must start at offset 0 again.
        std::fs::write(&path, "tty4\n").unwrap();
        assert_eq!(console.active().unwrap(), Some(4));
        assert_eq!(console.active().unwrap(), Some(4));
    }

    #[test]
    fn devices_are_found_by_name() {
        let devices = "I: Bus=0011 Vendor=0001 Product=0001 Version=ab41\n\
N: Name=\"AT Translated Set 2 keyboard\"\n\
H: Handlers=sysrq kbd leds event3\n\
\n\
I: Bus=0019 Vendor=0fac Product=0ade Version=0001\n\
N: Name=\"keyd virtual keyboard\"\n\
H: Handlers=sysrq kbd leds event19\n";
        assert_eq!(
            device_by_name(devices, "keyd virtual keyboard").as_deref(),
            Some("/dev/input/event19")
        );
        assert_eq!(device_by_name(devices, "keyd virtual").as_deref(), None);
        assert_eq!(device_by_name(devices, "inputd virtual keyboard"), None);
    }
}
