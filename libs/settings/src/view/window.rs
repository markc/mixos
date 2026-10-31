// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! `window.*`: compd's chrome and behaviour, and the framing the toolkit and
//! compd decor share.
use super::{Reader, View};

#[derive(Clone, Debug, PartialEq)]
pub struct Window {
    pub hint_shown: bool,
    /// The active window outline, in pixels.
    pub active_hint: u32,
    /// `accent` or `#rrggbb`.
    pub hint_colour: String,
    /// The inner gap in pixels; the outer gap is always 0.
    pub gaps: u32,
    pub controls: Controls,
    pub clip_floating: bool,
    pub clip_tiled: bool,
    pub shadow_tiled: bool,
    /// `launcher`, `workspaces`, `applications` or `none`.
    pub super_key: String,
    pub edge_gravity: bool,
    pub focus_follows_cursor: bool,
    /// Used only while `focus_follows_cursor` (C2).
    pub focus_delay_ms: u32,
    pub cursor_follows_focus: bool,
    pub framing: Framing,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Controls {
    pub minimize: bool,
    pub maximize: bool,
}

/// The eight framing keys moved from `appearance.*`. The design names are
/// as authored: unknown names are retained and render as the default, so a
/// consumer resolves them with `design::WindowStyle::resolve` and friends.
#[derive(Clone, Debug, PartialEq)]
pub struct Framing {
    /// `auto`, `csd` or `ssd`.
    pub decorations: String,
    pub caption_side: String,
    pub style: String,
    pub menu_style: String,
    pub menu_side: String,
    pub actions_place: String,
    pub actions_align: String,
    pub title_align: String,
}

impl View for Framing {
    fn read(r: &Reader) -> Self {
        Self {
            decorations: r.text("window.decorations"),
            caption_side: r.text("window.caption_side"),
            style: r.text("window.style"),
            menu_style: r.text("window.menu_style"),
            menu_side: r.text("window.menu_side"),
            actions_place: r.text("window.actions_place"),
            actions_align: r.text("window.actions_align"),
            title_align: r.text("window.title_align"),
        }
    }
}

impl View for Window {
    fn read(r: &Reader) -> Self {
        Self {
            hint_shown: r.bool("window.hint_shown"),
            active_hint: r.uint("window.active_hint"),
            hint_colour: r.text("window.hint_colour"),
            gaps: r.uint("window.gaps"),
            controls: Controls {
                minimize: r.bool("window.controls.minimize"),
                maximize: r.bool("window.controls.maximize"),
            },
            clip_floating: r.bool("window.clip_floating"),
            clip_tiled: r.bool("window.clip_tiled"),
            shadow_tiled: r.bool("window.shadow_tiled"),
            super_key: r.text("window.super_key"),
            edge_gravity: r.bool("window.edge_gravity"),
            focus_follows_cursor: r.bool("window.focus_follows_cursor"),
            focus_delay_ms: r.uint("window.focus_delay_ms"),
            cursor_follows_focus: r.bool("window.cursor_follows_focus"),
            framing: View::read(r),
        }
    }
}
