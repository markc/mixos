// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The Terminal group of the Theme editor: one `terminal.*` field per edit,
//! sent to settingsd as a fenced `settings.apply` (or `settings.reset` for
//! the whole group) on the same channel as the Appearance panel. The values
//! shown are the ones settingsd last read; Preferences never writes a terminal
//! config file.
use crate::engine::Engine;
use serde_json::{Value, json};

/// The terminal's fields and ranges, for the Terminal group to draw.
pub use settings::terminal::{
    BORDER_WIDTH_MAX, BORDER_WIDTH_MIN, CursorShape, FONT_SIZE_MAX, FONT_SIZE_MIN, FocusRing,
    PADDINGS, SCROLL_SPEED_MAX, SCROLL_SPEED_MIN, SCROLLBACK_MAX, Terminal,
};

/// One edit to the terminal's preferences, as settingsd takes it.
#[derive(Clone, Debug, PartialEq)]
pub enum Edit {
    /// One field: its `terminal.*` path and its new value.
    Set(&'static str, Value),
    /// The whole group back to its defaults (`settings.reset`).
    Reset,
}

/// A control's new value.
#[derive(Clone, Debug, PartialEq)]
pub enum Setting {
    /// `None` is the system monospace family.
    Family(Option<String>),
    Size(f64),
    Weight(u32),
    Padding(u32),
    Scrollback(u32),
    ScrollSpeed(u32),
    BorderWidth(u32),
    CursorShape(CursorShape),
    CursorBlink(bool),
    FocusRing(FocusRing),
    FocusStrength(f64),
    CopyOnSelect(bool),
    Term(String),
}

impl Setting {
    /// The edit that sets this control's field.
    pub fn edit(&self) -> Edit {
        let (path, value) = match self {
            Setting::Family(family) => ("terminal.font.family", json!(family)),
            Setting::Size(size) => ("terminal.font.size", json!(size)),
            Setting::Weight(weight) => ("terminal.font.weight", json!(weight)),
            Setting::Padding(padding) => ("terminal.padding", json!(padding)),
            Setting::Scrollback(lines) => ("terminal.scrollback.lines", json!(lines)),
            Setting::ScrollSpeed(speed) => ("terminal.scroll.speed", json!(speed)),
            Setting::BorderWidth(width) => ("terminal.border.width", json!(width)),
            Setting::CursorShape(shape) => ("terminal.cursor.shape", json!(shape)),
            Setting::CursorBlink(blink) => ("terminal.cursor.blink", json!(blink)),
            Setting::FocusRing(ring) => ("terminal.focus.ring", json!(ring)),
            Setting::FocusStrength(strength) => ("terminal.focus.strength", json!(strength)),
            Setting::CopyOnSelect(copy) => ("terminal.mouse.copy_on_select", json!(copy)),
            Setting::Term(term) => ("terminal.env.term", json!(term)),
        };
        Edit::Set(path, value)
    }

    /// Why settingsd would refuse this value, as a label key, by the
    /// settings crate's own rules; `None` when it is sendable. Settingsd
    /// stays the authority: this only spares a round trip and says why.
    pub fn refusal(&self) -> Option<&'static str> {
        match self {
            Setting::Family(Some(family)) if !settings::terminal::valid_family(family) => {
                Some("terminal-family-refused")
            }
            Setting::Term(term) if !settings::terminal::valid_term(term) => {
                Some("terminal-term-refused")
            }
            _ => None,
        }
    }
}

/// The family a text field names. Blank (after trimming) is the system
/// monospace family, which settingsd stores as null.
pub fn family_setting(text: &str) -> Setting {
    let name = text.trim();
    Setting::Family((!name.is_empty()).then(|| name.to_owned()))
}

/// The text a family shows: blank for the system family, which the field's
/// placeholder names.
pub fn family_text(family: Option<&str>) -> String {
    family.unwrap_or_default().to_owned()
}

impl Engine {
    /// The Terminal group takes edits: settingsd read, a fence to send with,
    /// and no dialog, no quit under way.
    pub fn can_edit_terminal(&self) -> bool {
        self.look.terminal.is_some()
            && self.look.terminal_ready()
            && self.connected
            && self.ui.dialog.is_none()
            && !self.quitting
    }

    /// Apply one terminal control's value at once. A value settingsd would
    /// refuse is not sent; the status says why.
    pub fn edit_terminal(&mut self, setting: Setting) {
        if let Some(refusal) = setting.refusal() {
            self.status = self.label(refusal, &[]);
            self.failed = true;
            return;
        }
        if self.can_edit_terminal() {
            self.send_terminal(setting.edit());
        }
    }

    /// The Terminal group's reset action: every terminal field back to its
    /// default, and no other group.
    pub fn can_reset_terminal(&self) -> bool {
        self.can_edit_terminal()
    }

    pub fn reset_terminal(&mut self) {
        if self.can_reset_terminal() {
            self.send_terminal(Edit::Reset);
        }
    }

    fn send_terminal(&mut self, edit: Edit) {
        let operation = self.look.operation();
        self.status = self.label("applying-terminal", &[]);
        self.failed = false;
        self.call_settings(crate::appearance::Call::Terminal(operation, edit));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Effect;
    use citizen::Reply;

    #[test]
    fn a_blank_family_is_the_system_font() {
        let system = Edit::Set("terminal.font.family", Value::Null);
        assert_eq!(family_setting("").edit(), system);
        assert_eq!(family_setting("   ").edit(), system);
        assert_eq!(
            family_setting(" JetBrains Mono ").edit(),
            Edit::Set("terminal.font.family", json!("JetBrains Mono"))
        );
        // The system family shows as the field's placeholder, not as a name.
        assert_eq!(family_text(None), "");
        assert_eq!(family_text(Some("Iosevka")), "Iosevka");
    }

    #[test]
    fn each_control_sends_its_own_path_and_value() {
        let cases = [
            (Setting::Size(14.5), "terminal.font.size", json!(14.5)),
            (Setting::Weight(700), "terminal.font.weight", json!(700)),
            (Setting::Padding(16), "terminal.padding", json!(16)),
            (
                Setting::Scrollback(0),
                "terminal.scrollback.lines",
                json!(0),
            ),
            (
                Setting::ScrollSpeed(200),
                "terminal.scroll.speed",
                json!(200),
            ),
            (Setting::BorderWidth(4), "terminal.border.width", json!(4)),
            (
                Setting::CursorShape(CursorShape::Bar),
                "terminal.cursor.shape",
                json!("bar"),
            ),
            (
                Setting::CursorBlink(true),
                "terminal.cursor.blink",
                json!(true),
            ),
            (
                Setting::FocusRing(FocusRing::Accent),
                "terminal.focus.ring",
                json!("accent"),
            ),
            (
                Setting::FocusStrength(0.5),
                "terminal.focus.strength",
                json!(0.5),
            ),
            (
                Setting::CopyOnSelect(true),
                "terminal.mouse.copy_on_select",
                json!(true),
            ),
            (
                Setting::Term("xterm".into()),
                "terminal.env.term",
                json!("xterm"),
            ),
        ];
        for (setting, path, value) in cases {
            assert_eq!(setting.edit(), Edit::Set(path, value), "{path}");
        }
    }

    fn reply(body: Value) -> Result<Reply, citizen::CallError> {
        Ok(Reply {
            rc: 0,
            body: body.to_string(),
            error: None,
        })
    }

    /// A `settings.get` summary at `revision`, its terminal as given.
    fn snapshot(revision: &str, terminal: Value) -> Value {
        json!({"status":"current","summary":{"schema":1,
            "binding":{"instance":"example","profile":"default"},
            "incarnation":"inc-1","revision":revision,"design_revision":revision,
            "source_digest":"d","appearance":{"scheme":"studio","mode":"dark","contrast":"normal","source":null},
            "custom_source":false,"terminal":terminal},
            "publication_pending":false,"recovering":false})
    }

    fn label(key: &str, _: &[(&str, &str)]) -> String {
        key.to_owned()
    }

    /// An engine whose Appearance panel has read revision 7.
    fn read() -> Engine {
        let mut e = Engine::new(label);
        let listing = e
            .take_effects()
            .into_iter()
            .find_map(|x| match x {
                Effect::Releases { ticket, .. } => Some(ticket),
                _ => None,
            })
            .expect("the first listing");
        e.released(listing, reply(json!([])));
        e.take_effects();
        e.start_appearance("example");
        let calls = settings_calls(&mut e);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].1, "settings.get");
        e.settled(calls[0].0, reply(snapshot("7", json!({}))));
        e
    }

    /// The settings calls queued: (ticket, command, body).
    fn settings_calls(e: &mut Engine) -> Vec<(u64, &'static str, Value)> {
        e.take_effects()
            .into_iter()
            .filter_map(|x| match x {
                Effect::Settings { ticket, command, body } => Some((ticket, command, body)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn an_edit_is_one_fenced_apply_and_waits_for_its_read() {
        let mut e = read();
        assert!(e.can_edit_terminal());
        e.edit_terminal(Setting::Padding(1));
        let calls = settings_calls(&mut e);
        assert_eq!(calls.len(), 1);
        let (ticket, command, body) = calls[0].clone();
        assert_eq!(command, "settings.apply");
        assert_eq!(body["expected_incarnation"], "inc-1");
        assert_eq!(body["expected_revision"], "7");
        assert_eq!(body["changes"], json!({"terminal.padding": 1}));
        assert_eq!(body["reset"], json!([]));
        assert!(!e.can_edit_terminal(), "one apply at a time");
        e.edit_terminal(Setting::Padding(16));
        assert!(
            settings_calls(&mut e).is_empty(),
            "an edit mid-apply is not sent"
        );

        e.settled(ticket, reply(json!({"status":"changed","receipt":{}})));
        let reads = settings_calls(&mut e);
        assert_eq!(reads.len(), 1);
        assert_eq!(reads[0].1, "settings.get");
        assert!(!e.can_edit_terminal(), "a read must come first");
        e.settled(reads[0].0, reply(snapshot("8", json!({"padding": 1}))));
        assert_eq!(e.look.terminal.as_ref().map(|t| t.padding), Some(1));
        assert!(e.can_edit_terminal());
    }

    /// Text settingsd would refuse is refused here by the same rules, says
    /// why, and sends nothing.
    #[test]
    fn a_value_settingsd_refuses_is_not_sent() {
        for setting in [
            Setting::Term("XTERM".into()),
            Setting::Term("xterm 256color".into()),
            Setting::Term(String::new()),
            Setting::Term("x".repeat(65)),
            Setting::Family(Some("Mono\u{7}".into())),
            Setting::Family(Some("x".repeat(129))),
        ] {
            assert!(setting.refusal().is_some(), "{setting:?}");
            let mut e = read();
            e.edit_terminal(setting.clone());
            assert!(settings_calls(&mut e).is_empty(), "{setting:?} not sent");
            assert!(e.failed);
            assert_eq!(e.status, setting.refusal().unwrap());
            assert!(e.can_edit_terminal(), "nothing is in flight");
        }
        for setting in [
            Setting::Term("foot".into()),
            Setting::Family(None),
            Setting::Family(Some("Iosevka Term".into())),
        ] {
            assert_eq!(setting.refusal(), None, "{setting:?}");
            let mut e = read();
            e.edit_terminal(setting);
            assert_eq!(settings_calls(&mut e).len(), 1);
        }
    }

    #[test]
    fn reset_is_settings_reset_of_the_group_only() {
        let mut e = read();
        e.reset_terminal();
        let calls = settings_calls(&mut e);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].1, "settings.reset");
        assert_eq!(calls[0].2["changes"], json!({}));
        assert_eq!(calls[0].2["reset"], json!(["terminal"]));
    }
}
