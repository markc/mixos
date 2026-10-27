// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Renderer-free window geometry executor, shared by the compositor and its
//! protocol fixtures. Callers resolve windows across their owning worlds before
//! borrowing the placement Space, and own redraw and pointer invalidation.

use std::collections::BTreeMap;

use crate::camera::transform::translate::slot;
use crate::comp::{CompState, FloatGeometry, Interactive, Placement, usable::Reserved};
use crate::window::interface::record::window::LoopWindow;
use protocols::window::shell;
use smithay::desktop::{Space, Window};
use smithay::output::Output;
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::utils::{Logical, Point, Rectangle, Size};
use surfaces::SurfaceId;

/// Bus and keyboard placement share this renderer-free operation. The caller
/// resolves the owning world before passing its Space and window.
pub fn place_as(
    comp: &mut CompState,
    space: &mut Space<Window>,
    window: &Window,
    id: u64,
    generation: u64,
    placement: comp_model::request::PlacementKind,
    output: Option<&str>,
) -> comp_model::reply::ControlReply {
    use comp_model::reply::ControlReply;
    use comp_model::request::PlacementKind;
    use serde_json::json;
    let sid = match comp.registry.resolve_window_target(id, Some(generation)) {
        Ok(record) => record.id(),
        Err(error) => return ControlReply::WindowTarget { id, error },
    };
    if fullscreen_owns(window) {
        return ControlReply::refused("invalid_state", json!({"id": id, "state": "fullscreen"}));
    }
    let selected = match output {
        Some(requested) => match space.outputs().find(|o| o.name() == requested || comp_model::snapshot::output_key(&o.name()) == requested) {
            Some(output) => Some(output.name()),
            None => return ControlReply::refused("unknown_output", json!({"output": requested})),
        },
        None => None,
    };
    let before = comp.placement(sid);
    let next = match placement {
        PlacementKind::Floating => Placement::Floating,
        PlacementKind::Maximized | PlacementKind::Tiled { .. } => {
            let Some((output, area)) = target_on(comp, space, sid, window, selected.as_deref()) else {
                return ControlReply::refused("unknown_output", json!({"output": selected}));
            };
            let (min, max) = size_hints(window);
            let fixed = (max.0 > 0 && min.0 >= max.0) || (max.1 > 0 && min.1 >= max.1);
            match placement {
                PlacementKind::Maximized => {
                    let content = decor::window::content_area(window, area);
                    if fixed || (max.0 > 0 && max.0 < content.size.w) || (max.1 > 0 && max.1 < content.size.h) {
                        return ControlReply::refused("not_maximizable", json!({"id": id}));
                    }
                    Placement::Maximized { output }
                }
                PlacementKind::Tiled { slot } => {
                    if fixed {
                        return ControlReply::refused("not_resizable", json!({"id": id}));
                    }
                    let Some(slot) = super::TileSlot::from_name(slot) else {
                        return ControlReply::refused("invalid_argument", json!({"argument": "slot"}));
                    };
                    Placement::Tiled { slot, output }
                }
                PlacementKind::Floating => unreachable!(),
            }
        }
    };
    // Cancel an armed drag before applying explicit policy, so releasing its
    // button cannot overwrite this command later in the same dispatch.
    finish_interactive(comp, sid);
    set_placement(comp, space, sid, window, next);
    placement_reply(comp, sid, generation, before != comp.placement(sid))
}

fn placement_reply(comp: &CompState, id: SurfaceId, generation: u64, changed: bool) -> comp_model::reply::ControlReply {
    let placement = comp.placement(id);
    comp_model::reply::ControlReply::Body(serde_json::json!({
        "id": id.0, "generation": generation, "changed": changed,
        "placement": placement.name(), "maximized": placement.maximized(),
        "output": placement.output().map(comp_model::snapshot::output_key),
        "float_geometry": comp.float_snapshot(id),
    }))
}

/// The only quick-tile operation, reached by both Super+arrow and tile_key.
pub fn tile_key(
    comp: &mut CompState,
    space: &mut Space<Window>,
    window: &Window,
    id: u64,
    generation: u64,
    direction: comp_model::request::TileDirection,
) -> comp_model::reply::ControlReply {
    use super::placement::{TileTransition, adjacent_output, tile_transition};
    use comp_model::reply::ControlReply;
    let sid = match comp.registry.resolve_window_target(id, Some(generation)) {
        Ok(record) => record.id(),
        Err(error) => return ControlReply::WindowTarget { id, error },
    };
    if fullscreen_owns(window) {
        return ControlReply::refused("invalid_state", serde_json::json!({"id": id, "state": "fullscreen"}));
    }
    let current = comp.placement(sid);
    let (slot, output) = match tile_transition(&current, direction) {
        TileTransition::Slot(slot) => (slot, None),
        TileTransition::Cross { direction, slot } => {
            let areas: Vec<_> = space.outputs().filter_map(|o| Some((o.name(), space.output_geometry(o)?))).collect();
            let area = areas.iter().find(|(name, _)| Some(name.as_str()) == current.output()).map(|(_, area)| *area);
            let adjacent = area.and_then(|area| adjacent_output(area, direction, areas.iter().map(|(name, area)| (name.as_str(), *area))));
            let Some(output) = adjacent else {
                return placement_reply(comp, sid, generation, false);
            };
            (slot, Some(output.to_owned()))
        }
    };
    place_as(comp, space, window, id, generation, comp_model::request::PlacementKind::Tiled { slot: slot.name() }, output.as_deref())
}

/// Tile membership is queried from placement, independent of focus or buffers.
pub fn tiles_get(comp: &CompState, space: &Space<Window>, output: Option<&str>, workspace: Option<u32>) -> comp_model::reply::ControlReply {
    use comp_model::reply::ControlReply;
    use policy::workspaces::{DefaultOutput, WorkspaceState};
    use serde_json::json;
    let selected = space.outputs().find(|o| output.is_none_or(|name| o.name() == name || comp_model::snapshot::output_key(&o.name()) == name));
    let default = selected.map(|o| DefaultOutput { key: comp_model::snapshot::output_key(&o.name()), name: o.name() });
    let Some(key) = WorkspaceState::resolve_output(default.as_ref(), output) else {
        return ControlReply::refused("unknown_output", json!({"output": output}));
    };
    let output = default.expect("resolved output").name;
    let workspace = workspace.unwrap_or_else(|| comp.workspaces.current_for(Some(&key)));
    if !(1..=comp.workspaces.count).contains(&workspace) {
        return ControlReply::refused("invalid_argument", json!({"argument": "workspace", "count": comp.workspaces.count}));
    }
    let split = comp.tile_split_for(&output, workspace);
    let slots: serde_json::Map<_, _> = super::TileSlot::ALL.into_iter().map(|slot| {
        let mut windows: Vec<_> = comp.registry.surface_rows().filter(|record| record.mapped() && record.workspace() == Some(workspace))
            .filter(|record| comp.placement(record.id()) == (Placement::Tiled { slot, output: output.clone() }))
            .map(|record| (record.id().0, record.generation())).collect();
        windows.sort_unstable();
        (slot.name().to_owned(), json!(windows.into_iter().map(|(id, generation)| json!({"id": id, "generation": generation})).collect::<Vec<_>>()))
    }).collect();
    ControlReply::Body(json!({"output": key, "workspace": workspace, "split": {"x": split.x, "y": split.y}, "slots": slots}))
}

/// Work-area observation and actual window placement/configure changes are
/// separate: changing an empty output's usable area need not move a window.
/// `windows` reports a geometry application; an explicit request may still send
/// a protocol response without changing placement or its decided slot.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GeometryChange {
    pub usable: bool,
    pub windows: bool,
}

/// Layer-shell exclusive zones, followed by compositor-owned docked panels.
pub fn usable_area(
    output: &Output,
    geometry: Rectangle<i32, Logical>,
    reserved: Reserved,
) -> Rectangle<i32, Logical> {
    let map = smithay::desktop::layer_map_for_output(output);
    let layered = if map.layers().next().is_none() {
        geometry
    } else {
        let zone = map.non_exclusive_zone();
        Rectangle::new(
            geometry.loc + zone.loc,
            (zone.size.w.max(0), zone.size.h.max(0)).into(),
        )
    };
    reserved.shrink(layered)
}

fn target(
    comp: &CompState,
    space: &Space<Window>,
    id: SurfaceId,
    window: &Window,
) -> Option<(String, Rectangle<i32, Logical>)> {
    target_on(comp, space, id, window, None)
}

fn target_on(
    comp: &CompState,
    space: &Space<Window>,
    id: SurfaceId,
    window: &Window,
    preferred: Option<&str>,
) -> Option<(String, Rectangle<i32, Logical>)> {
    let owner = comp.placement(id);
    // An explicit output disappearing must leave placement alone, never fall
    // back to an unrelated display between policy validation and execution.
    let explicit = match preferred {
        Some(name) => Some(
            space
                .outputs()
                .find(|o| o.name() == name && space.output_geometry(o).is_some())?
                .clone(),
        ),
        None => None,
    };
    let output = explicit
        .or_else(|| {
            owner.output().and_then(|name| {
                space
                    .outputs()
                    .find(|output| output.name() == name && space.output_geometry(output).is_some())
                    .cloned()
            })
        })
        // Overlap membership can still contain a just-unmapped output until
        // Space refreshes. It must not prevent an available-output fallback.
        .or_else(|| {
            space
                .outputs_for_element(window)
                .into_iter()
                .find(|output| space.output_geometry(output).is_some())
        })
        .or_else(|| {
            space
                .outputs()
                .find(|output| space.output_geometry(output).is_some())
                .cloned()
        })?;
    let name = output.name();
    let reserved = comp.reserved.get(&name).copied().unwrap_or_default();
    let outer = usable_area(&output, space.output_geometry(&output)?, reserved);
    Some((name, outer))
}

/// An explicit maximise/unmaximise request. With no available output, a new
/// maximise request makes no change; existing placement and float geometry survive.
pub fn set_maximized(
    comp: &mut CompState,
    space: &mut Space<Window>,
    id: SurfaceId,
    window: &Window,
    enabled: bool,
) -> GeometryChange {
    set_maximized_on(comp, space, id, window, enabled, None)
}

/// Explicit native output selection for saved maximised frames. Existing
/// restore geometry survives a retarget; neither focus nor workspace changes.
pub fn set_maximized_on(
    comp: &mut CompState,
    space: &mut Space<Window>,
    id: SurfaceId,
    window: &Window,
    enabled: bool,
    output: Option<&str>,
) -> GeometryChange {
    let placement = if enabled {
        let Some((output, _)) = target_on(comp, space, id, window, output) else {
            return GeometryChange::default();
        };
        Placement::Maximized { output }
    } else {
        Placement::Floating
    };
    set_placement(comp, space, id, window, placement)
}

/// Shared placement transition for drag, keyboard and Bus commands.
pub fn set_placement(
    comp: &mut CompState,
    space: &mut Space<Window>,
    id: SurfaceId,
    window: &Window,
    placement: Placement,
) -> GeometryChange {
    transition(comp, space, id, window, placement, true)
}

fn transition(
    comp: &mut CompState,
    space: &mut Space<Window>,
    id: SurfaceId,
    window: &Window,
    placement: Placement,
    save_float: bool,
) -> GeometryChange {
    let previous = comp.placement(id);
    let restoring_float = previous.constrained() && placement == Placement::Floating;
    if save_float && placement.constrained() && previous == Placement::Floating {
        let Some((output, area)) = target(comp, space, id, window) else {
            return GeometryChange::default();
        };
        let (location, size) = window.fullscreen().map_or_else(
            || {
                (
                    space.element_location(window).unwrap_or(area.loc),
                    slot::size_of(window).unwrap_or(window.geometry().size),
                )
            },
            |restore| (restore.restore_loc, restore.restore_size),
        );
        // A window initially mapped maximised may never have had a floating
        // buffer. Leave the geometry absent so drag-away uses the default.
        if size.w > 0 && size.h > 0 {
            comp.save_float_geometry(
                id,
                FloatGeometry {
                    location,
                    size,
                    output,
                },
            );
        }
    }
    comp.set_placement(id, window, placement);
    if fullscreen_owns(window) {
        // Fullscreen retains the visible slot until exit. A transition back to
        // Floating changes where that exit must land, without moving it now.
        if restoring_float
            && let Some(mut restore) = window.fullscreen()
            && let Some(area) = floating_area(comp, space, id, window)
        {
            restore.restore_loc = area.loc;
            restore.restore_size = area.size;
            window.set_fullscreen(Some(restore));
        }
        return GeometryChange::default();
    }
    let area = if comp.placement(id) == Placement::Floating {
        if !previous.constrained() {
            shell::set_maximized(window, false);
            shell::send(window);
            return GeometryChange::default();
        }
        floating_area(comp, space, id, window)
    } else {
        placement_area(comp, space, id, window)
    };
    let Some(area) = area else {
        return GeometryChange::default();
    };
    apply(space, window, area, comp.placement(id).maximized());
    // An external restore may arrive after a move captured the maximised frame
    // but before its first motion. Keep the host start on the restored frame;
    // the dispatcher still reports cumulative travel from the original press.
    if restoring_float
        && let Some(grab) = comp
            .interactive
            .as_mut()
            .filter(|grab| grab.id == id && grab.edges == 0)
    {
        grab.start = Some((area.loc, area.size));
    }
    GeometryChange {
        usable: false,
        windows: true,
    }
}

fn floating_area(
    comp: &CompState,
    space: &Space<Window>,
    id: SurfaceId,
    window: &Window,
) -> Option<Rectangle<i32, Logical>> {
    comp.float_geometry(id)
        .map(|saved| Rectangle::new(saved.location, saved.size))
        .or_else(|| {
            target(comp, space, id, window)
                .map(|(_, area)| decor::window::content_area(window, default_float(area)))
        })
}

fn default_float(area: Rectangle<i32, Logical>) -> Rectangle<i32, Logical> {
    let size: Size<i32, Logical> =
        ((area.size.w * 2 / 3).max(1), (area.size.h * 2 / 3).max(1)).into();
    Rectangle::new(
        area.loc + Point::from(((area.size.w - size.w) / 2, (area.size.h - size.h) / 2)),
        size,
    )
}

/// Fresh content geometry for the placement; fullscreen exit uses this too.
pub fn placement_area(
    comp: &CompState,
    space: &Space<Window>,
    id: SurfaceId,
    window: &Window,
) -> Option<Rectangle<i32, Logical>> {
    let (_, outer) = target(comp, space, id, window)?;
    match comp.placement(id) {
        Placement::Floating => None,
        Placement::Maximized { .. } => Some(decor::window::content_area(window, outer)),
        Placement::Tiled { slot: tile, output } => {
            let workspace = comp
                .registry
                .get(id)
                .and_then(|record| record.workspace())
                .unwrap_or_else(|| comp.current_workspace());
            let split = comp.tile_split_for(&output, workspace);
            let area = decor::window::content_area(window, tile.rectangle(outer, split));
            let (w, h) = clamp_size((area.size.w, area.size.h), size_hints(window));
            Some(Rectangle::new(
                area.loc + Point::from(((area.size.w - w) / 2, (area.size.h - h) / 2)),
                (w, h).into(),
            ))
        }
    }
}

/// Refresh work areas and their maximised or tiled windows. The candidates
/// must be resolved by the caller across all worlds, not just this Space.
pub fn refresh_usable(
    comp: &mut CompState,
    space: &mut Space<Window>,
    windows: &[(SurfaceId, Window)],
) -> GeometryChange {
    let usable: BTreeMap<_, _> = space
        .outputs()
        .filter_map(|output| {
            let name = output.name();
            let reserved = comp.reserved.get(&name).copied().unwrap_or_default();
            Some((
                name,
                usable_area(output, space.output_geometry(output)?, reserved),
            ))
        })
        .collect();
    let changed = usable != comp.usable;
    if changed {
        comp.usable = usable;
        comp.outputs_changed();
    }
    let mut change = GeometryChange {
        usable: changed,
        windows: false,
    };
    for (id, window) in windows {
        let placement = comp.placement(*id);
        if !placement.constrained() {
            continue;
        }
        // A dormant world's window may be resolved by the caller, but this
        // reconciliation must not admit it into the current placement Space.
        if space.element_location(window).is_none() {
            continue;
        }
        // Fullscreen owns its geometry through delayed entry/exit commits.
        // Revisit on the next normal dispatch after it releases ownership,
        // even when the work-area map itself has not changed again.
        if fullscreen_owns(window) {
            continue;
        }
        let Some((output, _)) = target(comp, space, *id, window) else {
            continue;
        };
        if placement.output() != Some(output.as_str()) {
            comp.set_placement(*id, window, placement.clone().on_output(output.clone()));
        }
        let workspace = comp
            .registry
            .get(*id)
            .and_then(|record| record.workspace())
            .unwrap_or_else(|| comp.current_workspace());
        comp.tile_split(&output, workspace);
        let Some(area) = placement_area(comp, space, *id, window) else {
            continue;
        };
        let maximized = window.toplevel().map_or_else(
            || window.x11_surface().is_some_and(|x11| x11.is_maximized()),
            |toplevel| {
                toplevel.with_pending_state(|state| {
                    state.states.contains(xdg_toplevel::State::Maximized)
                })
            },
        );
        if space.element_location(window) != Some(area.loc)
            || slot::decided_size(window) != Some(area.size)
            || maximized != placement.maximized()
        {
            apply(space, window, area, placement.maximized());
            change.windows = true;
        }
    }
    change
}

fn fullscreen_owns(window: &Window) -> bool {
    window.is_fullscreen()
        || protocols::window::ident::states(window).fullscreen
        || window.toplevel().is_some_and(|toplevel| {
            toplevel.with_committed_state(|state| {
                state.is_some_and(|state| state.states.contains(xdg_toplevel::State::Fullscreen))
            })
        })
}

pub fn size_hints(window: &Window) -> ((i32, i32), (i32, i32)) {
    if let Some(x11) = window.x11_surface() {
        return (
            x11.min_size().map_or((0, 0), |s| (s.w, s.h)),
            x11.max_size().map_or((0, 0), |s| (s.w, s.h)),
        );
    }
    window
        .toplevel()
        .map(|toplevel| {
            smithay::wayland::compositor::with_states(toplevel.wl_surface(), |states| {
                let mut cached = states
                    .cached_state
                    .get::<smithay::wayland::shell::xdg::SurfaceCachedState>();
                let current = cached.current();
                (
                    (current.min_size.w, current.min_size.h),
                    (current.max_size.w, current.max_size.h),
                )
            })
        })
        .unwrap_or_default()
}

pub fn clamp_size(size: (i32, i32), (min, max): ((i32, i32), (i32, i32))) -> (i32, i32) {
    let axis = |value: i32, min: i32, max: i32| {
        let value = value.max(min.max(1));
        if max > 0 {
            value.min(max.max(min.max(1)))
        } else {
            value
        }
    };
    (axis(size.0, min.0, max.0), axis(size.1, min.1, max.1))
}

/// End a grab for an explicit policy operation, without committing a drag zone.
/// The caller applies the ended grab and redraws to hide any preview.
pub fn finish_interactive(comp: &mut CompState, id: SurfaceId) -> bool {
    let Some(grab) = comp.interactive.as_mut().filter(|grab| grab.id == id) else {
        return false;
    };
    grab.ended = true;
    grab.updated = false;
    if let Some(drag) = comp.drag.as_mut() {
        drag.armed = None;
    }
    true
}

/// Apply the host's coalesced delta. The dispatcher's pointer start stays immutable.
/// Restore and movement share one map operation, before a frame can be drawn.
pub fn apply_interactive(
    comp: &mut CompState,
    space: &mut Space<Window>,
    window: &Window,
    grab: &mut Interactive,
) -> bool {
    let pointer = grab.pointer_start + Point::from(grab.delta);
    apply_interactive_at(comp, space, window, grab, pointer, false)
}

/// Host input uses the physical cursor converted to output-layout logical
/// coordinates. Dispatcher deltas remain in the window's world for movement.
pub fn apply_interactive_at(
    comp: &mut CompState,
    space: &mut Space<Window>,
    window: &Window,
    grab: &mut Interactive,
    pointer: Point<f64, Logical>,
    shift: bool,
) -> bool {
    if grab.edges != 0 {
        return move_window(comp, space, window, grab);
    }
    if fullscreen_owns(window) {
        grab.ended = true;
        return comp.drag.take().is_some();
    }
    let mut drag = comp.drag.take().unwrap_or_else(|| {
        super::tiling::Drag::new(comp.placement(grab.id), comp.float_geometry(grab.id))
    });
    let start = *drag.start.get_or_insert_with(|| {
        Rectangle::new(
            space.element_location(window).unwrap_or_default(),
            slot::size_of(window).unwrap_or(window.geometry().size),
        )
    });
    if drag.start_output.is_none() {
        drag.start_output = target(comp, space, grab.id, window).map(|(name, _)| name);
    }
    if grab.cancelled {
        comp.set_placement(grab.id, window, drag.placement.clone());
        apply(space, window, start, drag.placement.maximized());
        restore_saved(comp, grab.id, drag.float_geometry);
        return true;
    }
    // A coalesced Begin/Update/End still evaluates the final travel. An End
    // alone commits the last armed zone, without re-sampling modifiers.
    let sample = grab.updated || !grab.ended;
    let changed = move_window(comp, space, window, grab);
    let before = drag.armed.clone();
    if sample {
        drag.armed = super::tiling::armed(comp, space, window, grab.id, pointer, shift);
    }
    let preview_changed = drag.armed != before;
    if grab.ended {
        if let Some(armed) = drag.armed {
            let saved = if drag.placement == Placement::Floating {
                drag.start_output.map(|output| FloatGeometry {
                    location: start.loc,
                    size: start.size,
                    output,
                })
            } else {
                drag.float_geometry
            };
            transition(comp, space, grab.id, window, armed.placement, false);
            restore_saved(comp, grab.id, saved);
            return true;
        }
        return changed || before.is_some();
    }
    comp.drag = Some(drag);
    changed || preview_changed
}

fn restore_saved(comp: &mut CompState, id: SurfaceId, saved: Option<FloatGeometry>) {
    match saved {
        Some(saved) => {
            comp.float_geometries.insert(id, saved);
        }
        None => {
            comp.float_geometries.remove(&id);
        }
    }
}

fn move_window(
    comp: &mut CompState,
    space: &mut Space<Window>,
    window: &Window,
    grab: &mut Interactive,
) -> bool {
    if fullscreen_owns(window) || (grab.edges != 0 && comp.placement(grab.id).constrained()) {
        grab.ended = true;
        return false;
    }
    let (mut origin, size) = *grab.start.get_or_insert_with(|| {
        (
            space.element_location(window).unwrap_or_default(),
            slot::size_of(window).unwrap_or(window.geometry().size),
        )
    });
    let extents = decor::window::extents(window);
    let (left, top, right, bottom) = extents.map_or((0.0, 0.0, 0.0, 0.0), |e| {
        (
            f64::from(e.left),
            f64::from(e.top),
            f64::from(e.right),
            f64::from(e.bottom),
        )
    });
    let (fx, fy) = *grab.anchor.get_or_insert_with(|| {
        (
            ((grab.pointer_start.x - f64::from(origin.x) + left)
                / (f64::from(size.w) + left + right).max(1.0))
            .clamp(0.0, 1.0),
            ((grab.pointer_start.y - f64::from(origin.y) + top)
                / (f64::from(size.h) + top + bottom).max(1.0))
            .clamp(0.0, 1.0),
        )
    });
    if !grab.updated {
        return false;
    }
    grab.updated = false;
    let (dx, dy) = (grab.delta.0.round() as i32, grab.delta.1.round() as i32);
    if grab.edges == 0 {
        if (dx != 0 || dy != 0) && comp.placement(grab.id).constrained() {
            let (restored, anchor, keep_titlebar) =
                if let Some(saved) = comp.float_geometry(grab.id) {
                    (saved.size, (fx, fy), true)
                } else {
                    let Some((_, area)) = target(comp, space, grab.id, window) else {
                        return false;
                    };
                    (
                        decor::window::content_area(window, default_float(area)).size,
                        (0.5, 0.5),
                        false,
                    )
                };
            let (w, h) = (restored.w.max(1), restored.h.max(1));
            let restored = Size::from((w, h));
            let pointer = grab.pointer_start + Point::from(grab.delta);
            // Content origins include the chrome inset. Fractions refer to the frame.
            let anchor_y = anchor.1 * (f64::from(h) + top + bottom);
            let anchor_y =
                if keep_titlebar && top > 0.0 && grab.pointer_start.y < f64::from(origin.y) {
                    anchor_y.min((top - 1.0).max(0.0))
                } else {
                    anchor_y
                };
            let location = Point::from((
                (pointer.x - anchor.0 * (f64::from(w) + left + right) + left).round() as i32,
                (pointer.y - anchor_y + top).round() as i32,
            ));
            comp.set_placement(grab.id, window, Placement::Floating);
            shell::stage(window, restored, false);
            shell::set_maximized(window, false);
            shell::send(window);
            slot::set_expected_size(window, restored);
            origin = location - Point::from((dx, dy));
            grab.start = Some((origin, restored));
        }
        space.map_element(window.clone(), origin + Point::from((dx, dy)), false);
    } else {
        let (top, bottom, left, right) = (
            grab.edges & 1 != 0,
            grab.edges & 2 != 0,
            grab.edges & 4 != 0,
            grab.edges & 8 != 0,
        );
        let wanted = (
            size.w
                + if right {
                    dx
                } else if left {
                    -dx
                } else {
                    0
                },
            size.h
                + if bottom {
                    dy
                } else if top {
                    -dy
                } else {
                    0
                },
        );
        let (w, h) = clamp_size(wanted, size_hints(window));
        let location = Point::from((
            if left {
                origin.x + size.w - w
            } else {
                origin.x
            },
            if top { origin.y + size.h - h } else { origin.y },
        ));
        let size = Size::from((w, h));
        shell::stage(window, size, true);
        shell::send(window);
        slot::set_expected_size(window, size);
        space.map_element(window.clone(), location, false);
        grab.last_size = Some(size);
    }
    true
}

fn apply(
    space: &mut Space<Window>,
    window: &Window,
    area: Rectangle<i32, Logical>,
    maximized: bool,
) {
    shell::stage(window, area.size, false);
    shell::set_maximized(window, maximized);
    shell::send(window);
    slot::set_expected_size(window, area.size);
    space.map_element(window.clone(), area.loc, false);
}
