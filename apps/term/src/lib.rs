// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Terminal: MixOS's tabbed terminal, with split panes, drawn on the GPU.
//!
//! The behaviour is the `term-core` engine (PTYs, the VT grid, tabs and
//! panes, selection, mouse reporting, the `term.*` Bus verbs and the native
//! session); the grid is drawn by `term-render`. This crate is the egui
//! shell around them (AGENTS.md §4.1):
//!
//! - [`app`]: the window's state around the tab set;
//! - [`input`], [`mouse`], [`ime`]: keys, pointer and input method into
//!   terminal actions and PTY input;
//! - [`layout`]: pane rectangles and grid sizes on whole physical pixels;
//! - [`commands`]: the registry every action goes through;
//! - [`bus`]: the window's own verbs on the `term` name;
//! - [`view`]: the drawing; [`shell`]: the running window.

pub mod app;
pub mod bus;
pub mod clipboard;
pub mod commands;
mod font;
pub mod ime;
pub mod input;
pub mod layout;
pub mod mouse;
pub mod selection;
pub mod shell;
pub mod view;

#[cfg(test)]
mod tests;

use toolkit::Strings;

/// The Wayland app id and desktop file name.
pub const APP_ID: &str = "dev.mixos.term";

/// The English catalogue.
pub const CATALOGUE: &str = include_str!("../i18n/en/term.ftl");

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
