// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The page tree in COSMIC Settings' order (spec 1 §2), with the MixOS app
//! pages under Applications and Software under System & accounts. A page
//! still in the stage 5 work list is [`Body::Pending`].
use super::{Body, Page, page};

fn pending(id: &'static str, icon: &'static str, parent: &'static str) -> Page {
    page(id, icon, Some(parent), Body::Pending)
}

fn list(id: &'static str, icon: &'static str) -> Page {
    page(id, icon, None, Body::List)
}

fn custom(id: &'static str, icon: &'static str, parent: &'static str) -> Page {
    page(id, icon, Some(parent), Body::Custom { editor: id })
}

pub(super) fn pages() -> Vec<Page> {
    let mut pages = vec![super::network::network()];
    pages.extend(super::network::children());
    pages.extend([page("bluetooth", "bluetooth", None, Body::Pending)]);
    pages.extend(super::accessibility::pages());
    pages.extend([
        list("desktop", "layout-dashboard"),
        custom("wallpaper", "image", "desktop"),
        super::appearance::page(),
        super::panels::panel(),
        pending("panel_applets", "blocks", "panel"),
        super::panels::dock(),
        pending("dock_applets", "blocks", "dock"),
        super::window_management::page(),
        super::workspaces::page(),
        page("displays", "monitor", None, Body::Pending),
        page("sound", "volume-2", None, Body::Pending),
        pending("sound_applications", "audio-lines", "sound"),
        pending("sound_profiles", "speaker", "sound"),
        page("power", "battery-medium", None, Body::Pending),
        list("input", "keyboard"),
        super::input::keyboard(),
        pending("keyboard_shortcuts", "command", "keyboard"),
        pending("mouse", "mouse", "input"),
        pending("tablet", "pen-tool", "input"),
        pending("touchpad", "touchpad", "input"),
        list("applications", "layout-grid"),
        pending("default_apps", "star", "applications"),
        pending("startup_apps", "rocket", "applications"),
        pending("x11", "square-terminal", "applications"),
        custom("term", "file-terminal", "applications"),
        custom("email", "mail", "applications"),
        custom("ced", "file-code", "applications"),
        list("time", "clock"),
        super::time::date_time(),
        super::time::region(),
        list("system", "user"),
        pending("users", "users", "system"),
        pending("about", "info", "system"),
        custom("software", "package", "system"),
    ]);
    pages
}
