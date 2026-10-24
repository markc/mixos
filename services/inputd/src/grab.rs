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
//! still has the device (`EBUSY`). With no VT set it grabs always.
//!
//! A grab is only taken while **no key is down**: a key held across the
//! grab would stay down in the compositor, which saw its press on the
//! original device and would never see its release. While a key is held
//! inputd waits ungrabbed and tries again at each report from the device.
//! A repeat or release whose press was not seen under the grab is dropped
//! (and logged; with the quiet-grab rule it should not happen).
//!
//! While not grabbed, events read from the device are discarded: the
//! compositor of the active VT reads them itself.

use std::io;
use std::time::{Duration, Instant};

use inputd_core::{Fire, Interceptor, Resolver};
use inputd_schema::{SideModifiers, codes};

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

/// Events were lost: discard up to and including the next `SYN_REPORT`,
/// then read the device state afresh.
pub const SYN_DROPPED: u16 = 3;

/// Bytes in a `KEY_MAX` key bitmap (`EVIOCGKEY`).
pub const KEY_BITS_LEN: usize = 0x2ff / 8 + 1;

/// Which keys are physically down: an `EVIOCGKEY` bitmap.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyState(pub [u8; KEY_BITS_LEN]);

impl KeyState {
    #[cfg(test)]
    pub const NONE: Self = Self([0; KEY_BITS_LEN]);

    /// The state with exactly `codes` down.
    #[cfg(test)]
    pub fn of(codes: &[u16]) -> Self {
        let mut bits = [0; KEY_BITS_LEN];
        for &code in codes {
            if let Some(byte) = bits.get_mut(usize::from(code / 8)) {
                *byte |= 1 << (code % 8);
            }
        }
        Self(bits)
    }

    /// Every key down, ascending.
    pub fn down_codes(&self) -> Vec<u16> {
        (0..=0x2ffu16).filter(|&code| self.is_down(code)).collect()
    }

    pub fn any_down(&self) -> bool {
        self.0.iter().any(|&byte| byte != 0)
    }

    pub fn is_down(&self, code: u16) -> bool {
        self.0
            .get(usize::from(code / 8))
            .is_some_and(|byte| byte & (1 << (code % 8)) != 0)
    }

    /// The side-specific modifiers down.
    pub fn modifiers(&self) -> SideModifiers {
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
            mods.set(code, self.is_down(code));
        }
        mods
    }
}

/// The keyboard being intercepted.
pub trait GrabDevice {
    /// `EVIOCGRAB` on (`true`) or off.
    fn grab(&mut self, on: bool) -> io::Result<()>;
    /// The keys physically down now (`EVIOCGKEY`).
    fn key_state(&mut self) -> io::Result<KeyState>;
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
    /// Between a `SYN_DROPPED` and the `SYN_REPORT` that ends it.
    dropping: bool,
    /// A grab is wanted but a key is down: waiting, ungrabbed, for the
    /// keyboard to be quiet.
    waiting: bool,
    /// A key snapshot was just taken: the rest of the batch being processed
    /// predates it and is discarded.
    cut: bool,
    /// Something was written to the re-emitter since its last report.
    dirty: bool,
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
            dropping: false,
            waiting: false,
            cut: false,
            dirty: false,
            keys: Interceptor::new(),
        }
    }

    #[cfg(test)]
    fn is_grabbed(&self) -> bool {
        self.grabbed
    }

    /// True when the grab follows the active console.
    #[cfg(test)]
    fn follows_console(&self) -> bool {
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

    /// Handle one batch of events read from the device, in order. Returns
    /// the commands to fire. When a key snapshot is taken mid-batch (the grab
    /// is taken, or dropped events are recovered), the rest of the batch
    /// predates the snapshot and is discarded: processing resumes strictly
    /// after it.
    pub fn events(
        &mut self,
        resolver: &Resolver,
        batch: &[(u16, u16, i32)],
    ) -> io::Result<Vec<Fire>> {
        self.cut = false;
        let mut fired = Vec::new();
        for &(kind, code, value) in batch {
            fired.extend(self.event(resolver, kind, code, value)?);
            if std::mem::take(&mut self.cut) {
                break;
            }
        }
        Ok(fired)
    }

    /// Handle one event read from the device. Returns a command to fire.
    pub fn event(
        &mut self,
        resolver: &Resolver,
        kind: u16,
        code: u16,
        value: i32,
    ) -> io::Result<Option<Fire>> {
        if !self.grabbed {
            // Waiting for a quiet keyboard, ungrabbed: the compositor reads
            // these events itself. Each report may be the last key going up.
            if self.waiting && self.wanted && kind == EV_SYN && code == SYN_REPORT {
                self.try_grab(Instant::now())?;
            }
            return Ok(None);
        }
        // The kernel's overflow contract: after SYN_DROPPED, ignore events
        // up to and including the next SYN_REPORT, then re-read the state.
        // The marker itself is not forwarded.
        if kind == EV_SYN && code == SYN_DROPPED {
            self.dropping = true;
            return Ok(None);
        }
        if self.dropping {
            if kind == EV_SYN && code == SYN_REPORT {
                self.dropping = false;
                self.resync()?;
            }
            return Ok(None);
        }
        match kind {
            // Report frames keep the re-emitted stream well formed.
            EV_SYN => self.put(kind, code, value).map(|()| None),
            EV_KEY => {
                let step = self.keys.key(resolver, code, value);
                if step.orphan {
                    // A grab is only taken with no key down, so this should
                    // never happen; the drop is the safety net.
                    tracing::warn!(
                        code,
                        value,
                        "key event with no press seen under this grab; dropped"
                    );
                }
                if step.emit {
                    self.put(kind, code, value)?;
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
        self.waiting = false;
        if self.grabbed {
            return self.release();
        }
        Ok(())
    }

    /// Take the grab, but only at a quiet moment. A key held when the grab
    /// is taken was pressed on the original device, where the compositor
    /// saw it; its release would come only to inputd, and the compositor
    /// would keep the key down (a release on the re-emitter does not reach
    /// that device's state). So: no key down before the grab, and none
    /// after it once queued events are discarded; otherwise drop the grab
    /// and wait, ungrabbed, for a report with nothing held.
    ///
    /// Known limits, inherent to taking a grab from outside the compositor
    /// (README "Known limits"); all happen only at acquisition, at startup
    /// or switch-to, with keys in motion:
    /// - a key pressed before the grab and released inside the
    ///   check–grab–drain window stays down in the compositor: the release
    ///   is consumed under the grab, and EVIOCGRAB has no way to hand it
    ///   back on the original device;
    /// - a key tapped (pressed and released) between the grab and the
    ///   drain is lost to both the compositor and inputd, with nothing
    ///   left held;
    /// - while waiting for a quiet keyboard inputd holds no grab, so user
    ///   keys pressed meanwhile reach apps unclassified.
    ///
    /// None can be closed from here. They close where the compositor
    /// enforces the user keys itself (compd's user-key layer).
    fn try_grab(&mut self, now: Instant) -> io::Result<()> {
        let before = self.device.key_state()?;
        if before.any_down() {
            self.wait(&before, now);
            return Ok(());
        }
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
        self.retry = None;
        self.failures = 0;
        // Events queued before the grab took effect were delivered to the
        // compositor as well; replaying them would type them twice. Discard
        // them first, then snapshot the keys: every event after the snapshot
        // is processed. On failure the grab is dropped again and the error
        // goes up: never guess the key state.
        let after = match self.device.drain().and_then(|()| self.device.key_state()) {
            Ok(state) => state,
            Err(error) => {
                let _ = self.device.grab(false);
                return Err(error);
            }
        };
        if after.any_down() {
            // A key went down between the check and the grab.
            self.device.grab(false)?;
            self.wait(&after, now);
            return Ok(());
        }
        self.grabbed = true;
        self.waiting = false;
        self.dropping = false;
        self.cut = true;
        self.keys.begin(SideModifiers::NONE);
        tracing::info!("keyboard grab held");
        Ok(())
    }

    /// Wait, ungrabbed, for a quiet keyboard. Each report from the device
    /// triggers another attempt, and so does a bounded deadline on the
    /// grab-retry schedule (doubling to the cap): another grab holder can
    /// take the last release and the report with it, and inputd must not
    /// wait for a report that never comes.
    fn wait(&mut self, state: &KeyState, now: Instant) {
        if !self.waiting {
            tracing::info!(held = ?state.down_codes(), "keys held; the grab waits for the keyboard to be quiet");
        }
        self.waiting = true;
        let wait = match self.retry {
            None => GRAB_RETRY_FIRST,
            Some((_, last)) => (last * 2).min(GRAB_RETRY_CAP),
        };
        self.retry = Some((now + wait, wait));
    }

    /// Write one event to the re-emitter, tracking whether anything is
    /// waiting for a report.
    fn put(&mut self, kind: u16, code: u16, value: i32) -> io::Result<()> {
        self.out.emit(kind, code, value)?;
        self.dirty = !(kind == EV_SYN && code == SYN_REPORT);
        Ok(())
    }

    /// End the re-emitted frame if anything was written since the last
    /// report, so no event is left buffered in the re-emitter.
    fn flush(&mut self) -> io::Result<()> {
        if self.dirty {
            self.put(EV_SYN, SYN_REPORT, 0)?;
        }
        Ok(())
    }

    /// After dropped events: release on the re-emitter every key latched
    /// as emitted, quarantine every key down now until its release, and
    /// rebuild the modifiers from the device. The rest of the batch
    /// predates the snapshot and is discarded.
    fn resync(&mut self) -> io::Result<()> {
        let state = self.device.key_state()?;
        self.cut = true;
        let released = self.keys.resync(&state.down_codes(), state.modifiers());
        for &code in &released {
            self.put(EV_KEY, code, 0)?;
        }
        // Flush whatever was written since the last report, not only these
        // releases: the overflow may have eaten the report that ended a
        // frame already re-emitted.
        self.flush()?;
        tracing::warn!(
            released = released.len(),
            "input events were dropped; key state re-read from the device"
        );
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
                written = self.put(EV_KEY, code, 0);
            }
        }
        if written.is_ok() {
            written = self.flush();
        }
        self.grabbed = false;
        self.dropping = false;
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
        /// What EVIOCGKEY reports.
        held: KeyState,
        /// The key state once the queued events are discarded: a queue
        /// that held a key transition changes it.
        after_drain: Option<KeyState>,
        keys_fail: bool,
        /// Answers for the next key_state calls, before `held` and
        /// `keys_fail` apply.
        script: VecDeque<KeyState>,
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
        fn key_state(&mut self) -> io::Result<KeyState> {
            self.log.borrow_mut().push("keys".into());
            if let Some(state) = self.script.pop_front() {
                return Ok(state);
            }
            if self.keys_fail {
                return Err(io::Error::other("EVIOCGKEY failed"));
            }
            Ok(self.held)
        }
        fn drain(&mut self) -> io::Result<()> {
            self.log.borrow_mut().push("drain".into());
            if let Some(state) = self.after_drain.take() {
                self.held = state;
            }
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
            self.log
                .borrow_mut()
                .push(format!("emit {kind} {code} {value}"));
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
                held: KeyState::NONE,
                after_drain: None,
                keys_fail: false,
                script: VecDeque::new(),
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
            let fire = self
                .grabber
                .event(&self.resolver, EV_KEY, code, value)
                .unwrap();
            if let Some(fire) = fire {
                self.fired.push(fire.command.as_str());
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
        assert_eq!(rig.take_log(), ["keys", "grab 1", "drain", "keys"]);
        rig.grabber.console(Some(3), rig.now).unwrap();
        assert!(
            rig.grabber.is_grabbed(),
            "no VT set: the console is irrelevant"
        );
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
        assert_eq!(rig.take_log(), ["keys", "grab 1", "drain", "keys"]);
        rig.console(&mut console);
        assert!(
            rig.take_log().is_empty(),
            "a wake on the same VT changes nothing"
        );
        rig.console(&mut console);
        assert!(!rig.grabber.is_grabbed());
        assert_eq!(rig.take_log(), ["grab 0"]);
        rig.console(&mut console);
        assert!(
            !rig.grabber.is_grabbed(),
            "an unreadable console never grabs"
        );
    }

    #[test]
    fn events_while_not_grabbed_are_discarded() {
        let mut rig = Rig::new(Some(vec![1]), 0);
        rig.grabber.console(Some(4), rig.now).unwrap();
        rig.key(KEY_A, 1).key(KEY_F5, 1).key(KEY_A, 0);
        let fire = rig
            .grabber
            .event(&rig.resolver, EV_SYN, SYN_REPORT, 0)
            .unwrap();
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
        rig.key(KEY_A, 1)
            .key(KEY_F5, 1)
            .key(KEY_RIGHTSHIFT, 1)
            .key(KEY_UP, 1);
        assert_eq!(
            rig.take_log(),
            ["emit 1 30 1", "emit 1 54 1", "emit 1 103 1"]
        );
        rig.grabber.console(Some(4), rig.now).unwrap();
        assert_eq!(
            rig.take_log(),
            [
                "emit 1 30 0",
                "emit 1 54 0",
                "emit 1 103 0",
                "emit 0 0 0",
                "grab 0"
            ],
            "synthetic releases for emitted keys only, then the grab goes"
        );
        assert!(!rig.grabber.is_grabbed());
    }

    #[test]
    fn switching_back_with_keys_held_waits_for_their_release() {
        let mut rig = Rig::new(Some(vec![1]), 0);
        rig.grabber.console(Some(1), rig.now).unwrap();
        rig.key(KEY_A, 1).key(KEY_RIGHTCTRL, 1);
        rig.grabber.console(Some(4), rig.now).unwrap();
        // The person switches back while still holding both keys.
        rig.grabber.device_mut().held = KeyState::of(&[KEY_A, KEY_RIGHTCTRL]);
        rig.grabber.console(Some(1), rig.now).unwrap();
        assert!(!rig.grabber.is_grabbed(), "no grab while keys are down");
        rig.take_log();
        // Ungrabbed, their repeats and releases go to the compositor
        // directly; inputd re-checks at each report.
        rig.key(KEY_A, 2).key(KEY_A, 0);
        rig.grabber.device_mut().held = KeyState::of(&[KEY_RIGHTCTRL]);
        syn(&mut rig, SYN_REPORT);
        assert!(!rig.grabber.is_grabbed());
        rig.key(KEY_RIGHTCTRL, 0);
        rig.grabber.device_mut().held = KeyState::NONE;
        syn(&mut rig, SYN_REPORT);
        assert!(
            rig.grabber.is_grabbed(),
            "grabbed once the keyboard is quiet"
        );
        assert_eq!(rig.take_log(), ["keys", "keys", "grab 1", "drain", "keys"]);
        rig.key(KEY_A, 1).key(KEY_A, 0);
        assert_eq!(rig.take_log(), ["emit 1 30 1", "emit 1 30 0"]);
    }

    #[test]
    fn a_held_key_defers_the_grab_until_it_is_released() {
        // The VT-switch chord is still down at switch-to.
        let mut rig = Rig::new(Some(vec![1]), 0);
        rig.grabber.device_mut().held = KeyState::of(&[KEY_LEFTCTRL, KEY_LEFTALT, 59]);
        rig.grabber.console(Some(1), rig.now).unwrap();
        assert!(!rig.grabber.is_grabbed());
        assert_eq!(rig.take_log(), ["keys"], "checked, never grabbed");
        assert_eq!(
            rig.grabber.deadline(),
            Some(rig.now + GRAB_RETRY_FIRST),
            "a bounded deadline backs the reports up"
        );
        // Events while waiting are the compositor's.
        rig.key(59, 0).key(KEY_LEFTALT, 0).key(KEY_LEFTCTRL, 0);
        rig.grabber.device_mut().held = KeyState::NONE;
        syn(&mut rig, SYN_REPORT);
        assert!(rig.grabber.is_grabbed());
        assert_eq!(rig.take_log(), ["keys", "grab 1", "drain", "keys"]);
        // No orphan follows: the releases were never inputd's.
        rig.key(KEY_F5, 1).key(KEY_F5, 0);
        assert!(rig.take_log().is_empty(), "bare F5 is swallowed");
    }

    #[test]
    fn a_key_pressed_between_the_check_and_the_grab_drops_the_grab() {
        let mut rig = Rig::new(Some(vec![1]), 0);
        let device = rig.grabber.device_mut();
        // Quiet at the check; A down in the snapshot after the grab.
        device.script = VecDeque::from([KeyState::NONE]);
        device.held = KeyState::of(&[KEY_A]);
        rig.grabber.console(Some(1), rig.now).unwrap();
        assert!(!rig.grabber.is_grabbed());
        assert_eq!(
            rig.take_log(),
            ["keys", "grab 1", "drain", "keys", "grab 0"]
        );
        // Waiting again: A's release arrives ungrabbed, then the retry.
        rig.key(KEY_A, 0);
        rig.grabber.device_mut().held = KeyState::NONE;
        syn(&mut rig, SYN_REPORT);
        assert!(rig.grabber.is_grabbed());
        assert_eq!(rig.take_log(), ["keys", "grab 1", "drain", "keys"]);
    }

    #[test]
    fn switching_away_while_waiting_cancels_the_wait() {
        let mut rig = Rig::new(Some(vec![1]), 0);
        rig.grabber.device_mut().held = KeyState::of(&[KEY_A]);
        rig.grabber.console(Some(1), rig.now).unwrap();
        rig.grabber.console(Some(4), rig.now).unwrap();
        rig.take_log();
        rig.grabber.device_mut().held = KeyState::NONE;
        rig.key(KEY_A, 0);
        syn(&mut rig, SYN_REPORT);
        assert!(!rig.grabber.is_grabbed(), "VT4 is active: no grab");
        assert!(rig.take_log().is_empty(), "not even a check");
        assert_eq!(rig.grabber.deadline(), None, "the wait's deadline went too");
        rig.grabber.tick(rig.now + GRAB_RETRY_CAP).unwrap();
        assert!(rig.take_log().is_empty());
    }

    #[test]
    fn a_wait_is_retried_on_a_bounded_deadline_without_reports() {
        // Another holder owns the grab while A is down, takes A's release
        // and its report, then ungrabs: inputd sees no report at all.
        let mut rig = Rig::new(Some(vec![1]), 0);
        rig.grabber.device_mut().held = KeyState::of(&[KEY_A]);
        let t0 = rig.now;
        rig.grabber.console(Some(1), t0).unwrap();
        // While A stays down the deadline backs off to the cap, never
        // faster: a held key is no busy loop.
        let mut waits = Vec::new();
        let mut at = t0;
        for _ in 0..8 {
            let due = rig.grabber.deadline().expect("a deadline while waiting");
            waits.push((due - at).as_millis());
            rig.grabber.tick(due - Duration::from_millis(1)).unwrap();
            at = due;
            rig.grabber.tick(due).unwrap();
        }
        assert_eq!(waits, [50, 100, 200, 400, 800, 1600, 2000, 2000]);
        assert!(!rig.grabber.is_grabbed());
        // The holder took the release; the next deadline finds it quiet.
        rig.grabber.device_mut().held = KeyState::NONE;
        rig.take_log();
        let due = rig.grabber.deadline().unwrap();
        rig.grabber.tick(due).unwrap();
        assert!(rig.grabber.is_grabbed());
        assert_eq!(rig.take_log(), ["keys", "grab 1", "drain", "keys"]);
        assert_eq!(rig.grabber.deadline(), None);
    }

    #[test]
    fn a_recovery_flushes_a_frame_the_overflow_cut_short() {
        // A's release is read, then the overflow replaces the rest of its
        // frame, report included. The release goes out and must be flushed
        // even though recovery has nothing of its own to release.
        let mut rig = Rig::new(None, 0);
        rig.grabber.start(rig.now).unwrap();
        batch(&mut rig, &[(EV_KEY, KEY_A, 1), (EV_SYN, SYN_REPORT, 0)]);
        rig.take_log();
        batch(
            &mut rig,
            &[
                (EV_KEY, KEY_A, 0),
                (EV_SYN, SYN_DROPPED, 0),
                (EV_SYN, SYN_REPORT, 0),
            ],
        );
        assert_eq!(rig.take_log(), ["emit 1 30 0", "keys", "emit 0 0 0"]);
    }

    #[test]
    fn a_switch_away_flushes_any_unreported_write() {
        let mut rig = Rig::new(Some(vec![1]), 0);
        rig.grabber.console(Some(1), rig.now).unwrap();
        // A's release is out but its report has not arrived yet.
        rig.key(KEY_A, 1);
        syn(&mut rig, SYN_REPORT);
        rig.key(KEY_A, 0);
        rig.take_log();
        rig.grabber.console(Some(4), rig.now).unwrap();
        assert_eq!(rig.take_log(), ["emit 0 0 0", "grab 0"]);
        // With nothing outstanding, a switch-away writes no empty frame.
        rig.grabber.console(Some(1), rig.now).unwrap();
        rig.take_log();
        rig.grabber.console(Some(4), rig.now).unwrap();
        assert_eq!(rig.take_log(), ["grab 0"]);
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
        assert!(
            rig.take_log().is_empty(),
            "swallowed at press, swallowed to release"
        );
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
        assert_eq!(
            rig.take_log().iter().filter(|l| *l == "grab busy").count(),
            7
        );
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
            fn key_state(&mut self) -> io::Result<KeyState> {
                Ok(KeyState::NONE)
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

    fn syn(rig: &mut Rig, code: u16) {
        let fire = rig.grabber.event(&rig.resolver, EV_SYN, code, 0).unwrap();
        assert!(fire.is_none());
    }

    #[test]
    fn dropped_events_are_recovered_from_the_device_state() {
        let mut rig = Rig::new(None, 0);
        rig.grabber.start(rig.now).unwrap();
        // Held and emitted: A and Right Ctrl. Then the device overflows and
        // their releases are lost; only Left Shift is down afterwards.
        rig.key(KEY_A, 1).key(KEY_RIGHTCTRL, 1);
        syn(&mut rig, SYN_REPORT);
        rig.take_log();
        rig.grabber.device_mut().held = KeyState::of(&[KEY_LEFTSHIFT]);
        syn(&mut rig, SYN_DROPPED);
        // Everything up to and including the next report is ignored.
        rig.key(KEY_RIGHTCTRL, 0).key(KEY_F5, 1);
        assert!(rig.take_log().is_empty(), "events inside the drop leaked");
        syn(&mut rig, SYN_REPORT);
        assert_eq!(
            rig.take_log(),
            ["keys", "emit 1 30 0", "emit 1 97 0", "emit 0 0 0"],
            "the state is re-read and the lost releases are sent"
        );
        // Right Ctrl is no longer counted, Left Shift is, but it is
        // quarantined: the apps never saw its press, so Left Shift + → is
        // held back rather than sent as a bare →. After Left Shift goes up
        // a bare F5 is swallowed.
        rig.key(KEY_RIGHT, 1).key(KEY_RIGHT, 0);
        assert!(rig.take_log().is_empty(), "→ went out without its Shift");
        rig.key(KEY_LEFTSHIFT, 0).key(KEY_F5, 1).key(KEY_F5, 0);
        assert!(rig.take_log().is_empty(), "bare F5 leaked after resync");
        // And the stream resumes normally.
        rig.key(KEY_A, 1);
        syn(&mut rig, SYN_REPORT);
        assert_eq!(rig.take_log(), ["emit 1 30 1", "emit 0 0 0"]);
    }

    #[test]
    fn a_right_ctrl_released_while_draining_is_not_left_held() {
        // Right Ctrl goes down after the quiet check and is down at the
        // grab; its release is among the queued events the grab discards.
        // The snapshot must come after the drain, or a stale Right Ctrl
        // turns bare F5 into RCtrl+F5, which passes to apps.
        let mut rig = Rig::new(None, 0);
        let device = rig.grabber.device_mut();
        device.script = VecDeque::from([KeyState::NONE]);
        device.held = KeyState::of(&[KEY_RIGHTCTRL]);
        device.after_drain = Some(KeyState::NONE);
        rig.grabber.start(rig.now).unwrap();
        assert_eq!(rig.take_log(), ["keys", "grab 1", "drain", "keys"]);
        rig.key(KEY_F5, 1).key(KEY_F5, 0);
        assert!(rig.take_log().is_empty(), "bare F5 leaked to the apps");
    }

    #[test]
    fn a_failed_key_snapshot_drops_the_grab_and_reports() {
        let mut rig = Rig::new(None, 0);
        let device = rig.grabber.device_mut();
        device.script = VecDeque::from([KeyState::NONE]);
        device.keys_fail = true;
        assert!(rig.grabber.start(rig.now).is_err());
        assert!(!rig.grabber.is_grabbed());
        assert_eq!(
            rig.take_log(),
            ["keys", "grab 1", "drain", "keys", "grab 0"]
        );
        // A failed check before the grab is reported too, with no grab.
        assert!(rig.grabber.start(rig.now).is_err());
        assert_eq!(rig.take_log(), ["keys"]);
        rig.key(KEY_F5, 1);
        assert!(rig.take_log().is_empty(), "nothing is processed ungrabbed");
    }

    #[test]
    fn key_state_bitmaps() {
        let state = KeyState::of(&[KEY_RIGHTCTRL, KEY_LEFTALT, 0x2ff]);
        assert!(state.is_down(KEY_RIGHTCTRL) && state.is_down(0x2ff));
        assert!(!state.is_down(KEY_LEFTCTRL) && !state.is_down(0x300));
        assert_eq!(
            state.modifiers(),
            SideModifiers {
                right_ctrl: true,
                left_alt: true,
                ..SideModifiers::NONE
            }
        );
        assert!(state.any_down() && !KeyState::NONE.any_down());
        assert_eq!(state.down_codes(), [KEY_LEFTALT, KEY_RIGHTCTRL, 0x2ff]);
    }

    fn batch(rig: &mut Rig, events: &[(u16, u16, i32)]) {
        let fired = rig.grabber.events(&rig.resolver, events).unwrap();
        rig.fired.extend(fired.iter().map(|f| f.command.as_str()));
    }

    #[test]
    fn a_recovery_discards_the_rest_of_its_batch() {
        // Case 1: the bare F5 after the recovery report predates the
        // snapshot, which shows Right Ctrl down. It must not be judged
        // (and emitted as RCtrl+F5) under that newer state.
        let mut rig = Rig::new(None, 0);
        rig.grabber.start(rig.now).unwrap();
        rig.take_log();
        rig.grabber.device_mut().held = KeyState::of(&[KEY_RIGHTCTRL]);
        batch(
            &mut rig,
            &[
                (EV_SYN, SYN_DROPPED, 0),
                (EV_SYN, SYN_REPORT, 0),
                (EV_KEY, KEY_F5, 1),
                (EV_KEY, KEY_F5, 0),
                (EV_KEY, KEY_RIGHTCTRL, 1),
                (EV_SYN, SYN_REPORT, 0),
            ],
        );
        assert_eq!(rig.take_log(), ["keys"], "nothing after the snapshot");
        // Right Ctrl is quarantined: its release reaches nothing either.
        batch(
            &mut rig,
            &[(EV_KEY, KEY_RIGHTCTRL, 0), (EV_SYN, SYN_REPORT, 0)],
        );
        assert_eq!(rig.take_log(), ["emit 0 0 0"]);
    }

    #[test]
    fn a_buffered_press_older_than_the_snapshot_cannot_stick() {
        // Case 2: A's press sits in the batch after the recovery report;
        // its release was flushed by the snapshot, which shows A up. The
        // press must not be emitted, or nothing would ever release it.
        let mut rig = Rig::new(None, 0);
        rig.grabber.start(rig.now).unwrap();
        rig.take_log();
        batch(
            &mut rig,
            &[
                (EV_SYN, SYN_DROPPED, 0),
                (EV_SYN, SYN_REPORT, 0),
                (EV_KEY, KEY_A, 1),
                (EV_SYN, SYN_REPORT, 0),
            ],
        );
        assert_eq!(rig.take_log(), ["keys"]);
        assert!(!rig.grabber.keys.is_emitted(KEY_A));
    }

    #[test]
    fn taking_the_grab_mid_batch_discards_the_rest_of_it() {
        // While waiting, the report that finds the keyboard quiet takes the
        // grab; events after it in the same batch were read before the
        // grab, so the compositor already had them.
        let mut rig = Rig::new(None, 0);
        rig.grabber.device_mut().held = KeyState::of(&[KEY_A]);
        rig.grabber.start(rig.now).unwrap();
        rig.grabber.device_mut().held = KeyState::NONE;
        rig.take_log();
        batch(
            &mut rig,
            &[
                (EV_KEY, KEY_A, 0),
                (EV_SYN, SYN_REPORT, 0),
                (EV_KEY, KEY_A, 1),
                (EV_SYN, SYN_REPORT, 0),
            ],
        );
        assert!(rig.grabber.is_grabbed());
        assert_eq!(rig.take_log(), ["keys", "grab 1", "drain", "keys"]);
        assert!(!rig.grabber.keys.is_emitted(KEY_A));
    }

    #[test]
    fn an_overflow_releases_every_emitted_key_and_quarantines_held_ones() {
        // RCtrl + F5 is emitted; the overflow loses F5 up, RCtrl up and a
        // new bare F5 down. The state shows F5 down.
        let mut rig = Rig::new(None, 0);
        rig.grabber.start(rig.now).unwrap();
        rig.key(KEY_RIGHTCTRL, 1).key(KEY_F5, 1);
        rig.take_log();
        rig.grabber.device_mut().held = KeyState::of(&[KEY_F5]);
        syn(&mut rig, SYN_DROPPED);
        syn(&mut rig, SYN_REPORT);
        assert_eq!(
            rig.take_log(),
            ["keys", "emit 1 63 0", "emit 1 97 0", "emit 0 0 0"]
        );
        // The new bare F5's repeats and release leak nowhere.
        rig.key(KEY_F5, 2).key(KEY_F5, 2).key(KEY_F5, 0);
        assert!(rig.take_log().is_empty(), "the new F5 leaked");
        assert!(rig.fired.is_empty());
    }
}
