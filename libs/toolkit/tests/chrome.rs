// SPDX-License-Identifier: MIT OR Apache-2.0
//! Offscreen snapshots of the chrome schemes (wgpu): the title bar with File
//! open, the keyboard four rows down on a submenu row and Right pressed into
//! its submenu, in each chrome scheme and mode; and the caption buttons'
//! hover fills. Regenerate with `UPDATE_SNAPSHOTS=1` and look at the images
//! before committing them.

#[path = "support/fixture.rs"]
mod fixture;

use design::{Mode, Scheme};
use egui::Key;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;

fn window(scheme: Scheme, mode: Mode) -> Harness<'static, fixture::Fixture> {
    let builder = Harness::builder().with_size(egui::vec2(760.0, 340.0)).wgpu();
    fixture::harness(builder, &fixture::theme(scheme, mode))
}

fn submenu_open(scheme: Scheme, mode: Mode, name: &str) {
    let mut h = window(scheme, mode);
    h.get_by_label("File").click();
    h.run();
    for key in [Key::ArrowDown, Key::ArrowDown, Key::ArrowDown, Key::ArrowDown, Key::ArrowRight] {
        h.key_press(key);
        h.run();
    }
    h.snapshot(name);
}

fn caption_hover(scheme: Scheme, mode: Mode, caption: &str, name: &str) {
    let mut h = window(scheme, mode);
    let at = h.get_by_label(caption).rect().center();
    h.hover_at(at);
    h.run();
    h.snapshot(name);
}

#[test]
fn chrome_pro_dark() {
    submenu_open(Scheme::Pro, Mode::Dark, "chrome_pro_dark");
}

#[test]
fn chrome_pro_light() {
    submenu_open(Scheme::Pro, Mode::Light, "chrome_pro_light");
}

#[test]
fn chrome_studio_dark() {
    submenu_open(Scheme::Studio, Mode::Dark, "chrome_studio_dark");
}

#[test]
fn chrome_studio_light() {
    submenu_open(Scheme::Studio, Mode::Light, "chrome_studio_light");
}

#[test]
fn chrome_classic() {
    submenu_open(Scheme::Classic, Mode::Light, "chrome_classic");
}

/// View › Theme open in Pro Medium Gray, Forest chosen: the check-row
/// gutter and tick.
#[test]
fn theme_submenu_pro_light() {
    let builder = Harness::builder().with_size(egui::vec2(760.0, 520.0)).wgpu();
    let mut h = fixture::harness(builder, &fixture::theme(Scheme::Pro, Mode::Light));
    h.state_mut().theme = (Some(Scheme::Forest), None);
    h.get_by_label("View").click();
    h.run();
    for key in [Key::ArrowDown, Key::ArrowDown, Key::ArrowRight] {
        h.key_press(key);
        h.run();
    }
    h.snapshot("theme_submenu_pro_light");
}

#[test]
fn caption_close_hover_pro_light() {
    caption_hover(Scheme::Pro, Mode::Light, "Close", "caption_close_hover_pro_light");
}

#[test]
fn caption_minimize_hover_studio_light() {
    caption_hover(Scheme::Studio, Mode::Light, "Minimize", "caption_minimize_hover_studio_light");
}
