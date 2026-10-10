// SPDX-License-Identifier: MIT OR Apache-2.0
//! Offscreen snapshots (wgpu) of the chrome components in each chrome
//! theme: a window with an options bar, tool bar, icon rail, status bar,
//! document tabs, the canvas surround and scrollbars, and a dock of panel
//! groups holding value fields, sliders, toggles, a combo, text and search
//! fields and push buttons; then the same window with a dialog open, a
//! combo list open and a tooltip showing. Regenerate with
//! `UPDATE_SNAPSHOTS=1` and look at the images before committing them.

#[path = "support/fixture.rs"]
mod fixture;

use design::{Mode, Scheme};
use egui::accesskit::Role;
use egui::{CentralPanel, Frame, Id, Key, Ui, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use toolkit::bars;
use toolkit::button::{IconButton, PushButton};
use toolkit::chrome::Chrome;
use toolkit::dialog::{Choice, Dialog, Role as DialogRole};
use toolkit::field::{self, ValueField};
use toolkit::slider::{self, Slider};
use toolkit::tabs::{self, DocTab};
use toolkit::toggle::{Checkbox, Switch, Toggle};
use toolkit::{Icon, canvas, combo, panel};

/// The gallery's controls.
struct Gallery {
    opacity: f64,
    flow: f64,
    zoom: f64,
    size: f64,
    hue: f64,
    pressure: bool,
    resample: bool,
    snap: bool,
    blend: usize,
    name: String,
    query: String,
    offset: egui::Vec2,
    dialog: bool,
}

impl Default for Gallery {
    fn default() -> Self {
        Self {
            opacity: 100.0,
            flow: 64.0,
            zoom: 100.0,
            size: 20.0,
            hue: 210.0,
            pressure: true,
            resample: true,
            snap: false,
            blend: 0,
            name: "Background".into(),
            query: String::new(),
            offset: vec2(200.0, 150.0),
            dialog: false,
        }
    }
}

const BLENDS: [&str; 4] = ["Normal", "Multiply", "Screen", "Overlay"];

fn gallery(ui: &mut Ui, s: &mut Gallery) {
    let sizes = Chrome::of(ui.ctx()).metrics.bars;
    bars::options(ui, |ui| {
        ui.add(IconButton::new(Icon::Play).selected(true).tooltip("Brush"));
        ui.add(IconButton::new(Icon::Settings).tooltip("Brush settings (Ctrl+B)"));
        bars::divider(ui, bars::DIVIDER_HEIGHT);
        ui.label("Opacity:");
        ui.add(
            ValueField::new(&mut s.opacity, 0.0..=100.0)
                .unit("%")
                .width(64.0),
        );
        ui.label("Flow:");
        ui.add(
            ValueField::new(&mut s.flow, 0.0..=100.0)
                .unit("%")
                .width(64.0),
        );
        bars::divider(ui, bars::DIVIDER_HEIGHT);
        ui.add(Toggle::new(&mut s.pressure, "Pressure for Size"));
    });
    bars::status(ui, |ui| {
        ui.add(
            ValueField::new(&mut s.zoom, 1.0..=3200.0)
                .unit("%")
                .width(64.0),
        );
        ui.label("640 × 480 px (72 ppi)");
    });
    bars::tools(ui, |ui| {
        for (index, icon) in [Icon::Square, Icon::Search, Icon::Copy, Icon::Play, Icon::X]
            .into_iter()
            .enumerate()
        {
            ui.add(
                IconButton::new(icon)
                    .size(sizes.tool_button)
                    .selected(index == 3),
            );
        }
    });
    bars::rail(ui, |ui| {
        for (index, icon) in [Icon::Settings, Icon::Server, Icon::Info]
            .into_iter()
            .enumerate()
        {
            ui.add(
                IconButton::new(icon)
                    .size(sizes.rail_button)
                    .rail()
                    .selected(index == 1),
            );
        }
    });
    bars::dock(ui, |ui| {
        panel::group(
            ui,
            "colour",
            &["Color", "Swatches", "Gradients", "Patterns", "Brushes"],
            |ui, _| {
                slider::row(ui, "Size", &mut s.size, 1.0..=500.0, Some("px"));
                panel::section_label(ui, "Hue");
                let hues = (0..=6)
                    .map(|k| egui::ecolor::Hsva::new(k as f32 / 6.0, 1.0, 1.0, 1.0).into())
                    .collect();
                ui.add(Slider::new(&mut s.hue, 0.0..=360.0).gradient(hues));
            },
        );
        panel::group(ui, "properties", &["Properties", "Adjustments"], |ui, _| {
            ui.add(Checkbox::new(&mut s.resample, "Resample"));
            ui.add(Switch::new(&mut s.snap, "Snap to pixels"));
            ui.horizontal(|ui| {
                ui.label("Mode:");
                combo::show(ui, "blend", &mut s.blend, &BLENDS, None);
            });
            field::text(ui, &mut s.name, "Layer name", 200.0);
            field::search(ui, &mut s.query, "Search menus", 200.0);
            ui.horizontal(|ui| {
                ui.add(PushButton::primary("Apply").min_width(84.0));
                ui.add(PushButton::secondary("Reset").min_width(84.0));
            });
        });
    });
    CentralPanel::default().frame(Frame::NONE).show(ui, |ui| {
        let docs = [
            DocTab {
                title: "Untitled @ 100% (RGB/8)".into(),
                name: "Untitled".into(),
                meta: "RGB/8".into(),
                ..DocTab::default()
            },
            DocTab {
                title: "Photo @ 50% (RGB/16)".into(),
                name: "Photo".into(),
                meta: "RGB/16".into(),
                dirty: true,
                progress: Some(0.6),
            },
        ];
        tabs::documents(ui, Id::new("docs"), &docs, 0, None);
        let rect = ui.available_rect_before_wrap();
        canvas::surround(ui, rect);
        let view = canvas::scrollbars(
            ui,
            Id::new("canvas"),
            rect,
            vec2(1600.0, 1200.0),
            &mut s.offset,
        );
        let page = egui::Rect::from_center_size(view.center(), vec2(240.0, 160.0));
        ui.painter().rect_filled(page, 0.0, egui::Color32::WHITE);
    });
    if s.dialog {
        let response = Dialog::new("image-size", "Image Size")
            .buttons(vec![
                Choice::new("OK", DialogRole::Default),
                Choice::new("Cancel", DialogRole::Cancel),
            ])
            .show(ui.ctx(), |ui| {
                ui.horizontal(|ui| {
                    ui.label("Width:");
                    ui.add(
                        ValueField::new(&mut s.size, 1.0..=30_000.0)
                            .unit("px")
                            .width(96.0),
                    );
                });
                ui.add(Checkbox::new(&mut s.resample, "Resample"));
            });
        s.dialog = response.chosen.is_none();
    }
}

fn window(scheme: Scheme, mode: Mode, dialog: bool) -> Harness<'static, Gallery> {
    let state = Gallery {
        dialog,
        ..Gallery::default()
    };
    let mut h = Harness::builder()
        .with_size(vec2(960.0, 600.0))
        .wgpu()
        .build_ui_state(gallery, state);
    toolkit::install(&h.ctx, &fixture::theme(scheme, mode));
    h.run();
    h
}

#[test]
fn components_in_every_chrome_theme() {
    let mut results = egui_kittest::SnapshotResults::new();
    for (id, scheme, mode) in fixture::CHROME_THEMES {
        let mut h = window(scheme, mode, false);
        results.add(h.try_snapshot(format!("components_{id}")));
    }
}

#[test]
fn dialogs_in_pro_medium_studio_and_classic() {
    let mut results = egui_kittest::SnapshotResults::new();
    for (id, scheme, mode) in [
        ("proMedium", Scheme::Pro, Mode::Light),
        ("studio", Scheme::Studio, Mode::Dark),
        ("classic", Scheme::Classic, Mode::Light),
    ] {
        let mut h = window(scheme, mode, true);
        // Keyboard focus on the default button: its focus ring shows.
        h.get_by_label("OK").focus();
        h.run();
        results.add(h.try_snapshot(format!("dialog_{id}")));
    }
}

#[test]
fn an_open_combo_list_in_pro_medium_and_studio_light() {
    let mut results = egui_kittest::SnapshotResults::new();
    for (id, scheme, mode) in [
        ("proMedium", Scheme::Pro, Mode::Light),
        ("studioLight", Scheme::Studio, Mode::Light),
    ] {
        let mut h = window(scheme, mode, false);
        h.get_by_role(Role::ComboBox).click();
        h.run();
        h.key_press(Key::ArrowDown);
        h.run();
        results.add(h.try_snapshot(format!("combo_open_{id}")));
    }
}

#[test]
fn a_tooltip_with_its_shortcut_in_pro_dark() {
    let mut h = window(Scheme::Pro, Mode::Dark, false);
    let at = h.get_by_label("Brush settings (Ctrl+B)").rect().center();
    h.hover_at(at);
    // Rest still past the 0.35 s delay.
    h.run_steps(40);
    h.snapshot("tooltip_pro");
}
