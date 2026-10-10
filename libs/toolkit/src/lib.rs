// SPDX-License-Identifier: MIT OR Apache-2.0
//! toolkit: the shared shell for MixOS egui applications (AGENTS.md §4.1).
//!
//! - [`theme`]: the resolved design for one scheme, mode and contrast;
//! - [`theme_menu`]: the View › Theme submenu that picks it, in every app;
//! - [`style`]: that design as an egui style (colours, spacing, radii, type);
//! - [`chrome`]: title-bar and menu colours and geometry, and the whole
//!   style of the chrome schemes (`pro`, `studio`, `classic`, `adwaita`, `solarized`);
//! - [`menu`]: menu bars and menus with shared pointer/keyboard navigation;
//! - [`fonts`]: the embedded Inter and JetBrains Mono faces, chosen by weight;
//! - [`icons`]: Lucide icons, stroke weight and colour from the theme;
//! - [`command`]: the command registry menus, shortcuts and Bus verbs come from;
//! - [`drive`]: the UI-level Bus verbs every app shares (widget tree, injected
//!   pointer and keys, menus, window commands, captures);
//! - [`strings`]: Fluent catalogues for every user-visible string;
//! - [`titlebar`]: the client-side title bar (icon, menus, title, right-hand
//!   controls, window buttons, drag and resize), for windows opened with
//!   [`titlebar::viewport`].
//!
//! The chrome components, each drawn to the chrome specification in every
//! chrome scheme (and in the hue schemes from their own style):
//!
//! - [`bars`]: options bar, status bar, tool bar, icon rail, dividers;
//! - [`button`]: icon buttons, primary and secondary push buttons, links;
//! - [`panel`]: dock panel groups (Pro tab strips, Studio cards and pills),
//!   section labels, the drop line;
//! - [`tabs`]: tab overflow and document tabs;
//! - [`field`]: numeric value fields, text and search fields;
//! - [`toggle`]: checkboxes and switches;
//! - [`combo`]: combo boxes;
//! - [`slider`]: sliders and slider rows;
//! - [`canvas`]: the canvas surround and its scrollbars;
//! - [`tooltip`]: tooltip wording;
//! - [`dialog`]: modal dialogs and their button rows.
//!
//! An application calls [`install`] once on its context, keeps its behaviour
//! in a headless engine, holds its UI state in one serialisable struct and
//! dispatches every action through its [`command::Registry`].

pub mod bars;
pub mod button;
pub mod canvas;
pub mod chrome;
pub mod combo;
pub mod command;
pub mod dialog;
pub mod drive;
pub mod field;
pub mod fonts;
pub mod icons;
pub mod menu;
pub mod panel;
pub mod slider;
pub mod strings;
pub mod style;
pub mod tabs;
pub mod theme;
pub mod theme_menu;
pub mod titlebar;
pub mod toggle;
pub mod tooltip;

pub use command::{Command, Registry};
pub use icons::Icon;
pub use strings::Strings;
pub use theme::Theme;

/// Prepare `ctx` for a MixOS application: `theme`'s style and fonts, and the
/// image loaders the icons need.
pub fn install(ctx: &egui::Context, theme: &Theme) {
    egui_extras::install_image_loaders(ctx);
    style::apply(ctx, theme);
}
