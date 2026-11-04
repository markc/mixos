// SPDX-License-Identifier: MIT OR Apache-2.0
// Lab-only generic viewport table. Model, selection and file policy stay in Mix.
use std::sync::Arc;

use egui::{Align2, Id, Key, Rect, Sense, Ui, pos2, vec2};
use serde_json::{Value, json};

use super::{Route, SceneMessage, flag, number};

#[derive(Clone)]
struct DragRow {
    node: String,
    item: Value,
}

fn modifiers(m: egui::Modifiers) -> Value {
    json!({"ctrl":m.ctrl,"shift":m.shift,"alt":m.alt,"command":m.command})
}

fn deliver(out: &mut Vec<SceneMessage>, route: &Arc<Route>, value: Value) {
    out.push(SceneMessage::Interaction(Arc::clone(route), value));
}

fn batch_start(first: usize) -> usize {
    first.saturating_sub(32)
}

pub(super) fn paint(
    ui: &mut Ui,
    rect: Rect,
    clip: Rect,
    node: &scene::Node,
    route: Arc<Route>,
    out: &mut Vec<SceneMessage>,
) {
    let id = Id::unique(("scene-table", &route.scene, &route.node));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
    child.set_clip_rect(clip.intersect(ui.clip_rect()));
    let background = child.interact(rect, id, Sense::click());
    let active = flag(node, "active");
    let old_active = child
        .ctx()
        .data_mut(|d| d.get_temp::<bool>(id.with("active")))
        .unwrap_or(false);
    if active && !old_active {
        background.request_focus();
    }
    child
        .ctx()
        .data_mut(|d| d.insert_temp(id.with("active"), active));
    if background.has_focus() {
        let events = child.input(|i| i.events.clone());
        for event in events {
            if let egui::Event::Key {
                key,
                pressed: true,
                modifiers: mods,
                ..
            } = event
                && matches!(
                    key,
                    Key::ArrowDown
                        | Key::ArrowUp
                        | Key::Home
                        | Key::End
                        | Key::Enter
                        | Key::Space
                        | Key::Tab
                        | Key::ArrowRight
                        | Key::ArrowLeft
                        | Key::A
                )
            {
                child.input_mut(|i| i.consume_key(mods, key));
                deliver(
                    out,
                    &route,
                    json!({"kind":"key","key":format!("{key:?}"),"modifiers":modifiers(mods)}),
                );
            }
        }
    }
    let total = number(node, "total").unwrap_or(0.0).clamp(0.0, 1_000_000.0) as usize;
    let offset = number(node, "offset").unwrap_or(0.0).max(0.0) as usize;
    let row_h = number(node, "row_height")
        .unwrap_or(24.0)
        .clamp(16.0, 200.0);
    let rows = node
        .ports
        .get("rows")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let columns = node
        .ports
        .get("columns")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let focus = number(node, "focus").unwrap_or(-1.0) as i64;
    let previous_focus = child
        .ctx()
        .data_mut(|d| d.get_temp::<i64>(id.with("focus")))
        .unwrap_or(-1);
    child
        .ctx()
        .data_mut(|d| d.insert_temp(id.with("focus"), focus));
    let previous_range = child
        .ctx()
        .data_mut(|d| d.get_temp::<(usize, usize)>(id.with("range")));
    let mut scroll = egui::ScrollArea::vertical()
        .id_salt(id.with("scroll"))
        .auto_shrink([false, false]);
    if focus >= 0
        && focus != previous_focus
        && previous_range.is_some_and(|(a, b)| (focus as usize) < a || (focus as usize) >= b)
    {
        scroll =
            scroll.vertical_scroll_offset((focus as f32 * row_h - rect.height() / 2.0).max(0.0));
    }
    let mut row_drop = false;
    let shown = scroll.show_rows(&mut child, row_h, total, |ui, range| {
        ui.ctx().data_mut(|d| d.insert_temp(id.with("range"), (range.start, range.end)));
        let missing = range.start < offset || range.end > offset.saturating_add(rows.len());
        if missing {
            let wanted = batch_start(range.start);
            let previous = ui.ctx().data_mut(|d| d.get_temp::<usize>(id.with("request")));
            if previous != Some(wanted) {
                ui.ctx().data_mut(|d| d.insert_temp(id.with("request"), wanted));
                deliver(out, &route, json!({"kind":"viewport","offset":wanted,"count":200}));
            }
        } else {
            ui.ctx().data_mut(|d| d.remove::<usize>(id.with("request")));
        }
        for index in range {
            let (row_rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), row_h), Sense::hover());
            let Some(row) = index.checked_sub(offset).and_then(|i| rows.get(i)) else {
                ui.painter().text(row_rect.left_center(), Align2::LEFT_CENTER, "Loading…", super::paint::font(13.0, false, false), ui.visuals().weak_text_color());
                continue;
            };
            let response = ui.interact(row_rect, id.with(row["id"].as_str().unwrap_or("")), Sense::click_and_drag());
            if row["selected"].as_bool().unwrap_or(false) {
                ui.painter().rect_filled(row_rect, 0.0, ui.visuals().selection.bg_fill);
            } else if response.hovered() {
                ui.painter().rect_filled(row_rect, 0.0, ui.visuals().widgets.hovered.bg_fill);
            }
            if let Some(cells) = row["cells"].as_array() {
                let count = columns.len().clamp(1, 16);
                let tail = 100.0 * count.saturating_sub(1) as f32;
                let first_w = (row_rect.width() - tail).max(60.0);
                let mut x = row_rect.left() + 4.0;
                for (col, cell) in cells.iter().take(count).enumerate() {
                    let width = if col == 0 { first_w } else { 100.0 };
                    let cell_clip = Rect::from_min_max(pos2(x, row_rect.top()), pos2((x + width - 4.0).min(row_rect.right()), row_rect.bottom()));
                    ui.painter().with_clip_rect(cell_clip.intersect(ui.clip_rect())).text(pos2(x, row_rect.center().y), Align2::LEFT_CENTER, cell.as_str().unwrap_or(""), super::paint::font(13.0, false, false), ui.visuals().text_color());
                    x += width;
                }
            }
            if response.clicked() || response.double_clicked() {
                background.request_focus();
                deliver(out, &route, json!({"kind":if response.double_clicked(){"double_click"}else{"click"},"index":index,"item":row,"modifiers":modifiers(ui.input(|i|i.modifiers))}));
            }
            response.dnd_set_drag_payload(DragRow { node: route.node.clone(), item: row.clone() });
            if let Some(payload) = response.dnd_release_payload::<DragRow>() {
                row_drop = true;
                deliver(out, &route, json!({"kind":"drop","source_node":payload.node,"item":payload.item,"target":row}));
            }
        }
    });
    if !row_drop && let Some(payload) = background.dnd_release_payload::<DragRow>() {
        deliver(
            out,
            &route,
            json!({"kind":"drop","source_node":payload.node,"item":payload.item,"target":null}),
        );
    }
    // The scroll area changes are native UI state, with no event per pixel.
    let _ = shown;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn viewport_batch_includes_the_requested_first_row_without_underflow() {
        for first in [0, 1, 31, 32, 33, 500, 10_000] {
            let start = batch_start(first);
            assert!(start <= first && first - start <= 32);
            assert!(first < start + 200);
        }
    }

    fn node() -> scene::Node {
        serde_json::from_value(json!({"family":"table","ports":{"rows":[],"columns":["Name"],"total":10000,"offset":0,"active":true,"on_change":"table_event"},"line":1,"is_template":false})).unwrap()
    }

    fn route() -> Arc<Route> {
        Arc::new(Route {
            scene: "test".into(),
            citizen: "test".into(),
            node: "table".into(),
            item: None,
            click: None,
            context: None,
            change: Some("table_event".into()),
            submit: None,
        })
    }

    #[test]
    fn empty_viewport_requests_one_bounded_batch_and_deduplicates_frames() {
        let ctx = egui::Context::default();
        let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(400.0, 300.0));
        let mut events = Vec::new();
        for _ in 0..2 {
            let _output = ctx.run_ui(Default::default(), |ui| {
                paint(ui, rect, rect, &node(), route(), &mut events)
            });
        }
        let values: Vec<_> = events
            .iter()
            .filter_map(super::super::event_of)
            .map(|(_, _, v)| v)
            .collect();
        assert_eq!(values.len(), 1);
        assert_eq!(
            values[0]["value"],
            json!({"kind":"viewport","offset":0,"count":200})
        );
    }

    #[test]
    fn keyboard_modifiers_cross_the_same_structured_event_route() {
        let ctx = egui::Context::default();
        let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(400.0, 300.0));
        let mut events = Vec::new();
        let _output = ctx.run_ui(Default::default(), |ui| {
            paint(ui, rect, rect, &node(), route(), &mut events)
        });
        events.clear();
        let input = egui::RawInput {
            events: vec![egui::Event::Key {
                key: Key::ArrowDown,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers {
                    shift: true,
                    ..Default::default()
                },
            }],
            ..Default::default()
        };
        let _output = ctx.run_ui(input, |ui| {
            paint(ui, rect, rect, &node(), route(), &mut events)
        });
        let value = events
            .iter()
            .filter_map(super::super::event_of)
            .map(|(_, _, v)| v)
            .find(|v| v["value"]["kind"] == "key")
            .expect("key event");
        assert_eq!(value["value"]["key"], "ArrowDown");
        assert_eq!(value["value"]["modifiers"]["shift"], true);
    }
}
