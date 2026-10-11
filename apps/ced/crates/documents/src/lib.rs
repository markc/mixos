// SPDX-License-Identifier: MIT OR Apache-2.0
//! documents: ced's engine, with no UI toolkit. The controller holds the
//! open documents (one tab per buffer of the `edit` Bus service, each a
//! mirror with its editing model, highlighting and diagnostics), runs every
//! action, serves the `ced.*` verbs and their waiters, and handles reconnects
//! and reattaches. It returns [`controller::Effect`]s; the window or the
//! headless host performs them (Bus calls, timers, clipboard, dialogs).
//!
//! - [`actions`]: every action's id, label and menu;
//! - [`keymap`]: their default chords;
//! - [`verbs`]: the `ced.*` request and reply types, with golden fixtures;
//! - [`config`]: `ced.conf.mix`; [`dirs`]: the app's files;
//! - [`session`]: the open tabs and recent files, restored at start;
//! - [`editor`]: what the editor view reports.

pub mod actions;
pub mod config;
pub mod controller;
pub mod dirs;
pub mod editor;
pub mod keymap;
pub mod session;
pub mod verbs;
