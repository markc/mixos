// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! `displays.<connector>.*`, applied by monitord: only confirmed values.
use super::{Reader, View, family};
use std::collections::BTreeMap;

/// The outputs with an authored value, by connector. An output not listed
/// takes every default ([`Display::default`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Displays(pub BTreeMap<String, Display>);

impl Displays {
    /// `connector`'s settings, its defaults where nothing was authored.
    pub fn get(&self, connector: &str) -> Display {
        self.0.get(connector).cloned().unwrap_or_default()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Display {
    pub enabled: bool,
    pub mirror_of: Option<String>,
    /// `WxH@R.RRR`; None is the preferred mode.
    pub mode: Option<String>,
    /// `always`, `auto` or `off`.
    pub vrr: String,
    /// Percent, 50 to 320 in steps of 5; None is monitord's.
    pub scale: Option<u32>,
    /// `normal`, `90`, `180` or `270`.
    pub rotation: String,
    /// `x,y` in logical pixels; None lets monitord lay it out.
    pub position: Option<String>,
}

impl Default for Display {
    /// The registry's defaults.
    fn default() -> Self {
        Self {
            enabled: true,
            mirror_of: None,
            mode: None,
            vrr: "auto".into(),
            scale: None,
            rotation: "normal".into(),
            position: None,
        }
    }
}

impl View for Displays {
    fn read(r: &Reader) -> Self {
        Self(family(r, "displays.", 1, |c| {
            let key = |leaf: &str| format!("displays.{c}.{leaf}");
            Display {
                enabled: r.bool(&key("enabled")),
                mirror_of: r.opt_text(&key("mirror_of")),
                mode: r.opt_text(&key("mode")),
                vrr: r.text(&key("vrr")),
                scale: r.opt_uint(&key("scale")),
                rotation: r.text(&key("rotation")),
                position: r.opt_text(&key("position")),
            }
        }))
    }
}
