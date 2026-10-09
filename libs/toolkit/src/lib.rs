// SPDX-License-Identifier: MIT OR Apache-2.0
//! toolkit: the shared shell for MixOS egui applications (AGENTS.md §4.1).
//!
//! - [`theme`]: the resolved design for one scheme, mode and contrast;
//! - [`style`]: that design as an egui style (colours, spacing, radii, type);
//! - [`chrome`]: title-bar and menu colours and geometry, and the whole
//!   style of the chrome schemes (`pro`, `studio`, `classic`);
//! - [`menu`]: menu bars and menus with shared pointer/keyboard navigation;
//! - [`fonts`]: the embedded Inter and JetBrains Mono faces, chosen by weight;
//! - [`icons`]: Lucide icons, stroke weight and colour from the theme;
//! - [`command`]: the command registry menus, shortcuts and Bus verbs come from;
//! - [`strings`]: Fluent catalogues for every user-visible string;
//! - [`titlebar`]: the client-side title bar (icon, menus, title, window
//!   buttons, drag and resize), for windows opened with [`titlebar::viewport`].
//!
//! An application calls [`install`] once on its context, keeps its behaviour
//! in a headless engine, holds its UI state in one serialisable struct and
//! dispatches every action through its [`command::Registry`].

pub mod chrome;
pub mod command;
pub mod fonts;
pub mod icons;
pub mod menu;
pub mod strings;
pub mod style;
pub mod theme;
pub mod titlebar;

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
