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

/// Internal placement transition; tile commands are deliberately not exposed yet.
pub fn set_placement(
    comp: &mut CompState,
    space: &mut Space<Window>,
    id: SurfaceId,
    window: &Window,
    placement: Placement,
) -> GeometryChange {
    let previous = comp.placement(id);
    if placement.constrained() && previous == Placement::Floating {
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
        return GeometryChange::default();
    }
    let area = if comp.placement(id) == Placement::Floating {
        if !previous.constrained() {
            shell::set_maximized(window, false);
            shell::send(window);
            return GeometryChange::default();
        }
        comp.float_geometry(id)
            .map(|saved| Rectangle::new(saved.location, saved.size))
            .or_else(|| {
                target(comp, space, id, window)
                    .map(|(_, area)| decor::window::content_area(window, default_float(area)))
            })
    } else {
        placement_area(comp, space, id, window)
    };
    let Some(area) = area else {
        return GeometryChange::default();
    };
    apply(space, window, area, comp.placement(id).maximized());
    GeometryChange {
        usable: false,
        windows: true,
    }
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

/// Apply the host's coalesced delta. The dispatcher's pointer start stays immutable.
/// Restore and movement share one map operation, before a frame can be drawn.
pub fn apply_interactive(
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
