// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Following the desktop profile's appearance from an app, with no I/O.
//!
//! The [`crate::consumer::Consumer`] serves shells that hold the raw native
//! connection and its owner-stamped deliveries. An app on a plain Bus
//! connection has less: a hint that the profile changed (a delivery on one
//! of [`Follower::topics`], the contract 1.0.0 look topic and the `keys`
//! topics of the app domains it carries) and the ability to call
//! `settings.get`. A [`Follower`] turns that into the profile's
//! [`Look`] and app domains:
//!
//! - every hint, reconnection or start asks for one [`Read`] of the
//!   `values` view (key space v2 §5.2); while one is in flight, further
//!   hints fold into a single read after it;
//! - a hint is never trusted: the look comes only from `settings.get`'s
//!   answer, and an answer older than one already seen (a lower revision in
//!   the same incarnation) is ignored;
//! - when settingsd serves another instance it answers `wrong_target` with
//!   its binding; the instance (never the profile) is adopted once.
//!
//! The app performs each [`Read`] on its connection and reports the reply
//! with [`Follower::answered`].
use crate::model::{Binding, Revision};
use crate::registry::Values;
use crate::view::{AppDomains, Keymaps, Look, View};
use serde_json::{Value, json};
use std::time::Duration;

/// The settings service.
pub const SERVICE: &str = "settingsd";
/// The desktop profile an app follows.
pub const PROFILE: &str = "default";

/// One settingsd call to perform: a `settings.get` read, or a
/// [`Follower::apply`] (whose ticket is 0: no answer is awaited).
#[derive(Clone, Debug, PartialEq)]
pub struct Read {
    pub ticket: u64,
    pub service: &'static str,
    pub command: &'static str,
    pub body: Value,
}

/// What an answer meant.
#[derive(Clone, Debug, PartialEq)]
pub enum Followed {
    /// The profile's look ([`Look`], the follower view), at `revision`, new
    /// to this follower. `custom_source` says the profile names a design
    /// source of its own (an app keeps its theme file then).
    Look {
        look: Box<Look>,
        custom_source: bool,
        revision: u64,
        /// The person's key bindings, per application
        /// ([`crate::keymap`]); each app applies its own entry.
        keymaps: crate::keymap::Keymaps,
        /// The terminal's preferences ([`crate::terminal`]).
        terminal: Box<crate::terminal::Terminal>,
        /// The email app's accounts and options ([`crate::email`]).
        email: Box<crate::email::Email>,
        /// The Ced app's editor style ([`crate::content`]).
        ced: Box<crate::content::Ced>,
    },
    /// The same revision as before: nothing to do.
    Unchanged,
    /// settingsd is not on the Bus: keep the app's own fallback.
    Missing,
    /// The answer could not be used; the last look (if any) stands. A
    /// `retry` fault (the call failed or was lost, the broker refused it)
    /// is worth asking again after [`Follower::retry_delay`]; another (a
    /// refusal or a malformed answer) waits for the next hint.
    Fault { message: String, retry: bool },
}

impl Followed {
    fn fault(message: impl Into<String>, retry: bool) -> Self {
        Followed::Fault {
            message: message.into(),
            retry,
        }
    }
}

/// The prefixes besides the look whose `keys` topics wake a follower: what
/// [`Followed::Look`] carries beyond the [`Look`].
pub const HINT_PREFIXES: [&str; 4] = ["keymaps", "terminal", "email", "ced"];

/// The hint topics of `profile`'s followers ([`Follower::topics`]), for an
/// app that subscribes before it builds its follower.
pub fn hint_topics(profile: &str) -> Vec<String> {
    std::iter::once(crate::topics::look_topic(profile))
        .chain(
            HINT_PREFIXES
                .iter()
                .map(|prefix| crate::topics::keys_topic(profile, prefix)),
        )
        .collect()
}

/// Whether `topic` is one of [`hint_topics`] of `profile`.
pub fn is_hint(profile: &str, topic: &str) -> bool {
    topic == crate::topics::look_topic(profile)
        || HINT_PREFIXES
            .iter()
            .any(|prefix| topic == crate::topics::keys_topic(profile, prefix))
}

/// The first retry after a transient fault, doubling up to [`RETRY_MAX`].
pub const RETRY_FIRST: Duration = Duration::from_millis(250);
pub const RETRY_MAX: Duration = Duration::from_secs(30);

/// One app's view of the profile's appearance.
#[derive(Clone, Debug)]
pub struct Follower {
    binding: Binding,
    retargeted: bool,
    next_ticket: u64,
    in_flight: Option<u64>,
    again: bool,
    seen: Option<(String, Revision)>,
    /// settingsd went missing since the last look: the same revision, when
    /// it comes back, is the look again (a restart keeps the revision).
    lost: bool,
    /// Transient faults in a row, for the retry delay.
    failures: u32,
    /// The answer just read retargeted the binding.
    retarget_now: bool,
}

impl Follower {
    /// Follow the default profile of settingsd `instance` (the host name is
    /// the usual guess; settingsd runs as `--instance %H`). An invalid name
    /// falls back to "host", which settingsd corrects with `wrong_target`.
    pub fn new(instance: &str) -> Self {
        let mut binding = Binding {
            instance: instance.to_owned(),
            profile: PROFILE.into(),
        };
        if binding.validate().is_err() {
            binding.instance = "host".into();
        }
        Self {
            binding,
            retargeted: false,
            next_ticket: 0,
            in_flight: None,
            again: false,
            seen: None,
            lost: false,
            failures: 0,
            retarget_now: false,
        }
    }

    /// The topics whose deliveries are hints for this follower: the
    /// profile's look topic and the `keys` topics of [`HINT_PREFIXES`].
    /// Each is change-only, so a follower wakes only when what it reads
    /// moved.
    pub fn topics(&self) -> Vec<String> {
        hint_topics(&self.binding.profile)
    }

    /// Whether a delivery on `topic` is a hint for this follower.
    pub fn is_hint(&self, topic: &str) -> bool {
        is_hint(&self.binding.profile, topic)
    }

    pub fn binding(&self) -> &Binding {
        &self.binding
    }

    /// The read in flight, if any.
    pub fn pending(&self) -> Option<u64> {
        self.in_flight
    }

    /// After a transient fault: how long to wait before asking again
    /// ([`RETRY_FIRST`] doubling to [`RETRY_MAX`]); `None` when the last
    /// answer was usable or not worth retrying.
    pub fn retry_delay(&self) -> Option<Duration> {
        (self.failures > 0).then(|| {
            RETRY_FIRST
                .saturating_mul(1 << (self.failures - 1).min(16))
                .min(RETRY_MAX)
        })
    }

    /// Something may have changed (start, a hint, a reconnection): the read
    /// to perform now, or `None` when one is already in flight (it is
    /// repeated once it answers).
    pub fn read(&mut self) -> Option<Read> {
        if self.in_flight.is_some() {
            self.again = true;
            return None;
        }
        Some(self.issue())
    }

    fn issue(&mut self) -> Read {
        self.next_ticket += 1;
        self.in_flight = Some(self.next_ticket);
        self.again = false;
        Read {
            ticket: self.next_ticket,
            service: SERVICE,
            command: "settings.get",
            body: json!({"binding":self.binding,"view":"values"}),
        }
    }

    /// Whether a look has been read and settingsd has not gone missing
    /// since: what [`Follower::apply`] needs.
    pub fn following(&self) -> bool {
        self.seen.is_some() && !self.lost
    }

    /// A `settings.apply` of `changes` (`{path: value}`), fenced on the last
    /// look this follower read, so a change made meanwhile elsewhere is
    /// refused as a `conflict` rather than overwritten. `None` until a look
    /// has been read, or while settingsd is missing. `operation_id` names
    /// the change in settingsd's receipts: unique per change, 1–128 of
    /// ASCII letters, digits, `-` and `_`. The reply needs no follower: the
    /// change comes back as a new look through the summary topic.
    pub fn apply(&self, operation_id: &str, changes: Value) -> Option<Read> {
        if !self.following() {
            return None;
        }
        let (incarnation, revision) = self.seen.as_ref()?;
        Some(Read {
            ticket: 0,
            service: SERVICE,
            command: "settings.apply",
            body: json!({
                "binding": self.binding,
                "expected_incarnation": incarnation,
                "expected_revision": revision,
                "operation_id": operation_id,
                "changes": changes,
            }),
        })
    }

    /// The reply to read `ticket`: `rc`, its body, and the broker's error
    /// header when the broker itself refused. `None` as `reply` is a call
    /// that failed or was lost. Returns what it meant (nothing, for a reply
    /// to an older read) and the next read to perform, if any.
    pub fn answered(
        &mut self,
        ticket: u64,
        reply: Option<(u8, &str, Option<&str>)>,
    ) -> (Option<Followed>, Option<Read>) {
        if self.in_flight != Some(ticket) {
            return (None, None);
        }
        self.in_flight = None;
        let meaning = match reply {
            None => Meaning::Followed(Followed::fault("the settings read failed", true)),
            Some((rc, body, error)) => self.meaning(rc, body, error),
        };
        match &meaning {
            Meaning::Followed(Followed::Fault { retry: true, .. }) => {
                self.failures = self.failures.saturating_add(1);
            }
            _ => self.failures = 0,
        }
        match &meaning {
            Meaning::Followed(Followed::Missing) => self.lost = true,
            Meaning::Followed(Followed::Look { .. }) => self.lost = false,
            _ => {}
        }
        // A retarget asks again at once; so does a hint that arrived meanwhile.
        match meaning {
            Meaning::Retargeted => (None, Some(self.issue())),
            Meaning::Followed(followed) => (Some(followed), self.again.then(|| self.issue())),
        }
    }

    fn meaning(&mut self, rc: u8, body: &str, error: Option<&str>) -> Meaning {
        let followed = self.followed(rc, body, error);
        if std::mem::take(&mut self.retarget_now) {
            Meaning::Retargeted
        } else {
            Meaning::Followed(followed)
        }
    }

    fn followed(&mut self, rc: u8, body: &str, error: Option<&str>) -> Followed {
        if body.trim().is_empty() {
            if rc == 10 && error.is_none_or(|e| e.contains("not found")) {
                return Followed::Missing;
            }
            return Followed::fault(
                error.map_or_else(|| format!("refused by the broker (rc {rc})"), str::to_owned),
                true,
            );
        }
        let Ok(value) = serde_json::from_str::<Value>(body) else {
            return Followed::fault("the settings answer is not JSON", false);
        };
        if rc != 0 {
            if value["status"] == "wrong_target"
                && !self.retargeted
                && let Ok(binding) = serde_json::from_value::<Binding>(value["binding"].clone())
                && binding.validate().is_ok()
                && binding.profile == self.binding.profile
            {
                self.binding.instance = binding.instance;
                self.retargeted = true;
                self.retarget_now = true;
                return Followed::Unchanged;
            }
            return Followed::fault(
                value["message"]
                    .as_str()
                    .or(value["status"].as_str())
                    .unwrap_or("refused"),
                false,
            );
        }
        let Some(incarnation) = value["incarnation"].as_str() else {
            return Followed::fault("values: no incarnation", false);
        };
        let revision: Revision = match serde_json::from_value(value["revision"].clone()) {
            Ok(revision) => revision,
            Err(e) => return Followed::fault(format!("values: revision: {e}"), false),
        };
        let values: Values = match serde_json::from_value(value["values"].clone()) {
            Ok(values) => values,
            Err(e) => return Followed::fault(format!("values: {e}"), false),
        };
        // Older is never the look; the same revision is, once settingsd was
        // missing and came back.
        if let Some((seen_incarnation, seen)) = &self.seen
            && seen_incarnation == incarnation
            && (revision < *seen || (revision == *seen && !self.lost))
        {
            return Followed::Unchanged;
        }
        self.seen = Some((incarnation.to_owned(), revision));
        let look = Look::of(&values);
        let domains = AppDomains::of(&values);
        Followed::Look {
            custom_source: look.appearance.source.is_some(),
            look: Box::new(look),
            revision: revision.0,
            keymaps: Keymaps::of(&values).apps,
            terminal: Box::new(domains.terminal),
            email: Box::new(domains.email),
            ced: Box::new(domains.ced),
        }
    }
}

/// What one answer meant inside the follower. Made once per answer and
/// consumed at once, so its size does not matter.
#[allow(clippy::large_enum_variant)]
enum Meaning {
    Followed(Followed),
    /// The instance was adopted: the read is repeated at once.
    Retargeted,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `settings.get {view: values}` answer, as contract 1.0.0 gives it.
    fn snapshot(incarnation: &str, revision: &str, scheme: &str) -> String {
        json!({"status":"current",
            "binding":{"instance":"example","profile":"default"},
            "incarnation":incarnation,"revision":revision,
            "values":{"appearance.scheme":scheme,"appearance.mode":"dark",
                "appearance.contrast":"normal"},
            "resolved":{"appearance.mode":"dark"}})
        .to_string()
    }

    #[test]
    fn it_follows_the_look_and_app_domain_topics_and_reads_the_values_view() {
        let mut f = Follower::new("example");
        assert_eq!(
            f.topics(),
            [
                "settingsd.desktop.look.default",
                "settingsd.desktop.keys.default.keymaps",
                "settingsd.desktop.keys.default.terminal",
                "settingsd.desktop.keys.default.email",
                "settingsd.desktop.keys.default.ced",
            ]
        );
        assert!(f.is_hint("settingsd.desktop.keys.default.terminal"));
        assert!(!f.is_hint("settingsd.desktop.keys.default.displays"));
        assert!(!f.is_hint("settingsd.desktop.look.other"));
        assert_eq!(f.read().unwrap().body["view"], "values");
    }

    #[test]
    fn the_look_is_the_look_view_of_the_values() {
        let mut f = Follower::new("example");
        let read = f.read().unwrap();
        let mut body: Value = serde_json::from_str(&snapshot("i", "3", "studio")).unwrap();
        body["values"]["window.decorations"] = json!("ssd");
        body["values"]["accessibility.text_scale"] = json!(1.5);
        body["values"]["appearance.density"] = json!("compact");
        match f
            .answered(read.ticket, Some((0, &body.to_string(), None)))
            .0
        {
            Some(Followed::Look {
                look,
                custom_source,
                ..
            }) => {
                assert_eq!(look.appearance.scheme, "studio");
                assert_eq!(look.framing.decorations, "ssd");
                assert_eq!(look.text_scale, 1.5);
                assert_eq!(
                    look.appearance.density,
                    design::geometry::Density::Compact
                );
                assert!(!custom_source);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_look_carries_the_key_bindings() {
        let mut f = Follower::new("example");
        let read = f.read().unwrap();
        let mut body: Value = serde_json::from_str(&snapshot("i", "3", "studio")).unwrap();
        body["values"]["keymaps.dopus.nav.parent"] = json!(["Ctrl+U"]);
        match f
            .answered(read.ticket, Some((0, &body.to_string(), None)))
            .0
        {
            Some(Followed::Look { keymaps, .. }) => {
                assert_eq!(keymaps["dopus"]["nav.parent"], ["Ctrl+U"]);
            }
            other => panic!("{other:?}"),
        }
        // A profile without them reads as none.
        let read = f.read().unwrap();
        match f
            .answered(read.ticket, Some((0, &snapshot("i", "4", "studio"), None)))
            .0
        {
            Some(Followed::Look { keymaps, .. }) => assert!(keymaps.is_empty()),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn an_apply_is_fenced_on_the_last_look_read() {
        let mut f = Follower::new("example");
        let changes = json!({"terminal.font.size": 14.0});
        assert_eq!(f.apply("op-1", changes.clone()), None, "no look yet");
        assert!(!f.following());
        let read = f.read().unwrap();
        f.answered(
            read.ticket,
            Some((0, &snapshot("inc", "7", "studio"), None)),
        );
        assert!(f.following());
        let apply = f.apply("op-1", changes.clone()).unwrap();
        assert_eq!((apply.service, apply.command), (SERVICE, "settings.apply"));
        let request: crate::model::ApplyRequest =
            serde_json::from_value(apply.body.clone()).unwrap();
        assert_eq!(request.binding, *f.binding());
        assert_eq!(request.expected_incarnation, "inc");
        assert_eq!(request.expected_revision, Revision(7));
        assert_eq!(request.operation_id, "op-1");
        assert_eq!(request.changes["terminal.font.size"], 14.0);
        // Gone missing: nothing to fence on.
        let read = f.read().unwrap();
        f.answered(read.ticket, Some((10, "", Some("not found"))));
        assert!(!f.following());
        assert_eq!(f.apply("op-2", changes), None);
    }

    #[test]
    fn the_look_carries_the_terminal_settings() {
        let mut f = Follower::new("example");
        let read = f.read().unwrap();
        let mut body: Value = serde_json::from_str(&snapshot("i", "3", "studio")).unwrap();
        body["values"]["terminal.padding"] = json!(8);
        body["values"]["terminal.cursor.shape"] = json!("bar");
        match f
            .answered(read.ticket, Some((0, &body.to_string(), None)))
            .0
        {
            Some(Followed::Look { terminal, .. }) => {
                assert_eq!(terminal.padding, 8);
                assert_eq!(terminal.cursor.shape, crate::terminal::CursorShape::Bar);
            }
            other => panic!("{other:?}"),
        }
        // A profile without them reads as the defaults.
        let read = f.read().unwrap();
        match f
            .answered(read.ticket, Some((0, &snapshot("i", "4", "studio"), None)))
            .0
        {
            Some(Followed::Look { terminal, .. }) => assert!(terminal.is_default()),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_look_carries_the_email_settings() {
        let mut f = Follower::new("example");
        let read = f.read().unwrap();
        let mut body: Value = serde_json::from_str(&snapshot("i", "3", "studio")).unwrap();
        body["values"]["email.accounts"] = json!([{"id":"acct-1","name":"Home",
            "url":"https://mail.example.org","email":"me@example.org"}]);
        body["values"]["email.mail.page_size"] = json!(50);
        match f
            .answered(read.ticket, Some((0, &body.to_string(), None)))
            .0
        {
            Some(Followed::Look { email, .. }) => {
                assert_eq!(email.accounts[0].id, "acct-1");
                assert_eq!(email.mail.page_size, 50);
            }
            other => panic!("{other:?}"),
        }
        let read = f.read().unwrap();
        match f
            .answered(read.ticket, Some((0, &snapshot("i", "4", "studio"), None)))
            .0
        {
            Some(Followed::Look { email, .. }) => assert!(email.is_default()),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_custom_source_is_reported() {
        let mut f = Follower::new("example");
        let read = f.read().unwrap();
        let mut body: Value = serde_json::from_str(&snapshot("i", "7", "forest")).unwrap();
        body["values"]["appearance.source"] = json!("design: {}");
        let (followed, _) = f.answered(read.ticket, Some((0, &body.to_string(), None)));
        match followed {
            Some(Followed::Look { custom_source, .. }) => assert!(custom_source),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_read_yields_the_look_once_per_revision() {
        let mut f = Follower::new("example");
        let read = f.read().unwrap();
        assert_eq!(read.command, "settings.get");
        assert_eq!(read.body["binding"]["instance"], "example");
        let (followed, next) =
            f.answered(read.ticket, Some((0, &snapshot("i", "7", "forest"), None)));
        assert!(next.is_none());
        match followed {
            Some(Followed::Look { look, revision, .. }) => {
                assert_eq!((look.appearance.scheme.as_str(), revision), ("forest", 7));
            }
            other => panic!("{other:?}"),
        }
        let read = f.read().unwrap();
        let (followed, _) = f.answered(read.ticket, Some((0, &snapshot("i", "7", "forest"), None)));
        assert_eq!(followed, Some(Followed::Unchanged));
    }

    #[test]
    fn an_older_revision_is_ignored_a_new_incarnation_is_not() {
        let mut f = Follower::new("example");
        let r = f.read().unwrap();
        f.answered(r.ticket, Some((0, &snapshot("i", "9", "forest"), None)));
        let r = f.read().unwrap();
        let (followed, _) = f.answered(r.ticket, Some((0, &snapshot("i", "8", "ocean"), None)));
        assert_eq!(followed, Some(Followed::Unchanged));
        let r = f.read().unwrap();
        let (followed, _) = f.answered(r.ticket, Some((0, &snapshot("j", "1", "ocean"), None)));
        assert!(matches!(followed, Some(Followed::Look { revision: 1, .. })));
    }

    #[test]
    fn hints_while_reading_fold_into_one_read_after() {
        let mut f = Follower::new("example");
        let r = f.read().unwrap();
        assert!(f.read().is_none() && f.read().is_none());
        let (_, next) = f.answered(r.ticket, Some((0, &snapshot("i", "1", "studio"), None)));
        let next = next.expect("one more read");
        let (_, after) = f.answered(next.ticket, Some((0, &snapshot("i", "2", "studio"), None)));
        assert!(after.is_none());
    }

    #[test]
    fn a_stale_ticket_is_ignored() {
        let mut f = Follower::new("example");
        let r = f.read().unwrap();
        assert_eq!(f.answered(r.ticket + 1, None), (None, None));
        assert!(f.read().is_none(), "the real read is still in flight");
    }

    #[test]
    fn wrong_target_adopts_the_instance_once_never_the_profile() {
        let mut f = Follower::new("guess");
        let r = f.read().unwrap();
        let body =
            json!({"status":"wrong_target","binding":{"instance":"real","profile":"default"}})
                .to_string();
        let (followed, next) = f.answered(r.ticket, Some((10, &body, None)));
        assert!(followed.is_none());
        let next = next.expect("the read again");
        assert_eq!(next.body["binding"]["instance"], "real");
        let again =
            json!({"status":"wrong_target","binding":{"instance":"other","profile":"default"}})
                .to_string();
        let (followed, next) = f.answered(next.ticket, Some((10, &again, None)));
        assert!(matches!(followed, Some(Followed::Fault { .. })) && next.is_none());

        let mut f = Follower::new("guess");
        let r = f.read().unwrap();
        let other =
            json!({"status":"wrong_target","binding":{"instance":"real","profile":"other"}})
                .to_string();
        let (followed, next) = f.answered(r.ticket, Some((10, &other, None)));
        assert!(matches!(followed, Some(Followed::Fault { .. })) && next.is_none());
        assert_eq!(f.binding().profile, PROFILE);
    }

    #[test]
    fn missing_settingsd_and_failures() {
        let mut f = Follower::new("example");
        let r = f.read().unwrap();
        let (followed, _) = f.answered(
            r.ticket,
            Some((10, "", Some("Service 'settingsd' not found"))),
        );
        assert_eq!(followed, Some(Followed::Missing));
        let r = f.read().unwrap();
        let (followed, _) = f.answered(r.ticket, None);
        assert!(matches!(followed, Some(Followed::Fault { .. })));
        let r = f.read().unwrap();
        let (followed, _) = f.answered(r.ticket, Some((20, "", Some("overloaded"))));
        assert!(matches!(followed, Some(Followed::Fault { .. })));
    }

    #[test]
    fn after_settingsd_was_missing_the_same_revision_is_the_look_again() {
        let mut f = Follower::new("example");
        let r = f.read().unwrap();
        f.answered(r.ticket, Some((0, &snapshot("i", "7", "forest"), None)));
        let r = f.read().unwrap();
        let (followed, _) = f.answered(
            r.ticket,
            Some((10, "", Some("Service 'settingsd' not found"))),
        );
        assert_eq!(followed, Some(Followed::Missing));
        // A restart keeps incarnation and revision.
        let r = f.read().unwrap();
        let (followed, _) = f.answered(r.ticket, Some((0, &snapshot("i", "7", "forest"), None)));
        assert!(matches!(followed, Some(Followed::Look { revision: 7, .. })));
        // Then the same revision is unchanged again, and older stays refused.
        let r = f.read().unwrap();
        let (followed, _) = f.answered(r.ticket, Some((0, &snapshot("i", "7", "forest"), None)));
        assert_eq!(followed, Some(Followed::Unchanged));
        let r = f.read().unwrap();
        f.answered(r.ticket, Some((10, "", None)));
        let r = f.read().unwrap();
        let (followed, _) = f.answered(r.ticket, Some((0, &snapshot("i", "6", "ocean"), None)));
        assert_eq!(
            followed,
            Some(Followed::Unchanged),
            "older is never the look"
        );
    }

    #[test]
    fn transient_faults_back_off_and_reset_on_an_answer() {
        let mut f = Follower::new("example");
        assert_eq!(f.retry_delay(), None);
        let mut delays = Vec::new();
        for _ in 0..10 {
            let r = f.read().unwrap();
            let (followed, _) = f.answered(r.ticket, None);
            assert!(matches!(
                followed,
                Some(Followed::Fault { retry: true, .. })
            ));
            delays.push(f.retry_delay().unwrap());
        }
        assert_eq!(delays[0], RETRY_FIRST);
        assert_eq!(delays[1], RETRY_FIRST * 2);
        assert!(delays.windows(2).all(|w| w[0] <= w[1]));
        assert_eq!(*delays.last().unwrap(), RETRY_MAX);
        let r = f.read().unwrap();
        f.answered(r.ticket, Some((0, &snapshot("i", "1", "studio"), None)));
        assert_eq!(f.retry_delay(), None);
        // A refusal or malformed answer waits for a hint instead.
        let r = f.read().unwrap();
        let (followed, _) = f.answered(r.ticket, Some((0, "not json", None)));
        assert!(matches!(
            followed,
            Some(Followed::Fault { retry: false, .. })
        ));
        assert_eq!(f.retry_delay(), None);
    }

    #[test]
    fn an_invalid_instance_starts_from_host() {
        assert_eq!(Follower::new("bad name").binding().instance, "host");
    }
}
