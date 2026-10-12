// SPDX-License-Identifier: MIT OR Apache-2.0
//! backdrop: the MixOS desktop background.
//!
//! This first stage is the probe: one egui surface on compd's Background
//! layer, drawn in the session theme, that counts pointer clicks. It proves
//! the layer surface, the GL path and input before the wallpapers and the
//! folder view are built on it. Each fact a test needs is printed as one
//! JSON line on stdout: `drawn` after the first painted pass at a size, and
//! `click` for each primary press.

mod layer;

use layer::{LayerOptions, Program, SurfaceInfo};
use smithay_client_toolkit::shell::wlr_layer::{KeyboardInteractivity, Layer};

const CATALOGUE: &str = include_str!("../i18n/en/backdrop.ftl");

struct Probe {
    strings: toolkit::Strings,
    clicks: u32,
    last: Option<egui::Pos2>,
    /// The size last reported as drawn.
    reported: Option<SurfaceInfo>,
}

fn emit(line: serde_json::Value) {
    println!("{line}");
}

impl Program for Probe {
    fn setup(&mut self, ctx: &egui::Context) {
        toolkit::install(ctx, &toolkit::Theme::load());
    }

    fn show(&mut self, ui: &mut egui::Ui, surface: SurfaceInfo) {
        let pressed = ui.input(|i| {
            i.pointer
                .button_pressed(egui::PointerButton::Primary)
                .then(|| i.pointer.interact_pos())
                .flatten()
        });
        if let Some(pos) = pressed {
            self.clicks += 1;
            self.last = Some(pos);
            emit(serde_json::json!({"event": "click", "x": pos.x, "y": pos.y}));
        }
        egui::CentralPanel::default().show_inside(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(ui.available_height() / 3.0);
                ui.heading(self.strings.get("probe-title"));
                ui.label(self.strings.with(
                    "probe-surface",
                    &[
                        ("width", &surface.width.to_string()),
                        ("height", &surface.height.to_string()),
                        ("scale", &surface.scale.to_string()),
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
        if self.reported != Some(surface) {
            self.reported = Some(surface);
            emit(serde_json::json!({
                "event": "drawn",
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
        clicks: 0,
        last: None,
        reported: None,
    };
    let options = LayerOptions {
        layer: Layer::Background,
        namespace: "backdrop".into(),
        keyboard: KeyboardInteractivity::None,
    };
    if let Err(error) = layer::run(options, probe) {
        eprintln!("backdrop: {error}");
        std::process::exit(1);
    }
}
