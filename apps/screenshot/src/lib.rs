// SPDX-License-Identifier: MIT OR Apache-2.0
//! screenshot: native screenshots and editable annotations for MixOS.
//!
//! The behaviour is the `annotation` engine (the document, its painter, the
//! screenshots folder and the ledger) and [`app`], the window's state and
//! rules with no drawing in it. This crate is the egui shell around them
//! (AGENTS.md §4.1):
//!
//! - [`capture`]: one capture job over compd's verbs (hide, select, frame,
//!   restore);
//! - [`app`]: the state one serialisable struct describes, every action,
//!   the Bus verbs, and the effects left for the shell;
//! - [`commands`]: the registry every action goes through, from a menu, a
//!   shortcut, the toolbar or the Bus;
//! - [`jobs`]: the effects that leave the process (captures, files, the
//!   ledger), shared by the window and headless mode;
//! - [`view`]: the drawing; [`shell`]: the running window;
//! - [`headless`]: the same app and verbs with no window, for agents.
pub mod app;
pub mod capture;
pub mod commands;
pub mod headless;
pub mod ink;
pub mod jobs;
pub mod names;
pub mod shell;
pub mod verbs;
pub mod view;

use toolkit::Strings;

/// The desktop identity.
pub const APP_ID: &str = "dev.mixos.screenshot";

/// The Bus service and the prefix of every verb.
pub const SERVICE: &str = "screenshot";

/// The English catalogue.
pub const CATALOGUE: &str = include_str!("../i18n/en/screenshot.ftl");

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

/// The face annotation text is shaped with: the toolkit's embedded Inter,
/// so preview, clipboard and export never depend on host fonts.
pub fn face() -> std::sync::Arc<annotation::Face> {
    static FACE: std::sync::OnceLock<std::sync::Arc<annotation::Face>> = std::sync::OnceLock::new();
    FACE.get_or_init(|| {
        std::sync::Arc::new(
            annotation::Face::new(toolkit::fonts::inter_face(400).1)
                .expect("the embedded Inter face loads"),
        )
    })
    .clone()
}
