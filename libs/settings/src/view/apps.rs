// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! `apps_default.*`, `apps_startup.*` (xdg-export) and `xwayland.*` (compd).
use super::{Reader, View};
use crate::registry::CATEGORIES;
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq)]
pub struct Applications {
    /// Desktop-entry ids by category; a category not listed keeps what
    /// `mimeapps.list` holds.
    pub defaults: BTreeMap<String, String>,
    /// Desktop-entry ids started with the session.
    pub startup: Vec<String>,
}

impl View for Applications {
    fn read(r: &Reader) -> Self {
        Self {
            defaults: CATEGORIES
                .iter()
                .filter_map(|c| Some((c.to_string(), r.opt_text(&format!("apps_default.{c}"))?)))
                .collect(),
            startup: r.texts("apps_startup.entries"),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Xwayland {
    /// `none`, `modifiers`, `combinations` or `all`.
    pub global_shortcuts: String,
    pub mouse_events: bool,
    /// `gaming`, `applications` or `compatibility`.
    pub scaling: String,
    /// `none` or a connector.
    pub primary_output: String,
}

impl View for Xwayland {
    fn read(r: &Reader) -> Self {
        Self {
            global_shortcuts: r.text("xwayland.global_shortcuts"),
            mouse_events: r.bool("xwayland.mouse_events"),
            scaling: r.text("xwayland.scaling"),
            primary_output: r.text("xwayland.primary_output"),
        }
    }
}
