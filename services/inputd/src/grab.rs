// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The VT-aware grab: when to hold `EVIOCGRAB`, and what to do with each
//! event while it is held.
//!
//! [`Grabber`] is a state machine over three small traits, so every
//! transition is tested with fakes and no real device:
//!
//! - [`GrabDevice`]: the keyboard being intercepted (grab, held modifiers,
//!   discard queued events);
//! - [`Emit`]: the uinput re-emitter, created **once** at startup and kept
//!   for the life of the process (a compositor inside a container cannot
//!   see a device created after it started);
//! - [`Console`]: which VT is active.
//!
//! With a VT set, inputd grabs only while the active console is one of
//! them. On switch-away it releases on the re-emitter every key latched as
//! emitted, forgets every latch, then drops the grab. On switch-to it grabs
//! again, retrying with bounded exponential backoff while another holder
//! still has the device (`EBUSY`); a repeat or release whose press it did
//! not see under this grab is dropped. With no VT set it grabs always.
//!
//! While not grabbed, events read from the device are discarded: the
//! compositor of the active VT reads them itself.

use std::io;
use std::time::{Duration, Instant};

use inputd_core::{Fire, Interceptor, Resolver};
use inputd_schema::SideModifiers;

/// `input-event-codes.h` event types and the report code.
pub const EV_SYN: u16 = 0;
pub const EV_KEY: u16 = 1;
pub const SYN_REPORT: u16 = 0;

/// The first wait after a busy grab; each further wait doubles.
pub const GRAB_RETRY_FIRST: Duration = Duration::from_millis(50);
/// The longest wait between grab attempts.
pub const GRAB_RETRY_CAP: Duration = Duration::from_secs(2);
/// At the cap, log one failed attempt in this many.
const GRAB_RETRY_LOG_EVERY: u32 = 30;

/// The keyboard being intercepted.
pub trait GrabDevice {
    /// `EVIOCGRAB` on (`true`) or off.
    fn grab(&mut self, on: bool) -> io::Result<()>;
    /// The modifier keys physically down now (`EVIOCGKEY`).
    fn held_modifiers(&mut self) -> io::Result<SideModifiers>;
    /// Discard every event already queued on the device.
    fn drain(&mut self) -> io::Result<()>;
}

/// The re-emitter.
pub trait Emit {
    fn emit(&mut self, kind: u16, code: u16, value: i32) -> io::Result<()>;
}

/// The active console.
pub trait Console {
    /// The active VT number, or `None` when it cannot be told.
    fn active(&mut self) -> io::Result<Option<u16>>;
}

/// Parse `--vt 1,4`: VT numbers 1 to 63, at least one.
pub fn parse_vts(values: &[u16]) -> Result<Vec<u16>, String> {
    if values.is_empty() {
        return Err("--vt needs at least one VT number".into());
    }
    if let Some(bad) = values.iter().find(|&&vt| !(1..=63).contains(&vt)) {
        return Err(format!("--vt {bad}: a VT number is 1 to 63"));
    }
    let mut vts = values.to_vec();
    vts.sort_unstable();
    vts.dedup();
    Ok(vts)
}

/// The grab state machine.
pub struct Grabber<D, E> {
    device: D,
    out: E,
    /// `None`: grab always.
    vts: Option<Vec<u16>>,
    wanted: bool,
    grabbed: bool,
    /// The next attempt and the wait that led to it.
    retry: Option<(Instant, Duration)>,
    failures: u32,
    keys: Interceptor,
}

impl<D: GrabDevice, E: Emit> Grabber<D, E> {
    pub fn new(device: D, out: E, vts: Option<Vec<u16>>) -> Self {
        Self {
            wanted: vts.is_none(),
            device,
            out,
            vts,
            grabbed: false,
            retry: None,
            failures: 0,
            keys: Interceptor::new(),
        }
    }

    pub fn is_grabbed(&self) -> bool {
        self.grabbed
    }

    /// True when the grab follows the active console.
    pub fn follows_console(&self) -> bool {
        self.vts.is_some()
    }

    pub fn device_mut(&mut self) -> &mut D {
        &mut self.device
    }

    /// Grab now if a grab is wanted (the start without a VT set).
    pub fn start(&mut self, now: Instant) -> io::Result<()> {
        self.settle(now)
    }

    /// The active console changed (or was read at startup).
    pub fn console(&mut self, active: Option<u16>, now: Instant) -> io::Result<()> {
        if let Some(vts) = &self.vts {
            self.wanted = active.is_some_and(|vt| vts.contains(&vt));
        }
        self.settle(now)
    }

    /// When the next grab attempt is due, if one is pending.
    pub fn deadline(&self) -> Option<Instant> {
        self.retry.map(|(at, _)| at)
    }

    /// Run a due grab attempt.
    pub fn tick(&mut self, now: Instant) -> io::Result<()> {
        match self.retry {
            Some((at, _)) if now >= at && self.wanted && !self.grabbed => self.try_grab(now),
            _ => Ok(()),
        }
    }

    /// Handle one event read from the device. Returns a verb to fire.
    pub fn event(
        &mut self,
        resolver: &Resolver,
        kind: u16,
        code: u16,
        value: i32,
    ) -> io::Result<Option<Fire>> {
        if !self.grabbed {
            return Ok(None);
        }
        match kind {
            // Report frames keep the re-emitted stream well formed.
            EV_SYN => self.out.emit(kind, code, value).map(|()| None),
            EV_KEY => {
                let step = self.keys.key(resolver, code, value);
                if step.emit {
                    self.out.emit(kind, code, value)?;
                }
                Ok(step.fire)
            }
            // Scan codes and the rest: the re-emitter advertises only keys.
            _ => Ok(None),
        }
    }

    /// Release the grab (shutdown), with the switch-away clean-up.
    pub fn shutdown(&mut self) -> io::Result<()> {
        self.wanted = false;
        self.settle(Instant::now())
    }

    fn settle(&mut self, now: Instant) -> io::Result<()> {
        if self.wanted {
            if !self.grabbed && self.retry.is_none() {
                return self.try_grab(now);
            }
            return Ok(());
        }
        self.retry = None;
        self.failures = 0;
        if self.grabbed {
            return self.release();
        }
        Ok(())
    }

    fn try_grab(&mut self, now: Instant) -> io::Result<()> {
        match self.device.grab(true) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::ResourceBusy => {
                let wait = match self.retry {
                    None => GRAB_RETRY_FIRST,
                    Some((_, last)) => (last * 2).min(GRAB_RETRY_CAP),
                };
                self.failures += 1;
                if wait < GRAB_RETRY_CAP || self.failures % GRAB_RETRY_LOG_EVERY == 1 {
                    tracing::warn!(
                        attempt = self.failures,
                        retry_ms = wait.as_millis() as u64,
                        "keyboard grab busy (another holder); retrying"
                    );
                }
                self.retry = Some((now + wait, wait));
                return Ok(());
            }
            Err(error) => return Err(error),
        }
        self.grabbed = true;
        self.retry = None;
        self.failures = 0;
        let held = self.device.held_modifiers().unwrap_or_else(|error| {
            tracing::warn!(%error, "could not read held modifiers; assuming none");
            SideModifiers::NONE
        });
        self.keys.begin(held);
        // Events queued before the grab took effect were delivered to the
        // compositor as well; replaying them would type them twice.
        if let Err(error) = self.device.drain() {
            tracing::warn!(%error, "could not discard queued events");
        }
        tracing::info!("keyboard grab held");
        Ok(())
    }

    /// Switch-away: release every emitted latched key on the re-emitter,
    /// forget the latches, then drop the grab. The grab is dropped even if
    /// a release cannot be written.
    fn release(&mut self) -> io::Result<()> {
        let held = self.keys.end();
        let mut written = Ok(());
        for &code in &held {
            if written.is_ok() {
                written = self.out.emit(EV_KEY, code, 0);
            }
        }
        if written.is_ok() && !held.is_empty() {
            written = self.out.emit(EV_SYN, SYN_REPORT, 0);
        }
        self.grabbed = false;
        let ungrabbed = self.device.grab(false);
        tracing::info!(released = held.len(), "keyboard grab dropped");
        written.and(ungrabbed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use inputd_core::default_keymap;
    use inputd_schema::{
        ActionId, BindingScope, PhysicalBinding, PhysicalStroke, RepeatPolicy, codes::*,
    };
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::rc::Rc;

    const KEY_A: u16 = 30;
    const KEY_F5: u16 = 63;

    /// One ordered record of every device call and every emitted event.
    type Log = Rc<RefCell<Vec<String>>>;

    struct FakeDevice {
        log: Log,
        busy: u32,
        held: SideModifiers,
    }

    impl GrabDevice for FakeDevice {
        fn grab(&mut self, on: bool) -> io::Result<()> {
            if on && self.busy > 0 {
                self.busy -= 1;
                self.log.borrow_mut().push("grab busy".into());
                return Err(io::Error::from(io::ErrorKind::ResourceBusy));
            }
            self.log.borrow_mut().push(format!("grab {}", u8::from(on)));
            Ok(())
        }
        fn held_modifiers(&mut self) -> io::Result<SideModifiers> {
            Ok(self.held)
        }
        fn drain(&mut self) -> io::Result<()> {
            self.log.borrow_mut().push("drain".into());
            Ok(())
        }
    }

    struct FakeOut {
        log: Log,
        fail: bool,
    }

    impl Emit for FakeOut {
        fn emit(&mut self, kind: u16, code: u16, value: i32) -> io::Result<()> {
            if self.fail {
                return Err(io::Error::other("uinput gone"));
            }
            self.log.borrow_mut().push(format!("emit {kind} {code} {value}"));
            Ok(())
        }
    }

    /// The console as a script of active VTs, behind the real trait.
    struct FakeConsole(VecDeque<Option<u16>>);

    impl Console for FakeConsole {
        fn active(&mut self) -> io::Result<Option<u16>> {
            Ok(self.0.pop_front().expect("console script ran out"))
        }
    }

    struct Rig {
        grabber: Grabber<FakeDevice, FakeOut>,
        resolver: Resolver,
        log: Log,
        fired: Vec<&'static str>,
        now: Instant,
    }

    impl Rig {
        fn new(vts: Option<Vec<u16>>, busy: u32) -> Self {
            let log: Log = Rc::default();
            let device = FakeDevice {
                log: log.clone(),
                busy,
                held: SideModifiers::NONE,
            };
            let out = FakeOut {
                log: log.clone(),
                fail: false,
            };
            Self {
                grabber: Grabber::new(device, out, vts),
                resolver: Resolver::new(default_keymap()),
                log,
                fired: Vec::new(),
                now: Instant::now(),
            }
        }

        fn key(&mut self, code: u16, value: i32) -> &mut Self {
            let fire = self.grabber.event(&self.resolver, EV_KEY, code, value).unwrap();
            if let Some(fire) = fire {
                self.fired.push(fire.verb.as_str());
            }
            self
        }

        fn console(&mut self, console: &mut FakeConsole) {
            let active = console.active().unwrap();
            self.grabber.console(active, self.now).unwrap();
        }

        fn take_log(&self) -> Vec<String> {
            std::mem::take(&mut *self.log.borrow_mut())
        }
    }

    fn passthrough(code: u16, modifiers: SideModifiers) -> PhysicalBinding {
        PhysicalBinding {
            stroke: PhysicalStroke { code, modifiers },
            action: ActionId::from_static("input.passthrough"),
            args: None,
            service: None,
            scope: BindingScope::default(),
            repeat: RepeatPolicy::Ignore,
            passthrough: true,
        }
    }

    #[test]
    fn vt_numbers_are_checked_and_deduplicated() {
        assert_eq!(parse_vts(&[4, 1, 4]), Ok(vec![1, 4]));
        assert!(parse_vts(&[]).is_err());
        assert!(parse_vts(&[0]).is_err());
        assert!(parse_vts(&[64]).is_err());
    }

    #[test]
    fn without_a_vt_set_it_grabs_at_start_and_ignores_the_console() {
        let mut rig = Rig::new(None, 0);
        assert!(!rig.grabber.follows_console());
        rig.grabber.start(rig.now).unwrap();
        assert!(rig.grabber.is_grabbed());
        assert_eq!(rig.take_log(), ["grab 1", "drain"]);
        rig.grabber.console(Some(3), rig.now).unwrap();
        assert!(rig.grabber.is_grabbed(), "no VT set: the console is irrelevant");
        assert!(rig.take_log().is_empty());
    }

    #[test]
    fn it_grabs_only_while_a_listed_vt_is_active() {
        let mut rig = Rig::new(Some(vec![1]), 0);
        let mut console = FakeConsole(VecDeque::from([Some(4), Some(1), Some(1), Some(3), None]));
        rig.console(&mut console);
        assert!(!rig.grabber.is_grabbed(), "starts on VT4: no grab");
        assert!(rig.take_log().is_empty());
        rig.console(&mut console);
        assert!(rig.grabber.is_grabbed());
        assert_eq!(rig.take_log(), ["grab 1", "drain"]);
        rig.console(&mut console);
        assert!(rig.take_log().is_empty(), "a wake on the same VT changes nothing");
        rig.console(&mut console);
        assert!(!rig.grabber.is_grabbed());
        assert_eq!(rig.take_log(), ["grab 0"]);
        rig.console(&mut console);
        assert!(!rig.grabber.is_grabbed(), "an unreadable console never grabs");
    }

    #[test]
    fn events_while_not_grabbed_are_discarded() {
        let mut rig = Rig::new(Some(vec![1]), 0);
        rig.grabber.console(Some(4), rig.now).unwrap();
        rig.key(KEY_A, 1).key(KEY_F5, 1).key(KEY_A, 0);
        let fire = rig.grabber.event(&rig.resolver, EV_SYN, SYN_REPORT, 0).unwrap();
        assert!(fire.is_none());
        assert!(rig.take_log().is_empty(), "nothing reached the re-emitter");
        assert!(rig.fired.is_empty());
    }

    #[test]
    fn switch_away_with_held_keys_releases_then_ungrabs() {
        let mut rig = Rig::new(Some(vec![1]), 0);
        rig.resolver
            .bind_physical(passthrough(KEY_UP, SideModifiers::RIGHT_SHIFT))
            .unwrap();
        rig.grabber.console(Some(1), rig.now).unwrap();
        rig.take_log();
        // Held: a letter, a bare F5 (unbound, so swallowed) and Right Shift +
        // ↑ (the person's passthrough).
        rig.key(KEY_A, 1).key(KEY_F5, 1).key(KEY_RIGHTSHIFT, 1).key(KEY_UP, 1);
        assert_eq!(rig.take_log(), ["emit 1 30 1", "emit 1 54 1", "emit 1 103 1"]);
        rig.grabber.console(Some(4), rig.now).unwrap();
        assert_eq!(
            rig.take_log(),
            ["emit 1 30 0", "emit 1 54 0", "emit 1 103 0", "emit 0 0 0", "grab 0"],
            "synthetic releases for emitted keys only, then the grab goes"
        );
        assert!(!rig.grabber.is_grabbed());
    }

    #[test]
    fn switch_back_drops_orphan_releases_and_repeats() {
        let mut rig = Rig::new(Some(vec![1]), 0);
        rig.grabber.console(Some(1), rig.now).unwrap();
        rig.key(KEY_A, 1).key(KEY_RIGHTCTRL, 1);
        rig.grabber.console(Some(4), rig.now).unwrap();
        // On VT4 the keys are still physically held, then the person
        // switches back while holding them.
        rig.grabber.console(Some(1), rig.now).unwrap();
        rig.take_log();
        rig.key(KEY_A, 2).key(KEY_A, 0).key(KEY_RIGHTCTRL, 0);
        assert!(rig.take_log().is_empty(), "orphans reached the re-emitter");
        // A fresh press under the new grab is ordinary again.
        rig.key(KEY_A, 1).key(KEY_A, 0);
        assert_eq!(rig.take_log(), ["emit 1 30 1", "emit 1 30 0"]);
    }

    #[test]
    fn a_held_modifier_at_grab_still_forms_a_user_key_chord() {
        let mut rig = Rig::new(Some(vec![1]), 0);
        rig.grabber.device_mut().held = SideModifiers::RIGHT_CTRL;
        rig.grabber.console(Some(1), rig.now).unwrap();
        rig.take_log();
        rig.key(KEY_RIGHT, 1).key(KEY_RIGHT, 0).key(KEY_RIGHTCTRL, 0);
        assert!(rig.take_log().is_empty(), "Right Ctrl + → is swallowed; its Ctrl release is an orphan");
    }

    #[test]
    fn a_rebind_while_held_keeps_the_press_decision_across_the_grab() {
        let mut rig = Rig::new(None, 0);
        rig.grabber.start(rig.now).unwrap();
        rig.take_log();
        rig.key(KEY_F5, 1);
        rig.resolver
            .bind_physical(passthrough(KEY_F5, SideModifiers::NONE))
            .unwrap();
        rig.key(KEY_F5, 2).key(KEY_F5, 0);
        assert!(rig.take_log().is_empty(), "swallowed at press, swallowed to release");
        rig.key(KEY_F5, 1).key(KEY_F5, 0);
        assert_eq!(rig.take_log(), ["emit 1 63 1", "emit 1 63 0"]);
    }

    #[test]
    fn a_busy_grab_backs_off_exponentially_to_the_cap() {
        let mut rig = Rig::new(Some(vec![1]), 7);
        let t0 = rig.now;
        rig.grabber.console(Some(1), t0).unwrap();
        let mut waits = Vec::new();
        let mut at = t0;
        while !rig.grabber.is_grabbed() {
            let due = rig.grabber.deadline().expect("a retry is pending");
            waits.push(due - at);
            // Nothing happens before the deadline.
            rig.grabber.tick(due - Duration::from_millis(1)).unwrap();
            assert!(!rig.grabber.is_grabbed());
            at = due;
            rig.grabber.tick(due).unwrap();
        }
        let ms: Vec<u128> = waits.iter().map(Duration::as_millis).collect();
        assert_eq!(ms, [50, 100, 200, 400, 800, 1600, 2000]);
        assert_eq!(rig.grabber.deadline(), None);
        assert_eq!(rig.take_log().iter().filter(|l| *l == "grab busy").count(), 7);
    }

    #[test]
    fn switching_away_cancels_a_pending_retry() {
        let mut rig = Rig::new(Some(vec![1]), 1);
        rig.grabber.console(Some(1), rig.now).unwrap();
        assert!(rig.grabber.deadline().is_some());
        rig.grabber.console(Some(4), rig.now).unwrap();
        assert_eq!(rig.grabber.deadline(), None);
        rig.grabber.tick(rig.now + GRAB_RETRY_CAP).unwrap();
        assert!(!rig.grabber.is_grabbed());
        // Back on VT1 the first attempt is immediate again.
        rig.grabber.console(Some(1), rig.now).unwrap();
        assert!(rig.grabber.is_grabbed());
    }

    #[test]
    fn other_grab_errors_are_reported() {
        struct Gone;
        impl GrabDevice for Gone {
            fn grab(&mut self, _: bool) -> io::Result<()> {
                Err(io::Error::from_raw_os_error(19)) // ENODEV
            }
            fn held_modifiers(&mut self) -> io::Result<SideModifiers> {
                Ok(SideModifiers::NONE)
            }
            fn drain(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let out = FakeOut {
            log: Rc::default(),
            fail: false,
        };
        let mut grabber = Grabber::new(Gone, out, None);
        assert!(grabber.start(Instant::now()).is_err());
        assert!(!grabber.is_grabbed());
    }

    #[test]
    fn the_grab_is_dropped_even_when_a_release_cannot_be_written() {
        let mut rig = Rig::new(Some(vec![1]), 0);
        rig.grabber.console(Some(1), rig.now).unwrap();
        rig.key(KEY_A, 1);
        rig.take_log();
        rig.grabber.out.fail = true;
        assert!(rig.grabber.console(Some(4), rig.now).is_err());
        assert!(!rig.grabber.is_grabbed());
        assert_eq!(rig.take_log(), ["grab 0"]);
    }

    #[test]
    fn one_re_emitter_serves_every_grab() {
        // The re-emitter is owned for the grabber's whole life: switching
        // away and back writes to the same device, never a new one.
        let mut rig = Rig::new(Some(vec![1]), 0);
        for _ in 0..3 {
            rig.grabber.console(Some(1), rig.now).unwrap();
            rig.key(KEY_A, 1);
            rig.grabber.console(Some(4), rig.now).unwrap();
        }
        let log = rig.take_log();
        assert_eq!(log.iter().filter(|l| *l == "emit 1 30 1").count(), 3);
        assert_eq!(log.iter().filter(|l| *l == "emit 1 30 0").count(), 3);
    }

    #[test]
    fn shutdown_releases_like_a_switch_away() {
        let mut rig = Rig::new(None, 0);
        rig.grabber.start(rig.now).unwrap();
        rig.key(KEY_A, 1);
        rig.take_log();
        rig.grabber.shutdown().unwrap();
        assert_eq!(rig.take_log(), ["emit 1 30 0", "emit 0 0 0", "grab 0"]);
    }
}
