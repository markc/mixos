// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! `shell.panels.<panel>.*`: Quoin's two panels, `top` (COSMIC's Panel) and
//! `bottom` (its Dock, Quoin's panel today).
use super::{Reader, View};

#[derive(Clone, Debug, PartialEq)]
pub struct Panels {
    pub top: Panel,
    pub bottom: Panel,
}

impl Panels {
    /// The panel by id, `top` or `bottom`.
    pub fn get(&self, id: &str) -> Option<&Panel> {
        match id {
            "top" => Some(&self.top),
            "bottom" => Some(&self.bottom),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Panel {
    pub enabled: bool,
    /// `top`, `bottom`, `left` or `right`.
    pub edge: String,
    /// `dock` (an exclusive zone) or `overlay`.
    pub mode: String,
    pub autohide: bool,
    /// `all` or a connector.
    pub output: String,
    pub gap: bool,
    pub extend: bool,
    /// `match`, `light` or `dark`.
    pub appearance: String,
    /// One of 32, 40, 56, 64, 80 pixels.
    pub thickness: u32,
    pub opacity: f64,
    pub applets: Applets,
}

/// Applet ids per section. None is Quoin's own order (the default is the
/// owner's, not the registry's).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Applets {
    pub start: Option<Vec<String>>,
    pub center: Option<Vec<String>>,
    pub end: Option<Vec<String>>,
}

fn panel(r: &Reader, id: &str) -> Panel {
    let key = |leaf: &str| format!("shell.panels.{id}.{leaf}");
    let applets = |section: &str| {
        r.authored(&key(&format!("applets.{section}")))
            .map(|v| super::strings(&v))
    };
    Panel {
        enabled: r.bool(&key("enabled")),
        edge: r.text(&key("edge")),
        mode: r.text(&key("mode")),
        autohide: r.bool(&key("autohide")),
        output: r.text(&key("output")),
        gap: r.bool(&key("gap")),
        extend: r.bool(&key("extend")),
        appearance: r.text(&key("appearance")),
        thickness: r.uint(&key("thickness")),
        opacity: r.number(&key("opacity")),
        applets: Applets {
            start: applets("start"),
            center: applets("center"),
            end: applets("end"),
        },
    }
}

impl View for Panels {
    fn read(r: &Reader) -> Self {
        Self {
            top: panel(r, "top"),
            bottom: panel(r, "bottom"),
        }
    }
}
