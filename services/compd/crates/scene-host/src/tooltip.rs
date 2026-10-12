// SPDX-License-Identifier: MIT OR Apache-2.0
//! The hover tip card: one compositor-owned, click-through surface beside
//! the scene that asked for it (a panel's tips sit just above a bottom
//! panel, below a top one, beside a side one), in the session look.
//!
//! The scene's view decides when a tip shows and measures its card
//! (`crate::view::tip`); this module places, draws and hides it. The surface
//! is created once and reused: shown, moved and hidden as the pointer moves
//! across the panel, never re-created per tip.

use std::sync::Arc;

use serde_json::{Value, json};
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::utils::{Physical, Rectangle};
use ui::UiProgram;
use ui::{HandleId, UiHandle};
use world::scene::layer::base::Layer;
use world::state::Loop;
use world::surface::draw::handle::handle::{UiSpace, load};

use crate::view::tip::{self, Tip, TipAt};

/// Transparent margin around the card for its shadow (logical px).
pub const MARGIN: f32 = 12.0;
/// Space between the scene's edge and the card.
pub const GAP: f32 = 6.0;
/// The card never comes closer than this to the output's edges.
pub const EDGE: f32 = 4.0;
/// Card corner radius.
const RADIUS: f32 = 6.0;

/// Where a card goes, in output logical px: centred on its anchor along the
/// scene's long side, just outside the scene on the side facing the middle
/// of the output, clamped to the output. `scene` and the result are
/// `(x, y, w, h)`; the result is the CARD (the surface adds [`MARGIN`]).
pub fn place(scene: (f32, f32, f32, f32), at: &TipAt, output: (f32, f32)) -> (f32, f32, f32, f32) {
    let (sx, sy, sw, sh) = scene;
    let (ax, ay, aw, ah) = at.anchor;
    let (cw, ch) = at.size;
    let (ow, oh) = output;
    let (cx, cy) = (sx + ax + aw / 2.0, sy + ay + ah / 2.0);
    let clamp = |v: f32, size: f32, extent: f32| v.min(extent - size - EDGE).max(EDGE);
    if sw >= sh {
        // A horizontal scene (a top or bottom panel): above or below it.
        let y = if cy > oh / 2.0 {
            sy - GAP - ch
        } else {
            sy + sh + GAP
        };
        (clamp(cx - cw / 2.0, cw, ow), clamp(y, ch, oh), cw, ch)
    } else {
        // A vertical scene (a side panel): beside it.
        let x = if cx < ow / 2.0 {
            sx + sw + GAP
        } else {
            sx - GAP - cw
        };
        (clamp(x, cw, ow), clamp(cy - ch / 2.0, ch, oh), cw, ch)
    }
}

/// The text size a card's title and lines derive from: the look's UI size,
/// as the scene measured it.
fn ui_size(look: Option<&crate::appearance::Look>, theme: &toolkit::theme::Theme) -> f32 {
    let scale = look.map_or(1.0, |look| look.text_scale);
    let record = design::active_typography(Some(theme.typography()), design::TypographyRole::Ui);
    if record.font_size > 0.0 {
        record.font_size as f32 * scale
    } else {
        crate::view::TEXT_SIZE * scale
    }
}

#[derive(Clone, Debug)]
pub enum Message {
    Replace(Tip),
    Appearance(Arc<crate::appearance::Look>),
}

struct TooltipUi {
    tip: Tip,
    look: Option<Arc<crate::appearance::Look>>,
    installed: Option<usize>,
}

impl TooltipUi {
    fn identity(&self) -> usize {
        self.look
            .as_ref()
            .map_or(0, |look| Arc::as_ptr(look) as usize)
    }

    /// Install the look's fonts and style (an edge page's, so the card
    /// measures exactly as the panel measured it).
    fn install(&mut self, ctx: &egui::Context) {
        let scale = self.look.as_ref().map_or(1.0, |look| look.text_scale);
        crate::appearance::install(ctx, &self.theme(), scale, false);
        self.installed = Some(self.identity());
    }

    fn theme(&self) -> Arc<toolkit::theme::Theme> {
        match &self.look {
            Some(look) => look.theme.clone(),
            // The panel's own embedded look: an edge page in dark mode.
            None => Arc::new(toolkit::theme::Theme::for_context(design::DesignContext {
                mode: design::Mode::Dark,
                ..design::DesignContext::default()
            })),
        }
    }
}

impl UiProgram for TooltipUi {
    type Message = Message;

    fn setup(&mut self, ctx: &egui::Context) {
        // Before the first pass: egui binds new fonts from the NEXT pass on,
        // so a first pass that drew the bold (heading) family before this
        // install would panic ("not bound to any fonts").
        self.install(ctx);
    }

    fn show(&mut self, ui: &mut egui::Ui, _emit: &mut Vec<Message>) {
        let ctx = ui.ctx().clone();
        let theme = self.theme();
        if self.installed != Some(self.identity()) {
            // A new look: its fonts bind from the next pass. Draw nothing
            // this pass rather than measure with fonts not yet bound.
            self.install(&ctx);
            ctx.request_repaint();
            return;
        }
        let palette = crate::appearance::page(&theme, false);
        let surface = crate::view::colour(palette.base.surface);
        let ink = crate::view::colour(palette.base.foreground);
        let border = crate::view::colour(palette.border);
        let full = ui.max_rect();
        let card = full.shrink(MARGIN);
        let painter = ui.painter();
        painter.add(
            egui::epaint::Shadow {
                offset: [0, 2],
                blur: 10,
                spread: 0,
                color: egui::Color32::from_black_alpha(110),
            }
            .as_shape(card, RADIUS),
        );
        painter.rect_filled(card, RADIUS, surface);
        painter.rect_stroke(
            card,
            RADIUS,
            egui::Stroke::new(1.0, border),
            egui::StrokeKind::Inside,
        );
        let measure = |text: &str, font: &egui::FontId| {
            let size = ctx.fonts_mut(|fonts| {
                fonts
                    .layout_no_wrap(text.to_owned(), font.clone(), egui::Color32::WHITE)
                    .size()
            });
            (size.x, size.y)
        };
        let laid = tip::card(&self.tip, ui_size(self.look.as_deref(), &theme), &measure);
        for line in &laid.lines {
            painter.text(
                egui::pos2(
                    card.left() + tip::PAD_X,
                    card.top() + line.y + line.height / 2.0,
                ),
                egui::Align2::LEFT_CENTER,
                &line.text,
                crate::view::tip_font(line.size, line.bold),
                tip::line_colour(ink, line.dim),
            );
        }
    }

    fn update(&mut self, message: Message) {
        match message {
            Message::Replace(tip) => self.tip = tip,
            Message::Appearance(look) => {
                self.look = Some(look);
                self.installed = None;
            }
        }
    }
}

pub(crate) struct Surface {
    handle: HandleId,
    /// The output key the card is bound to (it follows the shown tip).
    output: String,
    tip: Option<Tip>,
    rect: Rectangle<i32, Physical>,
    factor: f32,
    visible: bool,
    appearance_generation: u64,
}

/// Hide this world's card, if it shows.
fn hide(surfaces: &mut Surfaces, world: u128, state: &mut Loop) {
    if let Some(surface) = surfaces.get_mut(&world)
        && surface.visible
        && let Some(registry) = state.inner.surface_mut().registry.as_mut()
    {
        registry.set_visible_by_id(surface.handle, false);
        surface.visible = false;
    }
}

/// The cards, one per world: a world's card lives in that world's registry,
/// so it is kept (and hidden or shown by that world's next pass) rather
/// than dropped when another world is current.
pub(crate) type Surfaces = std::collections::BTreeMap<u128, Surface>;

/// Show, move or hide the tip card. `want` is the scene asking and its tip;
/// `placed` the scenes' drawn rects (output logical px). Returns the
/// `shell.props.get` `tooltip` snapshot (`null` when hidden).
#[allow(clippy::too_many_arguments)]
pub(crate) fn reconcile(
    want: Option<&(String, TipAt)>,
    surfaces: &mut Surfaces,
    placed: &std::collections::BTreeMap<String, (f32, f32, f32, f32)>,
    prepared: Option<Arc<crate::appearance::Look>>,
    appearance_generation: u64,
    state: &mut Loop,
    renderer: &mut GlesRenderer,
    restack: bool,
) -> Value {
    let world = state.inner.worlds.spawn_target().as_u128();
    let output = state.inner.current_output();
    let factor = output.current_scale().fractional_scale().max(0.1) as f32;
    let mode = output.current_mode().map(|mode| mode.size);
    let Some(mode) = mode else {
        // No mode to place against: hide rather than leave a card up that
        // the props say is gone.
        hide(surfaces, world, state);
        return Value::Null;
    };
    let logical = (mode.w as f32 / factor, mode.h as f32 / factor);
    // This world's card; one the registry dropped is gone.
    if surfaces.get(&world).is_some_and(|surface| {
        !state
            .inner
            .surface()
            .registry
            .as_ref()
            .is_some_and(|registry| registry.contains(surface.handle))
    }) {
        surfaces.remove(&world);
    }
    let output_key = state.inner.current_output_key();
    let target = want.and_then(|(scene, at)| {
        let rect = placed.get(scene)?;
        Some((scene, at, place(*rect, at, logical)))
    });
    let Some((scene, at, (x, y, w, h))) = target else {
        hide(surfaces, world, state);
        return Value::Null;
    };
    let rect = crate::render::physical(
        (x - MARGIN, y - MARGIN, w + MARGIN * 2.0, h + MARGIN * 2.0),
        f64::from(factor),
    );
    let snapshot = json!({
        "scene": scene, "title": at.tip.title, "subtitle": at.tip.subtitle,
        "lines": at.tip.lines, "x": x, "y": y, "width": w, "height": h,
    });
    let Some(surface) = surfaces.get_mut(&world) else {
        let mut ui = TooltipUi {
            tip: at.tip.clone(),
            look: None,
            installed: None,
        };
        if let Some(look) = &prepared {
            ui.look = Some(Arc::clone(look));
        }
        let handle = load(
            state,
            renderer,
            ui,
            rect,
            UiSpace::Screen,
            Layer::SCENE.bits(),
        );
        if let Some(registry) = state.inner.surface_mut().registry.as_mut() {
            registry.request_resize_scaled_by_id(handle.id, rect.size, factor);
            registry.set_output_affinity_by_id(handle.id, Some(output_key.clone()));
            registry.set_passthrough_by_id(handle.id, true);
            registry.set_keyboard_transparent_by_id(handle.id, true);
            registry.raise(handle.id);
        }
        surfaces.insert(
            world,
            Surface {
                handle: handle.id,
                output: output_key,
                tip: Some(at.tip.clone()),
                rect,
                factor,
                visible: true,
                appearance_generation,
            },
        );
        return snapshot;
    };
    let Some(registry) = state.inner.surface_mut().registry.as_mut() else {
        return Value::Null;
    };
    if surface.rect.size != rect.size || surface.factor != factor {
        registry.request_resize_scaled_by_id(surface.handle, rect.size, factor);
    }
    if surface.rect.loc != rect.loc {
        registry.set_location_by_id(surface.handle, rect.loc);
    }
    // The tip's scene may be on another output than the card last was.
    if surface.output != output_key {
        registry.set_output_affinity_by_id(surface.handle, Some(output_key.clone()));
        surface.output = output_key;
    }
    if surface.tip.as_ref() != Some(&at.tip) {
        let _ = registry.dispatch_message(
            UiHandle::<TooltipUi>::from_id(surface.handle),
            Message::Replace(at.tip.clone()),
        );
        surface.tip = Some(at.tip.clone());
    }
    if surface.appearance_generation != appearance_generation
        && let Some(look) = &prepared
    {
        let _ = registry.dispatch_message(
            UiHandle::<TooltipUi>::from_id(surface.handle),
            Message::Appearance(Arc::clone(look)),
        );
        surface.appearance_generation = appearance_generation;
    }
    if !surface.visible {
        registry.set_visible_by_id(surface.handle, true);
        surface.visible = true;
        registry.raise(surface.handle);
    } else if restack {
        registry.raise(surface.handle);
    }
    surface.rect = rect;
    surface.factor = factor;
    snapshot
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(anchor: (f32, f32, f32, f32), size: (f32, f32)) -> TipAt {
        TipAt {
            tip: Tip {
                title: "Mail".into(),
                ..Tip::default()
            },
            anchor,
            size,
        }
    }

    /// The card's first passes draw a bold (heading-family) title. egui binds
    /// fonts from the pass AFTER an install, so the card must install before
    /// its first pass; drawing first panicked compd on VT4 (2026-10-12).
    #[test]
    fn the_card_draws_its_first_passes_and_a_new_look_without_panicking() {
        let tip = Tip {
            title: "Workspace 2".into(),
            subtitle: "Mail · 2 windows".into(),
            lines: vec!["Inbox".into(), "Drafts".into()],
        };
        for look in [None, Some(crate::appearance::fixture("dark", 1.25))] {
            let mut rt = ui::UiRuntime::headless(
                TooltipUi {
                    tip: tip.clone(),
                    look: look.clone(),
                    installed: None,
                },
                (240, 120),
                1.0,
            );
            rt.tick();
            rt.tick();
            // A look change mid-life: reinstall, skip one pass, then draw.
            rt.queue_message(Message::Appearance(crate::appearance::fixture(
                "light", 1.0,
            )));
            rt.tick();
            rt.tick();
            rt.queue_message(Message::Replace(Tip {
                title: "Thunderbird".into(),
                ..Tip::default()
            }));
            rt.tick();
        }
    }

    #[test]
    fn a_bottom_panel_tip_sits_above_it_centred_on_its_anchor() {
        // A 1536x864 output, a 52-px bottom panel; a button at x 100.
        let (x, y, w, h) = place(
            (0.0, 812.0, 1536.0, 52.0),
            &at((100.0, 6.0, 40.0, 40.0), (120.0, 40.0)),
            (1536.0, 864.0),
        );
        assert_eq!((w, h), (120.0, 40.0));
        assert_eq!(x, 120.0 - 60.0);
        assert_eq!(y, 812.0 - GAP - 40.0);
    }

    #[test]
    fn a_top_panel_tip_sits_below_it() {
        let (_, y, _, _) = place(
            (0.0, 0.0, 1536.0, 40.0),
            &at((300.0, 0.0, 40.0, 40.0), (120.0, 40.0)),
            (1536.0, 864.0),
        );
        assert_eq!(y, 40.0 + GAP);
    }

    #[test]
    fn side_panel_tips_go_beside_the_panel() {
        let left = place(
            (0.0, 0.0, 52.0, 864.0),
            &at((6.0, 400.0, 40.0, 40.0), (120.0, 40.0)),
            (1536.0, 864.0),
        );
        assert_eq!(left.0, 52.0 + GAP);
        assert_eq!(left.1, 420.0 - 20.0);
        let right = place(
            (1484.0, 0.0, 52.0, 864.0),
            &at((6.0, 400.0, 40.0, 40.0), (120.0, 40.0)),
            (1536.0, 864.0),
        );
        assert_eq!(right.0, 1484.0 - GAP - 120.0);
    }

    #[test]
    fn cards_clamp_inside_the_output() {
        // A button at the far left and one at the far right.
        let (x, _, _, _) = place(
            (0.0, 812.0, 1536.0, 52.0),
            &at((0.0, 6.0, 40.0, 40.0), (300.0, 40.0)),
            (1536.0, 864.0),
        );
        assert_eq!(x, EDGE);
        let (x, _, _, _) = place(
            (0.0, 812.0, 1536.0, 52.0),
            &at((1500.0, 6.0, 36.0, 40.0), (300.0, 40.0)),
            (1536.0, 864.0),
        );
        assert_eq!(x, 1536.0 - 300.0 - EDGE);
    }
}
