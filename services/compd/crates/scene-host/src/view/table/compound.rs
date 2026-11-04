// SPDX-License-Identifier: MIT OR Apache-2.0
//! Native local interaction over versioned, bounded model pages.
use std::sync::Arc;

use egui::{Id, Key, Rect, Sense, Ui, pos2, vec2};
use serde_json::{Value, json};

use super::model::{Edit, PAGE, State};
use super::{DragRow, deliver, modifiers};
use crate::view::{Route, SceneMessage, flag, number};

pub(super) fn paint(
    ui: &mut Ui,
    rect: Rect,
    clip: Rect,
    node: &scene::Node,
    route: Arc<Route>,
    out: &mut Vec<SceneMessage>,
    state: &mut State,
    text_epoch: u64,
) {
    let session = node
        .ports
        .get("session")
        .and_then(Value::as_str)
        .unwrap_or("");
    if state.session != session {
        *state = State::default();
        state.session = session.to_owned();
    }
    if node.ports["view"]["session"].as_str() != Some(session) {
        state.refused += 1;
        return;
    }
    state.model_owner = format!("{}.{}", route.scene, route.node);
    let old_generation = state.generation;
    let was_empty = state.total == 0;
    state.ingest(&node.ports["view"]);
    let mut reset_scroll = state.generation != old_generation || (was_empty && state.total > 0);
    if let Some(error) = state.model_error.clone() {
        if !state.model_error_sent {
            state.model_error_sent = true;
            let mut value = state.event("model_error");
            value["generation"] = node.ports["view"]["generation"].clone();
            value["error"] = json!(error);
            deliver(out, &route, value);
        }
        return;
    }
    state.text_epoch = text_epoch ^ u64::from(ui.ctx().pixels_per_point().to_bits());
    state.disable_text_cache = !node
        .ports
        .get("text_cache")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let font_size = number(node, "font_size").unwrap_or(13.0).clamp(6.0, 72.0);
    let secondary_scale = number(node, "secondary_scale")
        .unwrap_or(1.0)
        .clamp(0.25, 2.0);
    let id = Id::unique(("scene-table", &route.scene, &route.node));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
    child.set_clip_rect(clip.intersect(ui.clip_rect()));
    child.spacing_mut().item_spacing.y = 0.0;
    let background = child.interact(rect, id, Sense::click());
    let active = flag(node, "active");
    if active && !state.old_active {
        background.request_focus();
    }
    state.old_active = active;
    // This compound control owns navigation; egui must not move focus first.
    child.memory_mut(|m| {
        m.set_focus_lock_filter(
            id,
            egui::EventFilter {
                tab: true,
                horizontal_arrows: true,
                vertical_arrows: true,
                escape: true,
            },
        )
    });
    let row_h = number(node, "row_height")
        .unwrap_or(24.0)
        .clamp(16.0, 200.0);
    let columns = node.ports["columns"]
        .as_array()
        .map_or(1, Vec::len)
        .clamp(1, 16);
    // Cancellation must work even when the edited row is outside show_rows.
    if state.edit.is_some() && child.input(|i| i.key_pressed(Key::Escape)) {
        child.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Escape));
        state.edit = None;
        background.request_focus();
    }
    let editing = state.edit.is_some();
    let mut changed = false;
    let tab =
        child.memory(|m| m.had_focus_last_frame(id)) && child.input(|i| i.key_pressed(Key::Tab));
    if !editing && (background.has_focus() || tab) {
        for event in child.input(|i| i.events.clone()) {
            if let egui::Event::Key {
                key,
                pressed: true,
                modifiers: mods,
                ..
            } = event
            {
                let count = state.total;
                let focus = state.focus.unwrap_or(0);
                let target = match key {
                    Key::ArrowDown => Some((focus + 1).min(count.saturating_sub(1))),
                    Key::ArrowUp => Some(focus.saturating_sub(1)),
                    Key::Home => Some(0),
                    Key::End => Some(count.saturating_sub(1)),
                    _ => None,
                };
                if let Some(index) = target {
                    child.input_mut(|i| i.consume_key(mods, key));
                    background.request_focus();
                    state.pick(index, mods.ctrl, mods.shift);
                    state.scroll_to = Some(index);
                    changed = true;
                } else if key == Key::A && mods.ctrl {
                    child.input_mut(|i| i.consume_key(mods, key));
                    background.request_focus();
                    state.selection.range(0, count);
                    changed = true;
                } else if key == Key::Space {
                    child.input_mut(|i| i.consume_key(mods, key));
                    background.request_focus();
                    state.pick(focus, true, false);
                    changed = true;
                } else if key == Key::E && mods.ctrl && flag(node, "editable") {
                    child.input_mut(|i| i.consume_key(mods, key));
                    if let Some(row) = state.rows.get(&focus) {
                        state.edit = Some(Edit {
                            index: focus,
                            id: row.id.clone(),
                            text: row.edit.clone(),
                            pending: None,
                            focus: true,
                        });
                        child.ctx().request_repaint();
                    }
                } else if matches!(
                    key,
                    Key::Enter | Key::Tab | Key::ArrowLeft | Key::ArrowRight
                ) {
                    child.input_mut(|i| i.consume_key(mods, key));
                    if key == Key::Tab
                        && let Some(next) = node.ports.get("next_focus").and_then(Value::as_str)
                    {
                        child.memory_mut(|m| {
                            m.request_focus(Id::unique(("scene-table", &route.scene, next)))
                        });
                    }
                    let mut value = state.event("key");
                    value["key"] = json!(format!("{key:?}"));
                    value["modifiers"] = modifiers(mods);
                    deliver(out, &route, value);
                }
            }
        }
    }
    let mut scroll = egui::ScrollArea::vertical()
        .id_salt(id.with("scroll"))
        .auto_shrink([false, false]);
    if let Some(offset) = state.restore_scroll.take() {
        scroll = scroll.vertical_scroll_offset(offset);
    }
    if let Some(focus) = state.scroll_to.take() {
        if focus < state.first || focus >= state.end {
            reset_scroll = true;
            scroll = scroll
                .vertical_scroll_offset((focus as f32 * row_h - rect.height() / 2.0).max(0.0));
        }
    }
    if reset_scroll {
        // Setting offset alone leaves egui's kinetic velocity/target alive.
        // Reuse the stable container id but discard the old scroll physics.
        egui::scroll_area::State::default()
            .store(child.ctx(), child.make_persistent_id(id.with("scroll")));
        state.discard_scroll_tail = true;
    }
    if state.discard_scroll_tail {
        let (fresh_wheel, scrolling, hovered) = child.input(|input| {
            (
                input.events.iter().any(|event| matches!(event, egui::Event::MouseWheel { .. })),
                input.is_scrolling(),
                input.pointer.hover_pos().is_some_and(|pos| rect.contains(pos)),
            )
        });
        if !reset_scroll && fresh_wheel {
            state.discard_scroll_tail = false;
        } else {
            // Wheel smoothing belongs to InputState, outside ScrollArea::State.
            // Drain its old tail without cancelling a fresh wheel gesture or
            // consuming input over the other pane.
            if hovered {
                child.input_mut(|input| input.smooth_scroll_delta = egui::Vec2::ZERO);
            }
            if scrolling {
                child.ctx().request_repaint();
            } else {
                state.discard_scroll_tail = false;
            }
        }
    }
    let mut row_drop = false;
    let shown=scroll.show_rows(&mut child,row_h,state.total,|ui,range| {
        state.first=range.start; state.end=range.end;
        state.prepare_range(range.start,range.end);
        let missing=range.clone().find(|i| !state.rows.contains_key(i));
        let prefetch=(range.end+32).min(state.total.saturating_sub(1));
        let wanted=missing.or_else(|| (state.total>0 && !state.rows.contains_key(&prefetch)).then_some(prefetch));
        if let Some(first)=wanted {
            let offset=first/PAGE*PAGE;
            if state.request!=Some(offset) {
                state.request=Some(offset);
                deliver(out,&route,json!({"kind":"viewport","session":state.session,"generation":state.generation,"offset":offset,"count":PAGE}));
            }
        }
        let mut drawn=Vec::new();
        let mut commit=None;
        let mut cancel=false;
        for index in range {
            let (row_rect,_)=ui.allocate_exact_size(vec2(ui.available_width(),row_h),Sense::hover());
            let Some(row)=state.rows.get(&index).cloned() else {
                ui.painter().text(row_rect.left_center(),egui::Align2::LEFT_CENTER,"Loading…",crate::view::paint::font(font_size,false,false),ui.visuals().weak_text_color());
                continue;
            };
            let response=ui.interact(row_rect,id.with(&row.id),Sense::click_and_drag());
            response.widget_info(|| egui::WidgetInfo::selected(egui::Role::Row,true,state.selection.contains(index),row.cells.first().map(String::as_str).unwrap_or("")));
            if response.clicked() || response.double_clicked() {
                background.request_focus();
                let mods=ui.input(|i| i.modifiers);
                state.pick(index,mods.ctrl,mods.shift); changed=true;
                if response.double_clicked() {
                    let mut value=state.event("activate"); value["index"]=json!(index); value["item"]=row.raw.clone();
                    deliver(out,&route,value);
                }
            }
            if state.selection.contains(index) {
                ui.painter().rect_filled(row_rect,0.0,ui.visuals().selection.bg_fill);
            } else if response.hovered() {
                ui.painter().rect_filled(row_rect,0.0,ui.visuals().widgets.hovered.bg_fill);
            }
            // Construct payload only when egui actually starts dragging.
            if response.drag_started() {
                if !state.selection.contains(index) {state.pick(index,false,false); changed=true;}
                response.dnd_set_drag_payload(DragRow {node:route.node.clone(),item:row.raw.clone(),generation:state.generation,session:state.session.clone()});
            }
            if let Some(payload)=response.dnd_release_payload::<DragRow>() {
                row_drop=true;
                let mut value=state.event("drop");
                value["source_node"]=json!(payload.node); value["source_generation"]=json!(payload.generation); value["source_session"]=json!(payload.session);
                value["item"]=payload.item.clone(); value["target"]=row.raw.clone();
                deliver(out,&route,value);
            }
            if let Some(edit)=state.edit.as_mut().filter(|e| e.index==index) {
                let enter=ui.input(|i| i.key_pressed(Key::Enter));
                let escape=ui.input(|i| i.key_pressed(Key::Escape));
                let first=(row_rect.width()-100.0*columns.saturating_sub(1) as f32).max(60.0);
                let edit_rect=Rect::from_min_size(row_rect.min,vec2(first,row_h));
                let response=ui.put(edit_rect,egui::TextEdit::singleline(&mut edit.text).id(id.with("edit")).interactive(edit.pending.is_none()));
                if edit.focus {response.request_focus(); edit.focus=false;}
                if edit.pending.is_none() {
                    cancel=escape;
                    if enter {
                        commit=Some((edit.id.clone(),edit.text.clone()));
                    }
                }
            }
            drawn.push((index,row_rect,row));
        }
        if cancel {state.edit=None; background.request_focus();}
        if let Some((row_id,text))=commit {
            let mut value=state.event("edit_commit"); value["id"]=json!(row_id); value["text"]=json!(text);
            if let Some(edit)=state.edit.as_mut() {edit.pending=Some(state.seq);}
            deliver(out,&route,value);
        }
        // Column-major text, one clip per column where rows fit their height.
        // The per-cell path remains a controlled diagnostic option.
        for col in 0..columns {
            for (index,row_rect,row) in &drawn {
                if col==0 && state.edit.as_ref().is_some_and(|e| e.index==*index) {continue;}
                let Some(text)=row.cells.get(col) else {continue;};
                let first=(row_rect.width()-100.0*columns.saturating_sub(1) as f32).max(60.0);
                let x=row_rect.left()+4.0+if col==0 {0.0} else {first+(col-1) as f32*100.0};
                let width=if col==0 {first} else {100.0};
                let galley=state.galley(ui,*index,col,text,crate::view::paint::font(font_size*if col==0 {1.0} else {secondary_scale},false,false));
                let column_clip=Rect::from_min_max(pos2(x,ui.clip_rect().top()),pos2((x+width-4.0).min(row_rect.right()),ui.clip_rect().bottom()));
                let clip=if flag(node,"clip_columns") && galley.size().y<=row_h {column_clip} else {column_clip.intersect(*row_rect)};
                ui.painter().with_clip_rect(clip.intersect(ui.clip_rect())).galley(pos2(x,row_rect.center().y-galley.size().y/2.0),galley,ui.visuals().text_color());
            }
        }
    });
    state.scroll_y = shown.state.offset().y;
    if changed {
        let value = state.event("selection");
        deliver(out, &route, value);
    }
    if !row_drop && let Some(payload) = background.dnd_release_payload::<DragRow>() {
        let mut value = state.event("drop");
        value["source_node"] = json!(payload.node);
        value["source_generation"] = json!(payload.generation);
        value["source_session"] = json!(payload.session);
        value["item"] = payload.item.clone();
        value["target"] = Value::Null;
        deliver(out, &route, value);
    }
}
