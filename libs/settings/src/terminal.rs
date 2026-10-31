// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The terminal's preferences (`terminal.*`): text, layout, cursor, focus,
//! mouse and environment. Every default reproduces the term as it was before
//! these settings existed, except padding (8 px instead of 1 px) and the focus
//! ring: `accent` at strength 0.25 by default, subtler than the half-accent
//! ring the term drew. Colours are not here: the terminal follows the session theme.
//!
//! Every value is omitted from the serialised form at its default, and the
//! whole group while it is all defaults, so a profile written before it
//! re-serialises byte for byte and its seals still verify. Like
//! [`crate::keymap`], it does not touch the resolved design.
use crate::model::{Appearance, Diagnostic, deserialize_integer_u32, integer_u32};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const FONT_SIZE_MIN: f64 = 6.0;
pub const FONT_SIZE_MAX: f64 = 48.0;
pub const FONT_FAMILY_MAX_BYTES: usize = 128;
/// The padding choices, in logical pixels.
pub const PADDINGS: [u32; 5] = [1, 2, 4, 8, 16];
pub const SCROLLBACK_MAX: u32 = 1_000_000;
pub const SCROLL_SPEED_MIN: u32 = 10;
pub const SCROLL_SPEED_MAX: u32 = 200;
pub const BORDER_WIDTH_MIN: u32 = 1;
pub const BORDER_WIDTH_MAX: u32 = 4;
pub const TERM_MAX_BYTES: usize = 64;

fn is_default<T: Default + PartialEq>(value: &T) -> bool {
    *value == T::default()
}

fn error(path: &str, message: impl Into<String>) -> Diagnostic {
    Diagnostic::new("validation_failed", path, message)
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Terminal {
    #[serde(skip_serializing_if = "is_default")]
    pub font: Font,
    /// Space between the pane edge and the text, in logical pixels: one of
    /// [`PADDINGS`].
    #[serde(
        skip_serializing_if = "is_default_padding",
        deserialize_with = "deserialize_integer_u32"
    )]
    pub padding: u32,
    #[serde(skip_serializing_if = "is_default")]
    pub scrollback: Scrollback,
    #[serde(skip_serializing_if = "is_default")]
    pub scroll: Scroll,
    #[serde(skip_serializing_if = "is_default")]
    pub border: Border,
    #[serde(skip_serializing_if = "is_default")]
    pub cursor: Cursor,
    #[serde(skip_serializing_if = "is_default")]
    pub focus: Focus,
    #[serde(skip_serializing_if = "is_default")]
    pub mouse: Mouse,
    #[serde(skip_serializing_if = "is_default")]
    pub clipboard: Clipboard,
    #[serde(skip_serializing_if = "is_default")]
    pub env: Env,
}

impl Default for Terminal {
    fn default() -> Self {
        Self {
            font: Font::default(),
            padding: 8,
            scrollback: Scrollback::default(),
            scroll: Scroll::default(),
            border: Border::default(),
            cursor: Cursor::default(),
            focus: Focus::default(),
            mouse: Mouse::default(),
            clipboard: Clipboard::default(),
            env: Env::default(),
        }
    }
}
fn is_default_padding(padding: &u32) -> bool {
    *padding == 8
}

impl Terminal {
    pub fn is_default(&self) -> bool {
        is_default(self)
    }

    /// The family the terminal draws with: its own, else the session's
    /// monospace family. None is the bundled JetBrains Mono.
    pub fn font_family<'a>(&'a self, appearance: &'a Appearance) -> Option<&'a str> {
        self.font
            .family
            .as_deref()
            .or(appearance.mono_family.as_deref())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Font {
    /// A monospace family name, or None to follow `appearance.mono_family`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub family: Option<String>,
    /// Logical pixels, [`FONT_SIZE_MIN`] to [`FONT_SIZE_MAX`]; the design's
    /// UI type size by default.
    #[serde(skip_serializing_if = "is_default_font_size")]
    pub size: f64,
    /// A CSS weight, 100 to 900 in steps of 100.
    #[serde(
        skip_serializing_if = "is_default_font_weight",
        deserialize_with = "deserialize_integer_u32"
    )]
    pub weight: u32,
}

/// The design's UI type size, as the term used before this setting.
pub fn default_font_size() -> f64 {
    design::default_typography(design::TypographyRole::Ui).font_size
}
fn is_default_font_size(size: &f64) -> bool {
    *size == default_font_size()
}
fn is_default_font_weight(weight: &u32) -> bool {
    *weight == 400
}
impl Default for Font {
    fn default() -> Self {
        Self {
            family: None,
            size: default_font_size(),
            weight: 400,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Scrollback {
    /// Lines kept above the screen, 0 to [`SCROLLBACK_MAX`].
    #[serde(deserialize_with = "deserialize_integer_u32")]
    pub lines: u32,
}
impl Default for Scrollback {
    fn default() -> Self {
        Self { lines: 1000 }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Scroll {
    /// Rows per wheel line, scaled by speed / 40, [`SCROLL_SPEED_MIN`] to
    /// [`SCROLL_SPEED_MAX`]. Touchpad travel uses cell heights at the same scale.
    #[serde(deserialize_with = "deserialize_integer_u32")]
    pub speed: u32,
}
impl Default for Scroll {
    fn default() -> Self {
        Self { speed: 40 }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Border {
    /// The pane border in whole logical pixels, [`BORDER_WIDTH_MIN`] to
    /// [`BORDER_WIDTH_MAX`].
    #[serde(deserialize_with = "deserialize_integer_u32")]
    pub width: u32,
}
impl Default for Border {
    fn default() -> Self {
        Self { width: 1 }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Cursor {
    #[serde(skip_serializing_if = "is_default")]
    pub shape: CursorShape,
    #[serde(skip_serializing_if = "is_default")]
    pub blink: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CursorShape {
    Block,
    #[default]
    Underline,
    Bar,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Focus {
    #[serde(skip_serializing_if = "is_default")]
    pub ring: FocusRing,
    /// How much of the accent the ring takes, 0 to 1; used with
    /// [`FocusRing::Accent`].
    #[serde(skip_serializing_if = "is_default_focus_strength")]
    pub strength: f64,
}
fn is_default_focus_strength(strength: &f64) -> bool {
    *strength == 0.25
}
impl Default for Focus {
    fn default() -> Self {
        Self {
            ring: FocusRing::Accent,
            strength: 0.25,
        }
    }
}

/// The focused pane's ring: the card border tone, or the theme accent.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FocusRing {
    Neutral,
    #[default]
    Accent,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Mouse {
    /// Copy a selection to the clipboard as it is made.
    pub copy_on_select: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Clipboard {
    /// Answer OSC 52 clipboard queries: a program in the terminal (or on a
    /// remote host over ssh) may read the clipboard. Off by default; OSC 52
    /// writes (setting the clipboard) are always allowed.
    pub osc52_read: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Env {
    /// `TERM` for shells started after a change: 1–64 of `a-z`, `0-9`, `-`.
    pub term: String,
}
impl Default for Env {
    fn default() -> Self {
        Self {
            term: "xterm-256color".into(),
        }
    }
}

/// Whether `term` is a `TERM` value this field takes.
pub fn valid_term(term: &str) -> bool {
    !term.is_empty()
        && term.len() <= TERM_MAX_BYTES
        && term
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Whether `family` is a family name this field takes: 1–128 bytes, no
/// control characters, no surrounding space. Whether it is installed is the
/// terminal's check.
pub fn valid_family(family: &str) -> bool {
    !family.is_empty()
        && family.len() <= FONT_FAMILY_MAX_BYTES
        && !family.chars().any(char::is_control)
        && family.trim() == family
}

/// Check every range and shape. Enumerated values are checked as they are
/// read; whether a family is installed is the terminal's check.
pub fn validate(terminal: &Terminal) -> Result<(), Diagnostic> {
    let font = &terminal.font;
    if let Some(family) = &font.family
        && !valid_family(family)
    {
        return Err(error(
            "terminal.font.family",
            "Font family must be a plain name",
        ));
    }
    if !font.size.is_finite() || !(FONT_SIZE_MIN..=FONT_SIZE_MAX).contains(&font.size) {
        return Err(error(
            "terminal.font.size",
            format!("Expected {FONT_SIZE_MIN}..{FONT_SIZE_MAX}"),
        ));
    }
    if !(100..=900).contains(&font.weight) || !font.weight.is_multiple_of(100) {
        return Err(error(
            "terminal.font.weight",
            "Font weight must be 100 to 900 in steps of 100",
        ));
    }
    if !PADDINGS.contains(&terminal.padding) {
        return Err(error(
            "terminal.padding",
            "Padding must be 1, 2, 4, 8 or 16",
        ));
    }
    if terminal.scrollback.lines > SCROLLBACK_MAX {
        return Err(error(
            "terminal.scrollback.lines",
            format!("Expected 0..{SCROLLBACK_MAX}"),
        ));
    }
    if !(SCROLL_SPEED_MIN..=SCROLL_SPEED_MAX).contains(&terminal.scroll.speed) {
        return Err(error(
            "terminal.scroll.speed",
            format!("Expected {SCROLL_SPEED_MIN}..{SCROLL_SPEED_MAX}"),
        ));
    }
    if !(BORDER_WIDTH_MIN..=BORDER_WIDTH_MAX).contains(&terminal.border.width) {
        return Err(error(
            "terminal.border.width",
            format!("Expected {BORDER_WIDTH_MIN}..{BORDER_WIDTH_MAX}"),
        ));
    }
    let strength = terminal.focus.strength;
    if !strength.is_finite() || !(0.0..=1.0).contains(&strength) {
        return Err(error("terminal.focus.strength", "Expected 0..1"));
    }
    if !valid_term(&terminal.env.term) {
        return Err(error(
            "terminal.env.term",
            "Use 1–64 lowercase ASCII letters, digits or dash",
        ));
    }
    Ok(())
}

/// The `describe()` entries, one per field.
pub fn describe() -> Value {
    serde_json::json!({
        "terminal": {"type":"terminal", "null":"every terminal field back to its default"},
        "terminal.font.family": {"type":"string|null", "max_bytes":FONT_FAMILY_MAX_BYTES, "null":"appearance.mono_family"},
        "terminal.font.size": {"type":"number", "minimum":FONT_SIZE_MIN, "maximum":FONT_SIZE_MAX, "default":default_font_size()},
        "terminal.font.weight": {"type":"integer", "minimum":100, "maximum":900, "step":100, "default":400},
        "terminal.padding": {"type":"integer", "enum":PADDINGS, "default":8, "unit":"px"},
        "terminal.scrollback.lines": {"type":"integer", "minimum":0, "maximum":SCROLLBACK_MAX, "default":1000},
        "terminal.scroll.speed": {"type":"integer", "minimum":SCROLL_SPEED_MIN, "maximum":SCROLL_SPEED_MAX, "default":40, "unit":"rows per wheel line, scaled by speed/40"},
        "terminal.border.width": {"type":"integer", "minimum":BORDER_WIDTH_MIN, "maximum":BORDER_WIDTH_MAX, "default":1, "unit":"px"},
        "terminal.cursor.shape": {"type":"string", "enum":["block","underline","bar"], "default":"underline"},
        "terminal.cursor.blink": {"type":"boolean", "default":false},
        "terminal.focus.ring": {"type":"string", "enum":["neutral","accent"], "default":"accent"},
        "terminal.focus.strength": {"type":"number", "minimum":0.0, "maximum":1.0, "default":0.25, "used_when":"terminal.focus.ring is accent"},
        "terminal.mouse.copy_on_select": {"type":"boolean", "default":false},
        "terminal.clipboard.osc52_read": {"type":"boolean", "default":false},
        "terminal.env.term": {"type":"string", "pattern":"[a-z0-9-]{1,64}", "default":"xterm-256color", "applies":"to tabs and panes opened afterwards"}
    })
}

fn typed<T: serde::de::DeserializeOwned>(path: &str, value: Value) -> Result<T, Diagnostic> {
    serde_json::from_value(value).map_err(|e| error(path, e.to_string()))
}

/// A whole number, as Mix data sends it (48 or 48.0).
fn whole(path: &str, value: Value) -> Result<u32, Diagnostic> {
    integer_u32(typed::<f64>(path, value)?)
        .ok_or_else(|| error(path, "Expected an integral u32 number"))
}

/// Set (Some) or reset (None) one field. `field` is the path after
/// `terminal`; empty is the whole group.
pub(crate) fn set(
    terminal: &mut Terminal,
    path: &str,
    field: &[&str],
    value: Option<Value>,
) -> Result<(), Diagnostic> {
    let defaults = Terminal::default();
    match field {
        [] => {
            *terminal = value
                .filter(|v| !v.is_null())
                .map_or(Ok(defaults), |v| typed(path, v))?
        }
        ["font", "family"] => terminal.font.family = typed(path, value.unwrap_or(Value::Null))?,
        ["font", "size"] => {
            terminal.font.size = value.map_or(Ok(defaults.font.size), |v| typed(path, v))?
        }
        ["font", "weight"] => {
            terminal.font.weight = value.map_or(Ok(defaults.font.weight), |v| whole(path, v))?
        }
        ["padding"] => terminal.padding = value.map_or(Ok(defaults.padding), |v| whole(path, v))?,
        ["scrollback", "lines"] => {
            terminal.scrollback.lines =
                value.map_or(Ok(defaults.scrollback.lines), |v| whole(path, v))?
        }
        ["scroll", "speed"] => {
            terminal.scroll.speed = value.map_or(Ok(defaults.scroll.speed), |v| whole(path, v))?
        }
        ["border", "width"] => {
            terminal.border.width = value.map_or(Ok(defaults.border.width), |v| whole(path, v))?
        }
        ["cursor", "shape"] => {
            terminal.cursor.shape = value.map_or(Ok(defaults.cursor.shape), |v| typed(path, v))?
        }
        ["cursor", "blink"] => {
            terminal.cursor.blink = value.map_or(Ok(defaults.cursor.blink), |v| typed(path, v))?
        }
        ["focus", "ring"] => {
            terminal.focus.ring = value.map_or(Ok(defaults.focus.ring), |v| typed(path, v))?
        }
        ["focus", "strength"] => {
            terminal.focus.strength =
                value.map_or(Ok(defaults.focus.strength), |v| typed(path, v))?
        }
        ["mouse", "copy_on_select"] => {
            terminal.mouse.copy_on_select =
                value.map_or(Ok(defaults.mouse.copy_on_select), |v| typed(path, v))?
        }
        ["clipboard", "osc52_read"] => {
            terminal.clipboard.osc52_read =
                value.map_or(Ok(defaults.clipboard.osc52_read), |v| typed(path, v))?
        }
        ["env", "term"] => {
            terminal.env.term = value.map_or(Ok(defaults.env.term), |v| typed(path, v))?
        }
        _ => return Err(error(path, "Unknown field or scope")),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Desktop;
    use crate::v1::{patch, resolve};
    use serde_json::json;
    use std::collections::BTreeMap;

    /// Apply one change and resolve: Ok when the profile accepts it.
    fn apply(path: &str, value: Value) -> Result<Desktop, Diagnostic> {
        let next = patch(
            &Desktop::default(),
            &BTreeMap::from([(path.into(), value)]),
            &[],
        )?;
        resolve(&next).map_err(|mut errors| errors.remove(0))?;
        Ok(next)
    }

    #[test]
    fn an_old_profile_re_serialises_byte_for_byte() {
        let old = r#"{"appearance":{"scheme":"forest","mode":"dark","contrast":"normal","source":null},"ui":{"density":1.0,"text_scale":1.0,"reduced_motion":false},"shell":{"panels":{"bottom":{"edge":"bottom","mode":"dock","thickness":40}},"page_order":[]},"apps":{},"keymaps":{"dopus":{"nav.parent":["Ctrl+U"]}}}"#;
        let desktop: Desktop = serde_json::from_str(old).unwrap();
        assert!(desktop.terminal.is_default());
        assert_eq!(desktop.terminal.padding, 8);
        assert_eq!(serde_json::to_string(&desktop).unwrap(), old);
        let explicit = format!(
            r#"{},"terminal":{{"padding":1}}}}"#,
            old.strip_suffix('}').unwrap()
        );
        let desktop: Desktop = serde_json::from_str(&explicit).unwrap();
        assert_eq!(desktop.terminal.padding, 1);
        assert_eq!(serde_json::to_string(&desktop).unwrap(), explicit);
        assert!(
            !serde_json::to_string(&Desktop::default())
                .unwrap()
                .contains("terminal"),
            "omitted at its defaults"
        );
    }

    #[test]
    fn defaults_round_trip() {
        let t = Terminal::default();
        assert_eq!(t.font.family, None);
        assert_eq!(
            t.font.size,
            design::default_typography(design::TypographyRole::Ui).font_size
        );
        assert_eq!(
            (
                t.font.weight,
                t.padding,
                t.scrollback.lines,
                t.scroll.speed,
                t.border.width
            ),
            (400, 8, 1000, 40, 1)
        );
        assert_eq!(
            (t.cursor.shape, t.cursor.blink),
            (CursorShape::Underline, false)
        );
        assert_eq!((t.focus.ring, t.focus.strength), (FocusRing::Accent, 0.25));
        assert!(!t.mouse.copy_on_select);
        assert!(!t.clipboard.osc52_read);
        assert_eq!(t.env.term, "xterm-256color");
        validate(&t).unwrap();
        // Written out in full, every default reads back as the default and
        // serialises away again.
        let full = json!({"font":{"family":null,"size":t.font.size,"weight":400},
            "padding":8,"scrollback":{"lines":1000},"scroll":{"speed":40},"border":{"width":1},
            "cursor":{"shape":"underline","blink":false},"focus":{"ring":"accent","strength":0.25},
            "mouse":{"copy_on_select":false},"clipboard":{"osc52_read":false},
            "env":{"term":"xterm-256color"}});
        let read: Terminal = serde_json::from_value(full).unwrap();
        assert_eq!(read, t);
        assert_eq!(serde_json::to_value(&read).unwrap(), json!({}));
        // A changed value round-trips through JSON and Mix data (whole
        // numbers arrive as floats there).
        let mut changed = t.clone();
        changed.padding = 1;
        changed.cursor.shape = CursorShape::Bar;
        changed.focus.ring = FocusRing::Neutral;
        let text = serde_json::to_string(&changed).unwrap();
        assert_eq!(
            text,
            r#"{"padding":1,"cursor":{"shape":"bar"},"focus":{"ring":"neutral"}}"#
        );
        assert_eq!(serde_json::from_str::<Terminal>(&text).unwrap(), changed);
        assert_eq!(
            strict::from_str::<Terminal>(
                r#"{padding: 1.0, cursor: {shape: "bar"}, focus: {ring: "neutral"}}"#
            )
            .unwrap(),
            changed
        );
        // Each leaf resets to its default.
        let paths = [
            "terminal.font.family",
            "terminal.font.size",
            "terminal.font.weight",
            "terminal.padding",
            "terminal.scrollback.lines",
            "terminal.scroll.speed",
            "terminal.border.width",
            "terminal.cursor.shape",
            "terminal.cursor.blink",
            "terminal.focus.ring",
            "terminal.focus.strength",
            "terminal.mouse.copy_on_select",
            "terminal.env.term",
        ];
        let desktop = Desktop {
            terminal: changed,
            ..Default::default()
        };
        let reset: Vec<String> = paths.iter().map(|p| (*p).into()).collect();
        assert_eq!(
            patch(&desktop, &BTreeMap::new(), &reset).unwrap(),
            Desktop::default()
        );
        assert_eq!(
            patch(&desktop, &BTreeMap::new(), &["terminal".into()]).unwrap(),
            Desktop::default(),
            "the whole group"
        );
        let described = crate::v1::describe();
        assert_eq!(described["fields"]["terminal.padding"]["default"], 8);
        for path in paths {
            assert!(described["fields"].get(path).is_some(), "{path}");
        }
    }

    #[test]
    fn boundaries_are_accepted_and_one_step_outside_refused() {
        for (path, inside, outside) in [
            (
                "terminal.font.size",
                vec![json!(6.0), json!(48.0), json!(12)],
                vec![json!(5.9), json!(48.1), json!("12")],
            ),
            (
                "terminal.font.weight",
                vec![json!(100), json!(900), json!(400.0)],
                vec![json!(0), json!(1000), json!(450), json!(400.5)],
            ),
            (
                "terminal.padding",
                vec![
                    json!(1),
                    json!(2),
                    json!(4),
                    json!(8),
                    json!(16),
                    json!(16.0),
                ],
                vec![
                    json!(0),
                    json!(3),
                    json!(5),
                    json!(32),
                    json!(1.5),
                    json!(-1),
                ],
            ),
            (
                "terminal.scrollback.lines",
                vec![json!(0), json!(1_000_000)],
                vec![json!(1_000_001), json!(-1), json!(0.5)],
            ),
            (
                "terminal.scroll.speed",
                vec![json!(10), json!(200)],
                vec![json!(9), json!(201)],
            ),
            (
                "terminal.border.width",
                vec![json!(1), json!(4)],
                vec![json!(0), json!(5), json!(1.5)],
            ),
            (
                "terminal.cursor.shape",
                vec![json!("block"), json!("underline"), json!("bar")],
                vec![json!("beam"), json!("Block"), json!(1)],
            ),
            (
                "terminal.cursor.blink",
                vec![json!(true), json!(false)],
                vec![json!("yes"), json!(1)],
            ),
            (
                "terminal.focus.ring",
                vec![json!("neutral"), json!("accent")],
                vec![json!("violet"), json!(null)],
            ),
            (
                "terminal.focus.strength",
                vec![json!(0.0), json!(1.0), json!(0.5)],
                vec![json!(-0.01), json!(1.01)],
            ),
            (
                "terminal.mouse.copy_on_select",
                vec![json!(true)],
                vec![json!("true")],
            ),
            (
                "terminal.font.family",
                vec![json!(null), json!("Iosevka Term"), json!("x".repeat(128))],
                vec![
                    json!(""),
                    json!("Mono\u{7}"),
                    json!("Mono\n"),
                    json!(" Mono"),
                    json!("x".repeat(129)),
                ],
            ),
            (
                "terminal.env.term",
                vec![
                    json!("xterm-256color"),
                    json!("foot"),
                    json!("a"),
                    json!("x".repeat(64)),
                ],
                vec![
                    json!(""),
                    json!("XTERM"),
                    json!("xterm 256color"),
                    json!("xterm_256"),
                    json!("x".repeat(65)),
                    json!(null),
                ],
            ),
        ] {
            for value in inside {
                assert!(
                    apply(path, value.clone()).is_ok(),
                    "{path} = {value} accepted"
                );
            }
            for value in outside {
                let refused = apply(path, value.clone());
                assert_eq!(
                    refused.as_ref().map(|_| ()).unwrap_err().path,
                    path,
                    "{path} = {value} refused at its own path"
                );
            }
        }
    }

    #[test]
    fn unknown_keys_and_scopes_are_refused() {
        for value in [
            json!({"terminal":{"colours":"theme"}}),
            json!({"terminal":{"font":{"bold_bright":true}}}),
            json!({"terminal":{"cursor":{"style":"bar"}}}),
        ] {
            assert!(
                serde_json::from_value::<Desktop>(value.clone()).is_err(),
                "{value}"
            );
        }
        for path in [
            "terminal.colours",
            "terminal.font",
            "terminal.font.bold_bright",
            "terminal.cursor",
        ] {
            assert!(apply(path, json!(1)).is_err(), "{path}");
        }
        // The whole group takes a complete record, strictly.
        assert!(apply("terminal", json!({"padding":4})).is_ok());
        assert!(apply("terminal", json!({"padding":4,"typo":1})).is_err());
        assert!(apply("terminal", json!({"padding":3})).is_err());
    }

    #[test]
    fn null_resets_the_whole_terminal_group() {
        let changed = apply(
            "terminal",
            json!({"padding":8,"font":{"size":20,"weight":700}}),
        )
        .unwrap();
        let reset = patch(
            &changed,
            &BTreeMap::from([("terminal".into(), Value::Null)]),
            &[],
        )
        .unwrap();
        assert_eq!(reset, Desktop::default());
        resolve(&reset).unwrap();
    }

    #[test]
    fn the_family_follows_the_session_mono_family_and_the_design_is_untouched() {
        let mut desktop = Desktop::default();
        assert_eq!(desktop.terminal.font_family(&desktop.appearance), None);
        desktop.appearance.mono_family = Some("Iosevka".into());
        assert_eq!(
            desktop.terminal.font_family(&desktop.appearance),
            Some("Iosevka")
        );
        desktop.terminal.font.family = Some("Fira Code".into());
        assert_eq!(
            desktop.terminal.font_family(&desktop.appearance),
            Some("Fira Code")
        );
        let changed = apply("terminal.padding", json!(16)).unwrap();
        assert_eq!(
            crate::digest(&resolve(&changed).unwrap()).unwrap(),
            crate::digest(&resolve(&Desktop::default()).unwrap()).unwrap(),
            "terminal settings do not touch the effective design"
        );
    }
}
