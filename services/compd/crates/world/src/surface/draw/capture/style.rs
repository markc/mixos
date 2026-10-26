// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The shared look of the capture overlays.
//!
//! Panels, buttons and text are the MixOS theme's (`toolkit::style`), so the
//! overlays match the apps in every scheme: the accent is the theme's
//! selection colour, a destructive action its error colour. Only the masks
//! carry their own colour: they are translucency over the screen, not themed
//! surfaces.
//!
//! The theme is the session's, as the shell follows it ([`follow`], fed from
//! scene-host's look-change handler like the region-select overlay), and the
//! embedded theme until the shell has delivered one. Every capture UI
//! installs it before its first pass ([`install`]) and catches up with a
//! change at the start of each pass ([`keep_up`]); [`restyle`] wakes the
//! open ones so a change shows without waiting for input.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use egui::{Align2, Color32, Id, Response, RichText, Stroke, Ui};
use toolkit::theme::Theme;

use crate::state::Loop;

/// The session theme the shell last followed; `None` until it delivers one.
static SESSION: Mutex<Option<Arc<Theme>>> = Mutex::new(None);
/// Bumped on every [`follow`]: what a context has installed is compared
/// against it.
static GENERATION: AtomicU64 = AtomicU64::new(0);

/// Follow the session theme (the shell's look, as settingsd accepted it):
/// called on every look change. Open capture UIs take it on their next pass
/// ([`restyle`] asks for one).
pub fn follow(theme: Arc<Theme>) {
    *SESSION.lock().unwrap_or_else(|e| e.into_inner()) = Some(theme);
    GENERATION.fetch_add(1, Ordering::AcqRel);
}

/// The theme capture UIs draw with: the session's, else the embedded one.
pub fn theme() -> Arc<Theme> {
    static EMBEDDED: OnceLock<Arc<Theme>> = OnceLock::new();
    SESSION
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
        .unwrap_or_else(|| EMBEDDED.get_or_init(|| Arc::new(Theme::embedded())).clone())
}

/// Where a context records the generation it installed.
fn installed_id() -> Id {
    Id::unique("capture-style-generation")
}

/// Install the current theme on `ctx` (style, chrome, icons and fonts) and
/// record which one it is. A capture UI's `setup`, before its first pass.
pub fn install(ctx: &egui::Context) {
    let generation = GENERATION.load(Ordering::Acquire);
    toolkit::style::apply(ctx, &theme());
    ctx.data_mut(|data| data.insert_temp(installed_id(), generation));
}

/// At the start of a pass: if the session theme changed since `ctx`
/// installed one, install it and ask for another pass (egui applies a new
/// style and fonts from the next pass on). Returns whether it did.
pub fn keep_up(ctx: &egui::Context) -> bool {
    let generation = GENERATION.load(Ordering::Acquire);
    let installed = ctx.data(|data| data.get_temp::<u64>(installed_id()));
    if installed == Some(generation) {
        return false;
    }
    install(ctx);
    ctx.request_repaint();
    true
}

/// Ask every open capture UI for a pass, in whichever world's registry holds
/// it (as teardown sweeps them), so a theme change reaches a panel nobody is
/// touching.
pub fn restyle(lp: &mut Loop) {
    let ids = lp
        .inner
        .kernel
        .get(&crate::driver::capture::base::CAPTURE)
        .overlay_ids();
    if ids.is_empty() {
        return;
    }
    for world in lp.inner.worlds.ids() {
        let Some(surface) = lp
            .inner
            .worlds
            .get_mut(world)
            .storage_mut()
            .try_get_mut(&crate::surface::system::base::SURFACE_MUT)
        else {
            continue;
        };
        let Some(registry) = surface.registry.as_mut() else {
            continue;
        };
        for id in &ids {
            if let Some(item) = registry.get_mut(*id) {
                item.request_redraw();
            }
        }
    }
    lp.state
        .schedule_redraw(dispatcher::state::RedrawReason::Capture);
}

/// The setup overlay's mask outside the hole: dim, not opaque, so the user can
/// still aim the region at what is underneath.
pub const SETUP_MASK: Color32 = Color32::from_black_alpha(128);
/// The backdrop drawn below windows while capturing, outside the region.
pub const DIM_MASK: Color32 = Color32::from_black_alpha(115);

pub const BORDER_WIDTH: f32 = 2.0;

/// The theme's accent: the region outline, selected choices, the default
/// action.
pub fn accent(ui: &Ui) -> Color32 {
    ui.visuals().selection.bg_fill
}

/// The theme's colour for a destructive action.
pub fn danger(ui: &Ui) -> Color32 {
    ui.visuals().error_fg_color
}

/// Dimmed secondary text.
pub fn weak(ui: &Ui) -> Color32 {
    ui.visuals().weak_text_color()
}

/// A dialog-sized button. `fill` picks a filled style (accent or danger);
/// `None` is the theme's plain button.
pub fn button(ui: &mut Ui, label: String, fill: Option<Color32>) -> Response {
    let mut text = RichText::new(label).size(15.0);
    let mut button = match fill {
        Some(fill) => {
            text = text.color(contrast(fill));
            egui::Button::new(text).fill(fill)
        }
        None => egui::Button::new(text),
    };
    button = button.min_size(egui::vec2(0.0, 32.0));
    ui.add(button)
}

/// A choice in a row of choices: filled with the accent when selected.
pub fn choice(ui: &mut Ui, label: String, selected: bool) -> Response {
    let fill = selected.then(|| accent(ui));
    button(ui, label, fill)
}

/// Black or white, whichever reads on `fill`.
fn contrast(fill: Color32) -> Color32 {
    let [r, g, b, _] = fill.to_array();
    let luma = 0.299 * f32::from(r) + 0.587 * f32::from(g) + 0.114 * f32::from(b);
    if luma > 150.0 {
        Color32::BLACK
    } else {
        Color32::WHITE
    }
}

/// A dialog panel centred on the surface, outlined in `border`.
pub fn centred_dialog(ui: &mut Ui, id: &str, border: Color32, body: impl FnOnce(&mut Ui)) {
    egui::Area::new(Id::unique(id))
        .anchor(Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style())
                .inner_margin(24.0)
                .stroke(Stroke::new(1.0, border))
                .show(ui, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.spacing_mut().item_spacing.y = 14.0;
                        body(ui);
                    });
                });
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The panel fill a context draws with.
    fn fill(ctx: &egui::Context) -> Color32 {
        ctx.global_style().visuals.window_fill
    }

    #[test]
    fn the_panels_follow_the_session_theme_with_the_embedded_one_first() {
        // Before the shell delivers a look: the embedded theme.
        let embedded = Theme::embedded();
        let embedded_fill = toolkit::style::visuals(&embedded).window_fill;
        let ctx = egui::Context::default();
        install(&ctx);
        assert_eq!(fill(&ctx), embedded_fill);
        assert!(!keep_up(&ctx), "nothing changed");
        // A look change reaches a context on its next pass, once.
        let light = Arc::new(Theme::from_source("test", "mode: \"light\"\n").unwrap());
        let light_fill = toolkit::style::visuals(&light).window_fill;
        assert_ne!(light_fill, embedded_fill, "light mode changes the panels");
        follow(light.clone());
        assert!(Arc::ptr_eq(&theme(), &light));
        assert!(keep_up(&ctx), "the new look is installed");
        assert_eq!(fill(&ctx), light_fill);
        assert!(!keep_up(&ctx), "and only once");
        // A UI set up after the change starts in it.
        let later = egui::Context::default();
        install(&later);
        assert_eq!(fill(&later), light_fill);
        assert!(!keep_up(&later));
    }
}
