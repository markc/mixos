// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The settings key space v2 visual sign-off sheets (step S2), rendered
//! offscreen through wgpu. Not a gate: run on demand and look at them.
//!
//! ```text
//! SWATCH_OUT=/some/dir cargo test -p toolkit --test swatch_sheet -- --ignored
//! ```
//!
//! - `roles-<scheme>.png`, every authored scheme: its 31 roles and a small
//!   widget gallery, dark above light, the scheme as authored on the left
//!   and `mixos` from `seeds_of` of those roles on the right (the first
//!   tint's look, F4). `roles-mixos.png` shows the derived default with the
//!   palette's blue on the left and the green swatch on the right.
//! - `radii.png`: each style's own radii, then round, slightly round and
//!   square (A4), on the gallery.
//! - `density.png`: `mixos` dark at compact, comfortable and spacious (A5).

use design::geometry::{Density, Geometry, Roundness};
use design::{AccentChoice, DesignContext, Mode, Scheme, Style, Swatch};
use egui_kittest::Harness;
use image::RgbaImage;
use toolkit::Theme;

const PANEL: egui::Vec2 = egui::vec2(380.0, 1300.0);
const GALLERY: egui::Vec2 = egui::vec2(300.0, 190.0);

fn colour(linear: design::LinearRgba) -> egui::Color32 {
    let [r, g, b, a] = linear.to_srgba8();
    egui::Color32::from_rgba_unmultiplied(r, g, b, a)
}

fn gallery(ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        let _ = ui.button("Button");
        let _ = toolkit::selectable(ui, true, "Selected");
        let _ = toolkit::selectable(ui, false, "Plain");
    });
    let mut text = String::from("Text field");
    ui.text_edit_singleline(&mut text);
    ui.horizontal(|ui| {
        let mut on = true;
        ui.checkbox(&mut on, "Check");
        let mut value = 0.6_f32;
        ui.add(egui::Slider::new(&mut value, 0.0..=1.0));
    });
    ui.separator();
    ui.label("Body text, 0123456789");
    ui.weak("Weak text");
}

fn render(theme: &Theme, size: egui::Vec2, title: String, roles: bool) -> RgbaImage {
    let dictionary = theme.dictionary().clone();
    let mut harness = Harness::builder()
        .with_size(size)
        // A sheet to look at, not a gate: the gallery's fields are unnamed.
        .with_accessibility_check(false)
        .wgpu()
        .build_ui(move |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                ui.strong(&title);
                if roles && let Some(chrome) = &dictionary.chrome {
                    for role in design::family::chrome::ROLES {
                        let Some(value) = chrome.get(role) else {
                            continue;
                        };
                        ui.horizontal(|ui| {
                            let (rect, _) = ui
                                .allocate_exact_size(egui::vec2(44.0, 14.0), egui::Sense::hover());
                            ui.painter().rect_filled(rect, 0.0, colour(value));
                            let [r, g, b, a] = value.to_srgba8();
                            ui.monospace(format!("{role:<20} #{r:02x}{g:02x}{b:02x}{a:02x}"));
                        });
                    }
                    ui.separator();
                }
                gallery(ui);
            });
        });
    toolkit::install(&harness.ctx, theme);
    harness.run();
    harness.render().expect("offscreen render")
}

fn grid(cells: &[Vec<RgbaImage>]) -> RgbaImage {
    let width = cells[0].iter().map(RgbaImage::width).sum();
    let height = cells.iter().map(|row| row[0].height()).sum();
    let mut sheet = RgbaImage::new(width, height);
    let mut y = 0;
    for row in cells {
        let mut x = 0;
        for cell in row {
            image::imageops::replace(&mut sheet, cell, i64::from(x), i64::from(y));
            x += cell.width();
        }
        y += row[0].height();
    }
    sheet
}

fn with_geometry(theme: Theme, geometry: Geometry) -> Theme {
    let style = design::geometry::apply(theme.style(), &geometry);
    theme
        .with_overlay(toolkit::theme::Overlay {
            style: style.values(),
            ..Default::default()
        })
        .expect("geometry keeps a valid style")
}

fn context(scheme: Scheme, mode: Mode) -> DesignContext {
    DesignContext {
        scheme,
        mode,
        ..DesignContext::default()
    }
}

#[test]
#[ignore = "writes the S2 sign-off sheets to SWATCH_OUT"]
fn swatch_sheets() {
    let Some(out) = std::env::var_os("SWATCH_OUT").map(std::path::PathBuf::from) else {
        return;
    };
    std::fs::create_dir_all(&out).unwrap();
    for scheme in Scheme::AUTHORED {
        let mut rows = Vec::new();
        for mode in [Mode::Dark, Mode::Light] {
            let before = Theme::for_context(context(scheme, mode));
            let seeds = design::seeds::seeds_of(before.dictionary()).unwrap();
            let (accent, tints) = design::seeds::tint_batch(&seeds);
            let after = Theme::for_context(DesignContext {
                accent,
                tints,
                ..context(Scheme::Mixos, mode)
            });
            rows.push(vec![
                render(
                    &before,
                    PANEL,
                    format!("{} {} as authored", scheme.name(), mode.name()),
                    true,
                ),
                render(
                    &after,
                    PANEL,
                    format!("mixos from seeds_of({} {})", scheme.name(), mode.name()),
                    true,
                ),
            ]);
        }
        grid(&rows)
            .save(out.join(format!("roles-{}.png", scheme.name())))
            .unwrap();
    }
    let mut rows = Vec::new();
    for mode in [Mode::Dark, Mode::Light] {
        let green = AccentChoice::Swatch(Swatch::Green);
        rows.push(vec![
            render(
                &Theme::for_context(context(Scheme::Mixos, mode)),
                PANEL,
                format!("mixos {} (auto: blue)", mode.name()),
                true,
            ),
            render(
                &Theme::for_context(DesignContext {
                    accent: green,
                    ..context(Scheme::Mixos, mode)
                }),
                PANEL,
                format!("mixos {} green", mode.name()),
                true,
            ),
        ]);
    }
    grid(&rows).save(out.join("roles-mixos.png")).unwrap();

    let mut rows = Vec::new();
    for (style, scheme) in [
        (Style::Studio, Scheme::Studio),
        (Style::Adwaita, Scheme::Mixos),
        (Style::Pro, Scheme::Pro),
        (Style::Breeze, Scheme::Breeze),
        (Style::Classic, Scheme::Classic),
        (Style::Plain, Scheme::Ocean),
    ] {
        let theme = Theme::for_context(DesignContext {
            style: Some(style),
            ..context(scheme, Mode::Dark)
        });
        let mut row = vec![render(
            &theme,
            GALLERY,
            format!("{} today", style.name()),
            false,
        )];
        for roundness in Roundness::ALL {
            row.push(render(
                &with_geometry(
                    theme.clone(),
                    Geometry {
                        roundness,
                        density: Density::Standard,
                    },
                ),
                GALLERY,
                format!("{} {}", style.name(), roundness.name()),
                false,
            ));
        }
        rows.push(row);
    }
    grid(&rows).save(out.join("radii.png")).unwrap();

    let theme = Theme::for_context(context(Scheme::Mixos, Mode::Dark));
    let row = Density::ALL
        .into_iter()
        .map(|density| {
            render(
                &with_geometry(
                    theme.clone(),
                    Geometry {
                        roundness: Roundness::Round,
                        density,
                    },
                ),
                egui::vec2(300.0, 220.0),
                format!("mixos {}", density.name()),
                false,
            )
        })
        .collect();
    grid(&[row]).save(out.join("density.png")).unwrap();
}
