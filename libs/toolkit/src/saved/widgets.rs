// SPDX-License-Identifier: MIT OR Apache-2.0
//! Thin widget adapters. Stable caller names are the on-disk keys; egui IDs
//! remain implementation details. Runtime restore flags are never serialised.
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Widgets {
    pub panels: BTreeMap<String, f32>,
    pub scrolls: BTreeMap<String, Scroll>,
    pub open: BTreeMap<String, bool>,
    pub groups: BTreeMap<String, crate::panel::GroupState>,
    #[serde(skip)]
    applied: BTreeSet<String>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Scroll {
    pub key: Option<String>,
    pub offset: [f32; 2],
}

impl Widgets {
    /// Install an isolated per-window collection in the context before drawing.
    pub fn install(&self, ctx: &egui::Context) {
        ctx.data_mut(|d| {
            d.insert_temp(
                egui::Id::new("toolkit.saved.widgets"),
                Arc::new(Mutex::new(self.clone())),
            )
        });
    }
    pub fn collect(&mut self, ctx: &egui::Context) {
        if let Some(shared) = shared(ctx) {
            *self = shared.lock().unwrap_or_else(|e| e.into_inner()).clone();
        }
    }
}
fn shared(ctx: &egui::Context) -> Option<Arc<Mutex<Widgets>>> {
    ctx.data(|d| d.get_temp(egui::Id::new("toolkit.saved.widgets")))
}
fn with<R>(ctx: &egui::Context, f: impl FnOnce(&mut Widgets) -> R) -> Option<R> {
    let shared = shared(ctx)?;
    Some(f(&mut shared.lock().unwrap_or_else(|e| e.into_inner())))
}

/// A panel's size share. `share` is also the app's Bus-controlled value. Only
/// dragging records a changed size, so content minimums cannot feed back.
/// Construct the egui Panel with the same `name` as its ID. The resize handle
/// ID below follows pinned egui 0.36; the drag regression test guards it.
#[expect(
    clippy::too_many_arguments,
    reason = "thin adapter retains the panel builder and size policy"
)]
pub fn panel<R>(
    ui: &mut egui::Ui,
    name: &str,
    panel: egui::Panel,
    extent: f32,
    share: f32,
    limits: std::ops::RangeInclusive<f32>,
    horizontal: bool,
    body: impl FnOnce(&mut egui::Ui) -> R,
) -> (egui::InnerResponse<R>, Option<f32>) {
    let (share, restored) = with(ui.ctx(), |w| {
        if w.applied.insert(format!("panel:{name}")) {
            let saved = panel_share(w, name, share);
            (saved, ((saved - share).abs() > 0.005).then_some(saved))
        } else {
            (share, None)
        }
    })
    .unwrap_or((share, None));
    // egui remembers pixels. Invalidate them only when the available extent
    // changes, retaining the saved share when a window or adjacent dock resizes.
    ui.ctx().data_mut(|d| {
        let key = egui::Id::new(("saved-panel-extent", name));
        if d.get_temp::<f32>(key)
            .is_some_and(|old| (old - extent).abs() > 0.5)
        {
            d.remove::<egui::containers::panel::PanelState>(egui::Id::new(name));
        }
        d.insert_temp(key, extent);
    });
    let shown = panel
        .resizable(true)
        .default_size(extent * share.clamp(*limits.start(), *limits.end()))
        .size_range(extent * limits.start()..=extent * limits.end())
        .show(ui, |ui| {
            if horizontal {
                ui.set_min_width(ui.available_width());
            } else {
                ui.set_min_height(ui.available_height());
            }
            body(ui)
        });
    let size = if horizontal {
        shown.response.rect.width()
    } else {
        shown.response.rect.height()
    };
    let changed = ui
        .read_response(egui::Id::new(name).with("__resize"))
        .is_some_and(|r| r.dragged() || r.drag_stopped())
        .then_some((size / extent.max(1.0)).clamp(*limits.start(), *limits.end()))
        .filter(|v| (*v - share).abs() > 0.005)
        .or(restored);
    with(ui.ctx(), |w| {
        w.panels.insert(name.into(), changed.unwrap_or(share));
    });
    (shown, changed)
}

/// Set a share from a Bus command and invalidate egui's remembered pixels.
pub fn set_panel(ctx: &egui::Context, name: &str, share: f32) {
    with(ctx, |w| {
        w.panels.insert(name.into(), share);
        // A Bus choice made before the first layout must win over loading
        // the previous session's share on that first layout.
        w.applied.insert(format!("panel:{name}"));
    });
    ctx.data_mut(|d| d.remove::<egui::containers::panel::PanelState>(egui::Id::new(name)));
}

/// A restored selection's visibility takes priority over the old raw offset.
pub fn forget_scroll(ctx: &egui::Context, name: &str) {
    with(ctx, |w| {
        w.scrolls.remove(name);
    });
}

/// Read a saved panel share on startup; sanitise before handing it to egui.
pub fn panel_share(widgets: &Widgets, name: &str, fallback: f32) -> f32 {
    widgets
        .panels
        .get(name)
        .copied()
        .filter(|v| v.is_finite())
        .unwrap_or(fallback)
        .clamp(0.01, 0.99)
}

/// Restore once for each content identity after a first layout provides bounds.
/// `ready` lets live trees delay restoration until their rows have appeared.
pub fn scroll<R>(
    ui: &mut egui::Ui,
    name: &str,
    key: Option<&str>,
    ready: bool,
    area: egui::ScrollArea,
    body: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::scroll_area::ScrollAreaOutput<R> {
    let token = format!("scroll:{name}:{key:?}");
    let prior = with(ui.ctx(), |w| {
        if ready && !w.applied.contains(&token) {
            w.scrolls
                .get(name)
                .filter(|s| s.key.as_deref() == key)
                .cloned()
        } else {
            None
        }
    })
    .flatten();
    // First show lays out content. Setting egui's state now applies next frame,
    // with bounds known, rather than clamping against an empty first layout.
    let out = area.id_salt(("saved-scroll", name, key)).show(ui, body);
    let max = (out.content_size - out.inner_rect.size()).max(egui::Vec2::ZERO);
    let restore = prior.map(|s| egui::vec2(s.offset[0], s.offset[1]).clamp(egui::Vec2::ZERO, max));
    if let Some(offset) = restore {
        let mut state = out.state;
        state.offset = offset;
        state.store(ui.ctx(), out.id);
        ui.ctx().request_repaint();
    }
    with(ui.ctx(), |w| {
        if ready {
            w.applied.insert(token);
            let offset = restore.unwrap_or(out.state.offset);
            w.scrolls.insert(
                name.into(),
                Scroll {
                    key: key.map(str::to_owned),
                    offset: [offset.x, offset.y],
                },
            );
        }
    });
    out
}

/// A collapsing state whose openness belongs to a stable app name. Run the
/// native consuming `state.show_header(ui, header).body(body)` inside `show`;
/// the helper captures the state egui stored, without changing its response.
pub fn collapsing<R>(
    ctx: &egui::Context,
    name: &str,
    default: bool,
    show: impl FnOnce(egui::collapsing_header::CollapsingState) -> R,
) -> R {
    let open = with(ctx, |w| w.open.get(name).copied().unwrap_or(default)).unwrap_or(default);
    let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(
        ctx,
        egui::Id::new(("saved-open", name)),
        open,
    );
    state.set_open(open);
    let id = state.id();
    state.store(ctx);
    let out = show(state);
    let state = egui::collapsing_header::CollapsingState::load(ctx, id)
        .expect("the helper stored its initial state");
    with(ctx, |w| {
        w.open.insert(name.into(), state.is_open());
    });
    out
}

/// [`crate::panel::group`] with saved selected tab and collapsed state.
pub fn group<R>(
    ui: &mut egui::Ui,
    name: &str,
    tabs: &[&str],
    body: impl FnOnce(&mut egui::Ui, usize) -> R,
) -> crate::panel::GroupResponse<R> {
    let id = ui.make_persistent_id(name);
    let state = with(ui.ctx(), |w| w.groups.get(name).copied()).flatten();
    if let Some(state) = state {
        ui.data_mut(|d| d.insert_temp(id, state));
    }
    let out = crate::panel::group(ui, name, tabs, body);
    with(ui.ctx(), |w| {
        w.groups.insert(name.into(), out.state);
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn saved_shares_and_scroll_bounds_compose_with_text_scale_and_density() {
        for density in [0.85, 1.0, 1.2] {
            let ctx = egui::Context::default();
            let theme = crate::Theme::embedded().with_ui(crate::ui_scale::CommonUi {
                text_scale: 1.5, density, ..Default::default()
            });
            crate::install(&ctx, &theme);
            let mut widgets = Widgets::default();
            widgets.panels.insert("left".into(), 0.4);
            widgets.scrolls.insert("text".into(), Scroll { key: None, offset: [0.0, 100_000.0] });
            widgets.install(&ctx);
            let mut max_y = 0.0;
            draw(&ctx, |ui| {
                panel(ui, "left", egui::Panel::left("left"), 1000.0, 0.4, 0.1..=0.8, true, |ui| {
                    let out = scroll(ui, "text", None, true, egui::ScrollArea::vertical().max_height(100.0), |ui| {
                        for _ in 0..50 { ui.label("Larger text increases the laid out content height"); }
                    });
                    max_y = (out.content_size.y - out.inner_rect.height()).max(0.0);
                });
            });
            widgets.collect(&ctx);
            assert_eq!(widgets.panels["left"], 0.4);
            assert!(max_y > 100.0);
            assert!((widgets.scrolls["text"].offset[1] - max_y).abs() < 0.1);
        }
    }
    fn draw(ctx: &egui::Context, body: impl FnMut(&mut egui::Ui)) {
        let mut out = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1000.0, 600.0),
                )),
                ..Default::default()
            },
            body,
        );
        out.textures_delta.clear();
    }
    #[test]
    fn panel_share_restores_without_layout_feedback() {
        let ctx = egui::Context::default();
        let mut widgets = Widgets::default();
        widgets.panels.insert("left".into(), 0.4);
        widgets.install(&ctx);
        let mut share = 0.2;
        for _ in 0..2 {
            draw(&ctx, |ui| {
                let (out, changed) = panel(
                    ui,
                    "left",
                    egui::Panel::left("left"),
                    1000.0,
                    share,
                    0.1..=0.8,
                    true,
                    |_| {},
                );
                assert!((out.response.rect.width() - 400.0).abs() < 2.0);
                if let Some(restored) = changed {
                    share = restored;
                }
            });
        }
        assert_eq!(share, 0.4);
        widgets.collect(&ctx);
        assert_eq!(widgets.panels["left"], 0.4);
    }
    #[test]
    fn panel_drag_records_the_share_and_unrelated_clicks_do_not() {
        let mut h = egui_kittest::Harness::builder()
            .with_size(egui::vec2(1000.0, 600.0))
            .build_ui_state(
                |ui, share: &mut f32| {
                    let (_, changed) = panel(
                        ui,
                        "left",
                        egui::Panel::left("left"),
                        1000.0,
                        *share,
                        0.1..=0.8,
                        true,
                        |ui| {
                            ui.label("content");
                        },
                    );
                    if let Some(value) = changed {
                        *share = value;
                    }
                },
                0.3,
            );
        h.run();
        let edge = egui::containers::panel::PanelState::load(&h.ctx, egui::Id::new("left"))
            .unwrap()
            .outer_rect
            .right();
        h.hover_at(egui::pos2(edge, 250.0));
        h.run();
        h.drag_at(egui::pos2(edge, 250.0));
        h.run();
        h.hover_at(egui::pos2(edge + 100.0, 250.0));
        h.run();
        h.drop_at(egui::pos2(edge + 100.0, 250.0));
        h.run();
        assert!((*h.state() - 0.4).abs() < 0.02, "{}", h.state());
        let share = *h.state();
        h.hover_at(egui::pos2(50.0, 50.0));
        h.run();
        h.drag_at(egui::pos2(50.0, 50.0));
        h.run();
        h.drop_at(egui::pos2(50.0, 50.0));
        h.run();
        assert_eq!(*h.state(), share);
    }
    #[test]
    fn a_panel_command_before_first_layout_replaces_the_saved_share() {
        let ctx = egui::Context::default();
        let mut widgets = Widgets::default();
        widgets.panels.insert("left".into(), 0.4);
        widgets.install(&ctx);
        set_panel(&ctx, "left", 0.6);
        draw(&ctx, |ui| {
            let (out, changed) = panel(
                ui,
                "left",
                egui::Panel::left("left"),
                1000.0,
                0.6,
                0.1..=0.8,
                true,
                |_| {},
            );
            assert!((out.response.rect.width() - 600.0).abs() < 2.0);
            assert!(changed.is_none());
        });
        widgets.collect(&ctx);
        assert_eq!(widgets.panels["left"], 0.6);
    }
    #[test]
    fn scroll_applies_once_after_layout_clamped_and_only_for_its_key() {
        let ctx = egui::Context::default();
        let mut widgets = Widgets::default();
        widgets.scrolls.insert(
            "details".into(),
            Scroll {
                key: Some("one".into()),
                offset: [0.0, 10000.0],
            },
        );
        widgets.install(&ctx);
        draw(&ctx, |ui| {
            scroll(
                ui,
                "details",
                Some("one"),
                true,
                egui::ScrollArea::vertical().max_height(100.0),
                |ui| {
                    ui.allocate_space(egui::vec2(100.0, 1000.0));
                },
            );
        });
        widgets.collect(&ctx);
        let offset = widgets.scrolls["details"].offset[1];
        assert!(
            offset > 800.0 && offset < 1000.0,
            "clamped to actual content: {offset}"
        );
        with(&ctx, |w| {
            w.scrolls.get_mut("details").unwrap().offset[1] = 12.0;
        });
        draw(&ctx, |ui| {
            let out = scroll(
                ui,
                "details",
                Some("one"),
                true,
                egui::ScrollArea::vertical().max_height(100.0),
                |ui| {
                    ui.allocate_space(egui::vec2(100.0, 1000.0));
                },
            );
            assert!(out.state.offset.y > 800.0);
        });
        draw(&ctx, |ui| {
            let out = scroll(
                ui,
                "details",
                Some("two"),
                true,
                egui::ScrollArea::vertical().max_height(100.0),
                |ui| {
                    ui.allocate_space(egui::vec2(100.0, 1000.0));
                },
            );
            assert_eq!(out.state.offset.y, 0.0);
        });
    }
    #[test]
    fn group_and_collapsing_state_survive_a_new_context() {
        let ctx = egui::Context::default();
        let mut widgets = Widgets::default();
        widgets.groups.insert(
            "group".into(),
            crate::panel::GroupState {
                selected: 1,
                collapsed: true,
            },
        );
        widgets.open.insert("branch".into(), true);
        widgets.install(&ctx);
        draw(&ctx, |ui| {
            let shown = group(ui, "group", &["one", "two"], |_, _| panic!("collapsed"));
            assert_eq!(shown.state.selected, 1);
        });
        collapsing(&ctx, "branch", false, |s| {
            assert!(s.is_open());
            s
        });
        widgets.collect(&ctx);
        let raw = strict::to_string_pretty(&widgets).unwrap();
        let restored: Widgets = strict::from_str(&raw).unwrap();
        assert_eq!(restored.groups["group"].selected, 1);
        assert!(restored.open["branch"]);
        assert!(restored.applied.is_empty());
    }
    #[test]
    fn collapsing_accepts_the_native_consuming_header_and_body() {
        let ctx = egui::Context::default();
        let mut widgets = Widgets::default();
        widgets.install(&ctx);
        draw(&ctx, |ui| {
            collapsing(&ctx, "branch", true, |mut state| {
                state.set_open(false);
                state
                    .show_header(ui, |ui| ui.label("Branch"))
                    .body(|ui| ui.label("Contents"))
            });
        });
        widgets.collect(&ctx);
        assert!(!widgets.open["branch"]);
        let next = egui::Context::default();
        widgets.install(&next);
        collapsing(&next, "branch", true, |s| assert!(!s.is_open()));
    }
}
