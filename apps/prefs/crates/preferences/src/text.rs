// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The Text section: the style of each prose surface, saved in the profile.
//! The Email reader's body is `email.reader`, the composer `email.composer`,
//! and the Ced editor `ced.editor`. Each control is one fenced
//! `settings.apply` on the Terminal group's channel. The values shown are
//! settingsd's, from its last read; Prefs never keeps a copy.
use crate::engine::Engine;
use serde_json::{Value, json};

pub use settings::content::{
    Content, FAMILIES, FONT_SIZE_MAX, FONT_SIZE_MIN, MONO_FAMILY, PADDINGS,
};

/// A prose surface whose style the profile keeps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Surface {
    EmailReader,
    EmailComposer,
    CedEditor,
}

impl Surface {
    pub const ALL: [Surface; 3] = [
        Surface::EmailReader,
        Surface::EmailComposer,
        Surface::CedEditor,
    ];

    /// The settings path of this surface's style.
    pub fn path(self) -> &'static str {
        match self {
            Surface::EmailReader => "email.reader",
            Surface::EmailComposer => "email.composer",
            Surface::CedEditor => "ced.editor",
        }
    }

    /// The label key of this surface's card.
    pub fn title(self) -> &'static str {
        match self {
            Surface::EmailReader => "text-reader",
            Surface::EmailComposer => "text-composer",
            Surface::CedEditor => "text-editor",
        }
    }

    /// The bundled families this surface takes, after the design default.
    /// The editor lays text on a monospace grid, so it takes only that face.
    pub fn families(self) -> &'static [&'static str] {
        match self {
            Surface::CedEditor => &[MONO_FAMILY],
            _ => &FAMILIES,
        }
    }
}

/// One control's new value.
#[derive(Clone, Debug, PartialEq)]
pub enum Setting {
    Padding(Surface, u32),
    /// None is the design's default face: Inter for prose, the monospace face
    /// for the editor.
    Family(Surface, Option<String>),
    Size(Surface, f64),
    Light(Surface, bool),
}

/// One field edit: its full `settings.apply` path and its new value.
#[derive(Clone, Debug, PartialEq)]
pub struct TextEdit {
    pub path: String,
    pub value: Value,
}

impl Setting {
    pub fn surface(&self) -> Surface {
        match self {
            Setting::Padding(surface, _)
            | Setting::Family(surface, _)
            | Setting::Size(surface, _)
            | Setting::Light(surface, _) => *surface,
        }
    }

    /// The edit that sets this control's field.
    pub fn edit(&self) -> TextEdit {
        let (field, value) = match self {
            Setting::Padding(_, padding) => ("padding", json!(padding)),
            Setting::Family(_, family) => ("font.family", json!(family)),
            Setting::Size(_, size) => ("font.size", json!(size)),
            Setting::Light(_, light) => ("light", json!(light)),
        };
        TextEdit {
            path: format!("{}.{field}", self.surface().path()),
            value,
        }
    }

    /// Why settingsd would refuse this value, as a label key, by the same
    /// ranges and names the settings crate checks; `None` when it is
    /// sendable. Settingsd stays the authority.
    pub fn refusal(&self) -> Option<&'static str> {
        match self {
            Setting::Padding(_, padding) if !PADDINGS.contains(padding) => {
                Some("text-padding-refused")
            }
            Setting::Family(surface, Some(family))
                if !surface.families().contains(&family.as_str()) =>
            {
                Some("text-family-refused")
            }
            Setting::Size(_, size) if !(FONT_SIZE_MIN..=FONT_SIZE_MAX).contains(size) => {
                Some("text-size-refused")
            }
            _ => None,
        }
    }
}

impl Engine {
    /// The style a surface shows, from settingsd's last read.
    pub fn text_style(&self, surface: Surface) -> Option<Content> {
        match surface {
            Surface::EmailReader => self.look.email.as_ref().map(|e| e.reader.clone()),
            Surface::EmailComposer => self.look.email.as_ref().map(|e| e.composer.clone()),
            Surface::CedEditor => self.look.ced.as_ref().map(|c| c.editor.clone()),
        }
    }

    /// The Text section takes edits: settingsd read, a fence to send with,
    /// and no dialog, no quit under way.
    pub fn can_edit_text(&self) -> bool {
        self.look.email.is_some()
            && self.look.ced.is_some()
            && self.look.terminal_ready()
            && self.connected
            && self.ui.dialog.is_none()
            && !self.quitting
    }

    /// Apply one control's value at once. A value settingsd would refuse is
    /// not sent; the status says why.
    pub fn edit_text(&mut self, setting: Setting) {
        if let Some(refusal) = setting.refusal() {
            self.status = self.label(refusal, &[]);
            self.failed = true;
            return;
        }
        if self.can_edit_text() {
            self.send_text(setting.edit());
        }
    }

    fn send_text(&mut self, edit: TextEdit) {
        let operation = self.look.operation();
        self.status = self.label("applying-text", &[]);
        self.failed = false;
        self.call_settings(crate::appearance::Call::Text(operation, edit));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_control_sends_its_own_full_path() {
        let cases = [
            (
                Setting::Padding(Surface::EmailReader, 16),
                "email.reader.padding",
                json!(16),
            ),
            (
                Setting::Family(Surface::EmailComposer, Some("Inter".into())),
                "email.composer.font.family",
                json!("Inter"),
            ),
            (
                Setting::Size(Surface::CedEditor, 14.0),
                "ced.editor.font.size",
                json!(14.0),
            ),
            (
                Setting::Light(Surface::EmailReader, true),
                "email.reader.light",
                json!(true),
            ),
        ];
        for (setting, path, value) in cases {
            assert_eq!(setting.edit().path, path);
            assert_eq!(setting.edit().value, value);
        }
    }

    #[test]
    fn the_editor_refuses_the_prose_face_and_out_of_range_values() {
        assert_eq!(
            Setting::Family(Surface::CedEditor, Some("Inter".into())).refusal(),
            Some("text-family-refused")
        );
        assert_eq!(
            Setting::Family(Surface::CedEditor, Some(MONO_FAMILY.into())).refusal(),
            None
        );
        assert_eq!(
            Setting::Family(Surface::CedEditor, None).refusal(),
            None,
            "the default is the monospace face"
        );
        assert_eq!(
            Setting::Padding(Surface::EmailReader, 3).refusal(),
            Some("text-padding-refused")
        );
        assert_eq!(
            Setting::Size(Surface::EmailComposer, 4.0).refusal(),
            Some("text-size-refused")
        );
        assert_eq!(
            Setting::Family(Surface::EmailReader, Some("Inter".into())).refusal(),
            None
        );
    }
}
