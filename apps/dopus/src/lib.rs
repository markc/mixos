// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! dopus, the MixOS twin-pane file manager.
//!
//! The behaviour is the `dopus-core` engine (panes, listings, selection,
//! file operations, persisted layout). This crate is the egui shell around
//! it (AGENTS.md §4.1), and honours the core's seven laws:
//!
//! - law 1, tick for maintenance: [`shell`] ticks after every frame's work
//!   and arms a repaint for the core's next deadline;
//! - law 2, feed every core event through `on_event` once, on one thread:
//!   a pump thread forwards the core's channel and wakes the window
//!   ([`shell`]); [`headless`] owns a drainer thread instead;
//! - law 3, answer every dialog: [`app`] queues the core's confirm and
//!   prompt requests as dialogs and answers each one (dismissal is No);
//!   headless answers each No at once;
//! - law 4, the `OpenFile` handler: [`app`] spawns `xdg-open`; headless
//!   refuses and logs;
//! - law 5, `ascending: true` when switching sort columns: [`commands`] and
//!   the column headers in [`view`];
//! - law 6, validate prompt names with `validate_filename`: [`view`]'s
//!   prompt dialog;
//! - law 7, `set_split_ratio` from the divider: [`view`].
//!
//! Modules:
//! - [`bus`]: the Bus thread (`dopus.*` commands, topics, the session look);
//! - [`commands`]: the `dopus.v1` Bus surface, shared by window and headless;
//! - [`keymap`]: the default chords (the person's own come from the desktop
//!   profile);
//! - [`app`]: the window's state around the core; [`commands`]: the
//!   registry every action goes through; [`view`]: the drawing;
//!   [`shell`]: the running window.

pub mod actions;
pub mod app;
pub mod bus;
pub mod commands;
pub mod config;
pub mod dirs;
pub mod elide;
pub mod headless;
pub mod icons;
pub mod keymap;
pub mod shell;
pub mod view;

use toolkit::Strings;

/// The desktop identity.
pub const APP_ID: &str = "dev.mixos.dopus";

/// The English catalogue.
pub const CATALOGUE: &str = include_str!("../i18n/en/dopus.ftl");

/// The loaded catalogue.
pub fn strings() -> Strings {
    Strings::new(CATALOGUE)
}

thread_local! {
    static STRINGS: Strings = strings();
}

/// `key` from the catalogue.
pub fn label(key: &str) -> String {
    STRINGS.with(|s| s.get(key))
}

/// `key` from the catalogue with named arguments.
pub fn label_with(key: &str, args: &[(&str, &str)]) -> String {
    STRINGS.with(|s| s.with(key, args))
}
