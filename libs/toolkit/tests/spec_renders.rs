// SPDX-License-Identifier: MIT OR Apache-2.0
//! Renders for comparison with the chrome specification's evidence images.
//!
//! The evidence is private and never enters this tree, so nothing is
//! compared here: when `CHROME_RENDER_DIR` is set, this test renders the
//! fixture window at the evidence's size and scale (1280 x 800 pt at 2 px
//! per pt) for each chrome theme and scene, and writes
//! `<theme>-<scene>.png` into that directory for an out-of-tree comparison.
//! Unset, it does nothing. The harness keeps an 8 pt margin round the
//! window, so the images are 1296 x 816 pt and the window starts 16 px in.
//!
//! Scenes, as in the evidence: `idle`; `menu` (File open, Down twice);
//! `sub` (File open, Down four times, Right into the submenu);
//! `titlehover` (pointer on the Edit title); `closehover` and `minhover`
//! (pointer on Close and Minimize); `tooltip` (pointer rested on the search
//! button); `combo` (the workspace dropdown open); `dialog` (an Image Size
//! dialog as tall as the evidence's, its body a blank stand-in); `help`
//! (the Help menu open on its search field).

#[path = "support/fixture.rs"]
mod fixture;

use egui::Key;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use std::path::PathBuf;

/// The harness's own margin round the window, in points.
const MARGIN: f32 = 8.0;

const SCENES: [&str; 10] = [
    "idle",
    "menu",
    "sub",
    "titlehover",
    "closehover",
    "minhover",
    "tooltip",
    "combo",
    "dialog",
    "help",
];

fn keys(h: &mut Harness<'_, fixture::Fixture>, keys: &[Key]) {
    for key in keys {
        h.key_press(*key);
        h.run();
    }
}

fn hover(h: &mut Harness<'_, fixture::Fixture>, label: &str) {
    let at = h.get_by_label(label).rect().center();
    h.hover_at(at);
    h.run();
}

#[test]
fn render_chrome_scenes_for_comparison() {
    let Some(dir) = std::env::var_os("CHROME_RENDER_DIR").map(PathBuf::from) else {
        return;
    };
    std::fs::create_dir_all(&dir).expect("render directory");
    for (id, scheme, mode) in fixture::CHROME_THEMES {
        for scene in SCENES {
            let builder = Harness::builder()
                .with_size(egui::vec2(1280.0 + 2.0 * MARGIN, 800.0 + 2.0 * MARGIN))
                .with_pixels_per_point(2.0)
                .wgpu();
            let mut h = fixture::harness(builder, &fixture::theme(scheme, mode));
            match scene {
                "menu" | "sub" => {
                    h.get_by_label("File").click();
                    h.run();
                    let down = if scene == "menu" { 2 } else { 4 };
                    keys(&mut h, &[Key::ArrowDown].repeat(down));
                    if scene == "sub" {
                        keys(&mut h, &[Key::ArrowRight]);
                    }
                }
                "titlehover" => hover(&mut h, "Edit"),
                "closehover" => hover(&mut h, "Close"),
                "minhover" => hover(&mut h, "Minimize"),
                "tooltip" => {
                    hover(&mut h, "Search commands (Ctrl+K)");
                    // Rest still past the tooltip delay.
                    h.run_steps(40);
                }
                "combo" => {
                    h.get_by_role(egui::accesskit::Role::ComboBox).click();
                    h.run();
                }
                "dialog" => {
                    h.state_mut().dialog = true;
                    h.run();
                }
                "help" => {
                    h.get_by_label("Help").click();
                    h.run();
                }
                _ => {}
            }
            let image = h.render().expect("wgpu render");
            image
                .save(dir.join(format!("{id}-{scene}.png")))
                .expect("write render");
        }
    }
}
