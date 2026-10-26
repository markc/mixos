// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Draw and hit-test a laid-out scene: each node at exactly the rectangle
//! [`super::flex`] gave it, parents before children (so a control inside a
//! clickable row takes its own press), rows of a scrolling list offset by
//! its scroll and clipped to its box.

use std::sync::Arc;

use design::InteractionState;
use egui::{
    Align2, Color32, FontFamily, FontId, Id, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2,
};
use scene::Node;

use super::flex::{Placed, Source};
use super::{
    Route, SceneMessage, SceneUi, TITLEBAR, button_key, colour, field_id, flag, hex, linear, number,
};
use crate::templates::text as port_text;

/// The font a scene text draws in: Inter (SemiBold when bold), or the mono
/// face.
pub(crate) fn font(size: f32, mono: bool, bold: bool) -> FontId {
    let family = if mono {
        FontFamily::Monospace
    } else if bold {
        FontFamily::Name(toolkit::fonts::HEADING.into())
    } else {
        FontFamily::Proportional
    };
    FontId::new(size, family)
}

/// The node a placement draws, from the document or its row instance.
fn node_of<'a>(scene: &'a SceneUi, placed: &Placed) -> Option<&'a Node> {
    match &placed.source {
        Source::Tree => scene.content.tree.nodes.get(&placed.id),
        Source::Row { list, item } => {
            if placed.cell {
                scene.content.tree.nodes.get(list)
            } else {
                scene
                    .content
                    .lists
                    .get(list)?
                    .instances
                    .get(item)?
                    .get(&placed.id)
            }
        }
    }
}

/// The route an interaction on `placed` takes: its own node's in the
/// document, the list's (with the row item) inside a row.
fn route_of(scene: &SceneUi, placed: &Placed, node: &Node) -> Arc<Route> {
    let tree = &scene.content.tree;
    match &placed.source {
        Source::Tree => Arc::new(Route::new(tree, &placed.id, node)),
        Source::Row { list, item } => {
            let list_node = tree.nodes.get(list).unwrap_or(node);
            let mut route = Route::new(tree, list, list_node);
            route.item = crate::templates::rows(list_node)
                .iter()
                .find(|row| row["id"].as_str() == Some(item.as_str()))
                .cloned();
            Arc::new(route)
        }
    }
}

/// Keep the start of a line and end it with an ellipsis within its layout box.
pub(super) fn elide_end(ui: &Ui, text: &str, font: &FontId, width: f32) -> String {
    let fits = |s: &str| {
        ui.fonts_mut(|fonts| {
            fonts
                .layout_no_wrap(s.to_owned(), font.clone(), Color32::WHITE)
                .size()
                .x
        }) <= width
    };
    if fits(text) {
        return text.to_owned();
    }
    let chars: Vec<char> = text.chars().collect();
    let mut keep = chars.len();
    while keep > 0 {
        keep -= 1;
        let candidate: String = chars[..keep].iter().chain(std::iter::once(&'…')).collect();
        if fits(&candidate) {
            return candidate;
        }
    }
    if fits("…") {
        "…".into()
    } else {
        String::new()
    }
}

/// Paint the whole scene and collect its interactions.
pub(crate) fn paint(scene: &mut SceneUi, ui: &mut Ui, emit: &mut Vec<SceneMessage>) {
    let full = ui.max_rect();
    let palette = scene.palette();
    let page = colour(palette.base.surface);
    let ink = colour(palette.base.foreground);
    let framed = scene.content.frame.is_some();
    ui.painter().rect_filled(full, 0.0, page);
    if let Some(frame) = scene.content.frame.clone() {
        paint_frame(scene, ui, full, &frame.title, emit);
    }
    // Scroll the lists under the pointer, and keep every offset inside its
    // list's current content (rows removed, box grown) or drop it with the
    // list.
    let pointer = ui.ctx().input(|input| input.pointer.hover_pos());
    let wheel = ui.ctx().input(|input| input.smooth_scroll_delta.y);
    let lists = &scene.layout.lists;
    scene.scroll.retain(|id, _| lists.contains_key(id));
    for (id, list) in lists {
        let most = (list.content_height - list.rect.height()).max(0.0);
        if wheel != 0.0 && pointer.is_some_and(|at| list.rect.contains(at)) {
            let offset = scene.scroll.entry(id.clone()).or_insert(0.0);
            *offset -= wheel;
        }
        if let Some(offset) = scene.scroll.get_mut(id) {
            *offset = offset.clamp(0.0, most);
        }
    }
    let placed = scene.layout.placed.clone();
    let styles = scene.text_styles();
    // The textures this pass draws; the rest are dropped after it (egui
    // frees them, and the painter retires them a pass later).
    let mut drawn = std::collections::BTreeSet::new();
    // The innermost hovered node that has a tip (children paint after
    // their parents, so a later hit replaces an earlier one).
    let mut tip_candidate: Option<super::tip::Candidate> = None;
    for item in &placed {
        let Some(node) = node_of(scene, item) else {
            continue;
        };
        let node = node.clone();
        // A row of a scrolling list moves with its scroll and stays in its box.
        let (rect, clip) = match item
            .in_list
            .as_ref()
            .and_then(|list| scene.layout.lists.get(list).map(|b| (list, b)))
        {
            Some((list, boxed)) => {
                let offset = scene.scrolled(list);
                (item.rect.translate(vec2(0.0, -offset)), boxed.rect)
            }
            None => (item.rect, full),
        };
        let visible = rect.intersect(clip);
        if !visible.is_positive() && !(rect.width() == 0.0 || rect.height() == 0.0) {
            continue;
        }
        let painter = ui.painter().with_clip_rect(clip);
        let id = Id::unique(("scene-node", &item.key));
        if item.cell {
            // The whole row is the list's click target (Quoin's ClickRow).
            let response = ui.interact(visible, id, Sense::click());
            if response.clicked() {
                emit.push(SceneMessage::Click(route_of(scene, item, &node)));
            }
            if response.secondary_clicked() {
                let route = route_of(scene, item, &node);
                if route.context.is_some() {
                    emit.push(SceneMessage::Context(route));
                }
            }
            continue;
        }
        match node.family.as_str() {
            "column" | "row" => {
                let normal = hex(port_text(&node, "background"));
                let hover = hex(port_text(&node, "hover")).or(normal);
                let radius = number(&node, "radius").unwrap_or(0.0);
                let clickable = node.ports.contains_key("on_click")
                    || node.ports.contains_key("on_context")
                    || item.source != Source::Tree;
                let tip = node
                    .ports
                    .get("tooltip")
                    .and_then(super::tip::Tip::from_port);
                let hovered = if clickable || hover != normal || tip.is_some() {
                    let response = ui.interact(
                        visible,
                        id,
                        if clickable {
                            Sense::click()
                        } else {
                            Sense::hover()
                        },
                    );
                    if clickable && response.clicked() {
                        emit.push(SceneMessage::Click(route_of(scene, item, &node)));
                    }
                    // In a list row the route (and its on_context) is the list's.
                    if clickable && response.secondary_clicked() {
                        let route = route_of(scene, item, &node);
                        if route.context.is_some() {
                            emit.push(SceneMessage::Context(route));
                        }
                    }
                    response.hovered()
                } else {
                    false
                };
                if hovered && let Some(tip) = tip {
                    tip_candidate = Some(super::tip::Candidate {
                        key: item.key.clone(),
                        rect: visible,
                        tip,
                    });
                }
                if let Some(fill) = if hovered { hover } else { normal } {
                    painter.rect_filled(rect, radius, fill);
                }
            }
            "text" => {
                let mono = flag(&node, "mono");
                let style = if mono { styles.mono } else { styles.ui };
                let size = number(&node, "size").unwrap_or(style.size);
                let font = font(size, mono, flag(&node, "bold"));
                let colour = hex(port_text(&node, "color")).unwrap_or(ink);
                let mut text = port_text(&node, "text").to_owned();
                if flag(&node, "wrap") {
                    let lines = super::flex::wrap_text(
                        &text,
                        rect.width(),
                        number(&node, "max_lines").unwrap_or(3.0) as usize,
                        &|s| {
                            ui.fonts_mut(|fonts| {
                                fonts
                                    .layout_no_wrap(s.to_owned(), font.clone(), colour)
                                    .size()
                                    .x
                            })
                        },
                    );
                    let line_height = styles.line(mono, size);
                    for (i, line) in lines.iter().enumerate() {
                        let (align, x) = match port_text(&node, "align") {
                            "center" => (Align2::CENTER_CENTER, rect.center().x),
                            "right" => (Align2::RIGHT_CENTER, rect.right()),
                            _ => (Align2::LEFT_CENTER, rect.left()),
                        };
                        painter.text(
                            pos2(x, rect.top() + (i as f32 + 0.5) * line_height),
                            align,
                            line,
                            font.clone(),
                            colour,
                        );
                    }
                    continue;
                }
                if flag(&node, "elide") {
                    text = elide_end(ui, &text, &font, rect.width());
                }
                let (align, x) = match port_text(&node, "align") {
                    "center" => (Align2::CENTER_CENTER, rect.center().x),
                    "right" => (Align2::RIGHT_CENTER, rect.right()),
                    _ => (Align2::LEFT_CENTER, rect.left()),
                };
                painter.text(pos2(x, rect.center().y), align, text, font, colour);
            }
            "button" => {
                let response = ui.interact(visible, id, Sense::click());
                let interaction = if response.is_pointer_button_down_on() {
                    InteractionState::Pressed
                } else if response.hovered() {
                    InteractionState::Hovered
                } else {
                    InteractionState::Resting
                };
                if response.hovered()
                    && let Some(tip) = node
                        .ports
                        .get("tooltip")
                        .and_then(super::tip::Tip::from_port)
                {
                    tip_candidate = Some(super::tip::Candidate {
                        key: item.key.clone(),
                        rect: visible,
                        tip,
                    });
                }
                let theme = scene.theme();
                let tables = theme.design().tables();
                let cell = tables.button.cell(button_key(&node, interaction));
                let radius = cell.radius as f32;
                painter.rect_filled(rect, radius, linear(cell.pair.surface));
                if let Some(border) = cell.border {
                    painter.rect_stroke(
                        rect,
                        radius,
                        Stroke::new(cell.border_width as f32, linear(border)),
                        StrokeKind::Inside,
                    );
                }
                painter.text(
                    rect.center(),
                    Align2::CENTER_CENTER,
                    port_text(&node, "label"),
                    font(styles.ui.size, false, false),
                    linear(cell.pair.foreground),
                );
                if response.clicked() {
                    emit.push(SceneMessage::Click(route_of(scene, item, &node)));
                }
                // A right click: the button's own on_context (or, inside a
                // list row, the list's).
                if response.secondary_clicked() {
                    let route = route_of(scene, item, &node);
                    if route.context.is_some() {
                        emit.push(SceneMessage::Context(route));
                    }
                }
            }
            "field" => {
                let key = item.key.clone();
                let value = scene
                    .edits
                    .get(&key)
                    .cloned()
                    .unwrap_or_else(|| port_text(&node, "value").to_owned());
                let mut text = value.clone();
                let field = if item.source == Source::Tree {
                    field_id(&item.id)
                } else {
                    Id::unique(("scene-field-row", &item.key))
                };
                let edit = egui::TextEdit::singleline(&mut text)
                    .id(field)
                    .hint_text(port_text(&node, "placeholder"))
                    .password(flag(&node, "password"))
                    .font(font(styles.ui.size, false, false))
                    .margin(egui::Margin::symmetric(7, 4));
                // In a child clipped like the other nodes, so a field half
                // scrolled out of its list neither paints nor takes input
                // outside the list's box.
                let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
                child.set_clip_rect(clip.intersect(ui.clip_rect()));
                let response = child.put(rect, edit);
                let route = route_of(scene, item, &node);
                if response.changed() {
                    emit.push(SceneMessage::Input(Arc::clone(&route), key, text.clone()));
                }
                if response.lost_focus()
                    && ui.ctx().input(|input| input.key_pressed(egui::Key::Enter))
                {
                    emit.push(SceneMessage::Submit(route, text));
                }
            }
            "toggle" => {
                let key = item.key.clone();
                let value = scene
                    .toggles
                    .get(&key)
                    .copied()
                    .unwrap_or(flag(&node, "value"));
                let response = ui.interact(visible, id, Sense::click());
                let h = styles.ui.line(styles.ui.size).min(rect.height());
                let switch = Rect::from_min_size(
                    pos2(rect.left(), rect.center().y - h / 2.0),
                    vec2(h * 1.8, h),
                );
                let visuals = ui.visuals();
                let track = if value {
                    visuals.selection.bg_fill
                } else {
                    visuals.widgets.inactive.bg_fill
                };
                painter.rect_filled(switch, h / 2.0, track);
                let knob_x = if value {
                    switch.right() - h / 2.0
                } else {
                    switch.left() + h / 2.0
                };
                painter.circle_filled(
                    pos2(knob_x, switch.center().y),
                    h / 2.0 - 2.0,
                    visuals.widgets.active.fg_stroke.color,
                );
                let label = port_text(&node, "label");
                if !label.is_empty() {
                    painter.text(
                        pos2(switch.right() + 8.0, rect.center().y),
                        Align2::LEFT_CENTER,
                        label,
                        font(styles.ui.size, false, false),
                        ink,
                    );
                }
                if response.clicked() {
                    emit.push(SceneMessage::Toggle(
                        route_of(scene, item, &node),
                        key,
                        !value,
                    ));
                }
            }
            "image" => drawn.extend(paint_image(scene, ui, &painter, item, &node, rect, ink)),
            _ => {}
        }
    }
    scene.textures.retain(|key, _| drawn.contains(key));
    tip_pass(scene, ui, tip_candidate, emit);
    if framed {
        ui.painter().rect_stroke(
            full,
            0.0,
            Stroke::new(1.0, colour(palette.border)),
            StrokeKind::Inside,
        );
    }
}

/// Advance the hover tip: show, move, hide, or ask to be woken when the
/// delay runs out (a one-shot deadline, never a frame loop).
fn tip_pass(
    scene: &mut SceneUi,
    ui: &Ui,
    candidate: Option<super::tip::Candidate>,
    emit: &mut Vec<SceneMessage>,
) {
    let ctx = ui.ctx().clone();
    let pressed = ctx.input(|input| input.pointer.any_pressed());
    let size = scene.text_styles().ui.size;
    let measure = |text: &str, font: &egui::FontId| {
        let size = ctx.fonts_mut(|fonts| {
            fonts
                .layout_no_wrap(text.to_owned(), font.clone(), Color32::WHITE)
                .size()
        });
        (size.x, size.y)
    };
    let size_of = |tip: &super::tip::Tip| super::tip::card(tip, size, &measure);
    match scene
        .hover
        .pass(std::time::Instant::now(), candidate, pressed, &size_of)
    {
        super::tip::Outcome::Same => {}
        super::tip::Outcome::Show(at) => emit.push(SceneMessage::Tooltip(Some(at))),
        super::tip::Outcome::Hide => emit.push(SceneMessage::Tooltip(None)),
        super::tip::Outcome::Wait(after) => ctx.request_repaint_after(after),
    }
}

fn paint_frame(
    scene: &SceneUi,
    ui: &mut Ui,
    full: Rect,
    title: &str,
    emit: &mut Vec<SceneMessage>,
) {
    let bar = scene.palette().secondary;
    let rect = Rect::from_min_size(full.min, vec2(full.width(), TITLEBAR));
    ui.painter().rect_filled(rect, 0.0, colour(bar.surface));
    let theme = scene.theme();
    let display =
        design::active_typography(Some(theme.typography()), design::TypographyRole::UiDisplay);
    let size = display.font_size as f32 * scene.text_scale();
    ui.painter().text(
        pos2(rect.left() + 12.0, rect.center().y),
        Align2::LEFT_CENTER,
        title,
        font(size, false, display.weight >= 600),
        colour(bar.foreground),
    );
    let Some(close) = scene
        .layout
        .close
        .map(|close| close.translate(full.min.to_vec2()))
    else {
        return;
    };
    let response = ui.interact(
        close,
        Id::unique(("scene-node", super::CLOSE_ID)),
        Sense::click(),
    );
    if response.hovered() {
        ui.painter()
            .rect_filled(close, 4.0, ui.visuals().widgets.hovered.weak_bg_fill);
    }
    ui.painter().text(
        close.center(),
        Align2::CENTER_CENTER,
        "×",
        font(16.0, false, false),
        colour(bar.foreground),
    );
    if response.clicked() {
        emit.push(SceneMessage::Close);
    }
}

fn paint_image(
    scene: &mut SceneUi,
    ui: &Ui,
    painter: &egui::Painter,
    item: &Placed,
    node: &Node,
    rect: Rect,
    ink: Color32,
) -> Option<(u64, u32, u32)> {
    let key = match &item.source {
        Source::Tree => (String::new(), String::new(), item.id.clone()),
        Source::Row { list, item: row } => (list.clone(), row.clone(), item.id.clone()),
    };
    let Some(asset) = scene.images.get(&key).cloned() else {
        // Always visible, in the design foreground, even when no icon theme
        // or application icon is installed.
        let size = number(node, "w")
            .unwrap_or(16.0)
            .min(number(node, "h").unwrap_or(16.0));
        toolkit::icons::image_of(ui.ctx(), toolkit::icons::Icon::Image, size, ink)
            .paint_at(ui, Rect::from_center_size(rect.center(), vec2(size, size)));
        return None;
    };
    let ppp = ui.ctx().pixels_per_point();
    let (pw, ph) = (
        (rect.width() * ppp).ceil().max(1.0) as u32,
        (rect.height() * ppp).ceil().max(1.0) as u32,
    );
    let (cache, texture, tint) = match &asset {
        crate::images::Asset::Raster(raster) => {
            let cache = (raster.id, 0, 0);
            let texture = scene.textures.entry(cache).or_insert_with(|| {
                let image = egui::ColorImage::from_rgba_unmultiplied(raster.size, &raster.rgba);
                ui.ctx().load_texture(
                    format!("scene-image-{}", raster.id),
                    image,
                    egui::TextureOptions::LINEAR,
                )
            });
            (cache, texture.id(), Color32::WHITE)
        }
        crate::images::Asset::Svg(svg) => {
            let cache = (svg.id, pw, ph);
            let texture = match scene.textures.get(&cache) {
                Some(texture) => texture.id(),
                None => {
                    let image = crate::images::rasterise(svg, pw, ph)?;
                    let handle = ui.ctx().load_texture(
                        format!("scene-svg-{}-{pw}x{ph}", svg.id),
                        image,
                        egui::TextureOptions::LINEAR,
                    );
                    let id = handle.id();
                    scene.textures.insert(cache, handle);
                    id
                }
            };
            let tint = if svg.symbolic {
                hex(port_text(node, "color")).unwrap_or(ink)
            } else {
                Color32::WHITE
            };
            (cache, texture, tint)
        }
    };
    let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
    painter.image(texture, rect, uv, tint);
    Some(cache)
}
