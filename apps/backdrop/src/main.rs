// SPDX-License-Identifier: MIT OR Apache-2.0
//! backdrop: the MixOS desktop background.
//!
//! This first stage is the probe: an egui surface on compd's Background
//! layer on every output (`toolkit::layer`), drawn in the session theme,
//! that counts pointer clicks. It proves the layer surfaces, the GL path
//! and input before the wallpapers and the folder view are built on it.
//! Each fact a test needs is printed as one JSON line on stdout: `drawn`
//! after a pass at a new size or scale, and `click` for each primary press.

use toolkit::layer::{LayerOptions, LayerProgram, SurfaceInfo};

const CATALOGUE: &str = include_str!("../i18n/en/backdrop.ftl");

struct Probe {
    strings: toolkit::Strings,
    theme: toolkit::Theme,
    clicks: u32,
    last: Option<egui::Pos2>,
    /// The surfaces last reported as drawn.
    reported: Vec<SurfaceInfo>,
}

fn emit(line: serde_json::Value) {
    println!("{line}");
}

impl LayerProgram for Probe {
    fn setup(&mut self, ctx: &egui::Context) {
        toolkit::install(ctx, &self.theme);
    }

    fn show(&mut self, ui: &mut egui::Ui, surface: &SurfaceInfo) {
        let pressed = ui.input(|i| {
            i.pointer
                .button_pressed(egui::PointerButton::Primary)
                .then(|| i.pointer.interact_pos())
                .flatten()
        });
        if let Some(pos) = pressed {
            self.clicks += 1;
            self.last = Some(pos);
            emit(serde_json::json!({
                "event": "click",
                "x": pos.x,
                "y": pos.y,
                "output": surface.output,
            }));
        }
        egui::CentralPanel::default().show(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(ui.available_height() / 3.0);
                ui.heading(self.strings.get("probe-title"));
                ui.label(self.strings.with(
                    "probe-surface",
                    &[
                        ("width", &surface.width.to_string()),
                        ("height", &surface.height.to_string()),
                        ("scale", &format!("{:.2}", surface.scale)),
                    ],
                ));
                ui.label(match self.last {
                    Some(pos) => self.strings.with(
                        "probe-clicks",
                        &[
                            ("count", &self.clicks.to_string()),
                            ("x", &format!("{:.0}", pos.x)),
                            ("y", &format!("{:.0}", pos.y)),
                        ],
                    ),
                    None => self.strings.get("probe-no-clicks"),
                });
            });
        });
        if !self.reported.contains(surface) {
            self.reported.retain(|r| r.output != surface.output);
            self.reported.push(surface.clone());
            emit(serde_json::json!({
                "event": "drawn",
                "output": surface.output,
                "width": surface.width,
                "height": surface.height,
                "scale": surface.scale,
            }));
        }
    }
}

fn main() {
    let probe = Probe {
        strings: toolkit::Strings::new(CATALOGUE),
        theme: toolkit::Theme::load(),
        clicks: 0,
        last: None,
        reported: Vec::new(),
    };
    if let Err(error) = toolkit::layer::run(LayerOptions::background("backdrop"), probe) {
        eprintln!("backdrop: {error}");
        std::process::exit(1);
    }
}
