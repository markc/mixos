// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Immediate outer-edge move zones. No corner-detector state is read here.

use smithay::backend::renderer::element::{Id, Kind, solid::SolidColorRenderElement};
use smithay::backend::renderer::utils::CommitCounter;
use smithay::desktop::{Space, Window};
use smithay::utils::{Logical, Physical, Point, Rectangle};

use super::{CompState, FloatGeometry, Placement, TileSlot, geometry};
use crate::state::Loop;

pub const SIDE_BAND: f64 = 20.0;
pub const TOP_BAND: f64 = 5.0;
pub const CORNER_RATIO: f64 = 0.25;
pub const PREVIEW_ALPHA: f32 = 0.25;
const OUTLINE_WIDTH: f64 = 2.0;

#[derive(Clone, Debug, PartialEq)]
pub struct Armed {
    pub placement: Placement,
    /// Exact outer frame in output-layout logical coordinates.
    pub rectangle: Rectangle<i32, Logical>,
}

#[derive(Clone, Debug)]
pub struct Drag {
    pub placement: Placement,
    pub float_geometry: Option<FloatGeometry>,
    /// Immutable geometry captured before the executor applies any travel.
    pub start: Option<Rectangle<i32, Logical>>,
    pub start_output: Option<String>,
    pub armed: Option<Armed>,
}

impl Drag {
    pub fn new(placement: Placement, float_geometry: Option<FloatGeometry>) -> Self {
        Self {
            placement,
            float_geometry,
            start: None,
            start_output: None,
            armed: None,
        }
    }
}

/// The sides whose physical output edges meet another output. Usable-area
/// insets do not turn a shared monitor edge into an outer edge.
#[derive(Clone, Copy, Debug, Default)]
pub struct InnerEdges {
    pub left: bool,
    pub right: bool,
    pub top: bool,
}

impl InnerEdges {
    pub fn between(
        output: Rectangle<i32, Logical>,
        others: impl IntoIterator<Item = Rectangle<i32, Logical>>,
    ) -> Self {
        let mut edges = Self::default();
        let (right, bottom) = (output.loc.x + output.size.w, output.loc.y + output.size.h);
        for other in others {
            let (other_right, other_bottom) =
                (other.loc.x + other.size.w, other.loc.y + other.size.h);
            let vertical = output.loc.y < other_bottom && other.loc.y < bottom;
            let horizontal = output.loc.x < other_right && other.loc.x < right;
            edges.left |= vertical && other_right == output.loc.x;
            edges.right |= vertical && other.loc.x == right;
            edges.top |= horizontal && other_bottom == output.loc.y;
        }
        edges
    }
}

/// Coordinates and thresholds are output-local logical units. Deliberately
/// no lower/upper clipping: a pointer over a reserved panel still arms.
pub fn zone(
    area: Rectangle<i32, Logical>,
    pointer: Point<f64, Logical>,
    inner: InnerEdges,
    resizable: bool,
    maximizable: bool,
    shift: bool,
) -> Option<Placement> {
    if !resizable || shift || area.size.w <= 0 || area.size.h <= 0 {
        return None;
    }
    let (x, y) = (pointer.x, pointer.y);
    let (left, top) = (f64::from(area.loc.x), f64::from(area.loc.y));
    let (width, height) = (f64::from(area.size.w), f64::from(area.size.h));
    let side = if x <= left + SIDE_BAND {
        if inner.left {
            return None;
        }
        Some(false)
    } else if x >= left + width - SIDE_BAND {
        if inner.right {
            return None;
        }
        Some(true)
    } else {
        None
    };
    if let Some(right) = side {
        let slot = if y <= top + CORNER_RATIO * height {
            if right {
                TileSlot::TopRight
            } else {
                TileSlot::TopLeft
            }
        } else if y >= top + height - CORNER_RATIO * height {
            if right {
                TileSlot::BottomRight
            } else {
                TileSlot::BottomLeft
            }
        } else if right {
            TileSlot::Right
        } else {
            TileSlot::Left
        };
        Some(Placement::Tiled {
            slot,
            output: String::new(),
        })
    } else if y <= top + TOP_BAND && maximizable && !inner.top {
        Some(Placement::Maximized {
            output: String::new(),
        })
    } else {
        None
    }
}

pub fn armed(
    comp: &CompState,
    space: &Space<Window>,
    window: &Window,
    id: surfaces::SurfaceId,
    pointer: Point<f64, Logical>,
    shift: bool,
) -> Option<Armed> {
    let output = space.outputs().find(|output| {
        space
            .output_geometry(output)
            .is_some_and(|rect| rect.to_f64().contains(pointer))
    })?;
    let outer = space.output_geometry(output)?;
    let area = geometry::usable_area(
        output,
        outer,
        comp.reserved
            .get(&output.name())
            .copied()
            .unwrap_or_default(),
    );
    let inner = InnerEdges::between(
        outer,
        space
            .outputs()
            .filter(|other| *other != output)
            .filter_map(|other| space.output_geometry(other)),
    );
    let (min, max) = geometry::size_hints(window);
    let resizable = !((max.0 > 0 && min.0 >= max.0) || (max.1 > 0 && min.1 >= max.1));
    let content = decor::window::content_area(window, area);
    let maximizable =
        (max.0 <= 0 || max.0 >= content.size.w) && (max.1 <= 0 || max.1 >= content.size.h);
    let local = pointer - outer.loc.to_f64();
    let local_area = Rectangle::new(area.loc - outer.loc, area.size);
    let placement =
        zone(local_area, local, inner, resizable, maximizable, shift)?.on_output(output.name());
    let rectangle = match &placement {
        Placement::Tiled { slot, output } => {
            let workspace = comp
                .registry
                .get(id)
                .and_then(|r| r.workspace())
                .unwrap_or_else(|| comp.current_workspace());
            slot.rectangle(area, comp.tile_split_for(output, workspace))
        }
        _ => area,
    };
    Some(Armed {
        placement,
        rectangle,
    })
}

/// xkb has already processed this key; both its edges are intercepted ahead of
/// shortcuts and client forwarding. Only a live host move may claim Escape.
pub fn key(
    comp: &mut CompState,
    dispatch: &mut dispatcher::state::Dispatch,
    code: u32,
    pressed: bool,
    sym: Option<u32>,
    time: u32,
) -> bool {
    if comp.drag_escape == Some(code) {
        if !pressed {
            comp.drag_escape = None;
        }
        comp.bindings.last_edge = Some(super::bindings::KeyEdge {
            took: true,
            modifier: false,
        });
        return true;
    }
    if pressed
        && sym == Some(smithay::input::keyboard::keysyms::KEY_Escape)
        && comp
            .interactive
            .is_some_and(|grab| grab.edges == 0 && !grab.ended)
        && dispatcher::wayland::grab::interactive::cancel(dispatch, time)
    {
        comp.drag_escape = Some(code);
        comp.bindings.last_edge = Some(super::bindings::KeyEdge {
            took: true,
            modifier: false,
        });
        return true;
    }
    false
}

thread_local! {
    static PREVIEW_IDS: [Id; 5] = std::array::from_fn(|_| Id::new());
}

/// Stable elements, frontmost first: opaque focus-ring outline, translucent
/// focus-ring fill. The caller supplies output-local physical geometry.
pub fn elements(area: Rectangle<i32, Physical>, scale: f64) -> Vec<SolidColorRenderElement> {
    if area.size.w <= 0 || area.size.h <= 0 {
        return Vec::new();
    }
    let line = ((OUTLINE_WIDTH * scale).round() as i32)
        .max(1)
        .min(area.size.w)
        .min(area.size.h);
    let (x, y, w, h) = (area.loc.x, area.loc.y, area.size.w, area.size.h);
    let rects = [
        (x, y, w, line),
        (x, y + h - line, w, line),
        (x, y, line, h),
        (x + w - line, y, line, h),
        (x, y, w, h),
    ];
    let ring = super::region::focus_ring();
    PREVIEW_IDS.with(|ids| {
        rects
            .into_iter()
            .enumerate()
            .map(|(i, (x, y, w, h))| {
                let alpha = if i == 4 { PREVIEW_ALPHA } else { 1.0 };
                let colour = [ring[0] * alpha, ring[1] * alpha, ring[2] * alpha, alpha];
                SolidColorRenderElement::new(
                    ids[i].clone(),
                    Rectangle::new((x, y).into(), (w, h).into()),
                    CommitCounter::default(),
                    colour,
                    Kind::Unspecified,
                )
            })
            .collect()
    })
}

pub fn overlay(lp: &Loop) -> Vec<SolidColorRenderElement> {
    let Some(armed) = lp
        .inner
        .comp
        .drag
        .as_ref()
        .and_then(|drag| drag.armed.as_ref())
    else {
        return Vec::new();
    };
    let Some(name) = armed.placement.output() else {
        return Vec::new();
    };
    if !super::region::drawn_on(lp, name) {
        return Vec::new();
    }
    preview(&lp.inner.comp, &lp.inner.space_state().state, name)
}

/// Renderer-free lowering shared with the protocol fixtures.
pub fn preview(
    comp: &CompState,
    space: &Space<Window>,
    name: &str,
) -> Vec<SolidColorRenderElement> {
    let Some(armed) = comp.drag.as_ref().and_then(|drag| drag.armed.as_ref()) else {
        return Vec::new();
    };
    if armed.placement.output() != Some(name) {
        return Vec::new();
    }
    let Some(output) = space.outputs().find(|output| output.name() == name) else {
        return Vec::new();
    };
    let Some(outer) = space.output_geometry(output) else {
        return Vec::new();
    };
    let scale = output.current_scale().fractional_scale();
    let local = Rectangle::new(armed.rectangle.loc - outer.loc, armed.rectangle.size);
    elements(local.to_physical_precise_round(scale), scale)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn area() -> Rectangle<i32, Logical> {
        Rectangle::new((40, 30).into(), (1000, 800).into())
    }

    #[test]
    fn inclusive_thresholds_and_side_precedence() {
        for (point, slot) in [
            ((60.0, 430.0), TileSlot::Left),
            ((1020.0, 430.0), TileSlot::Right),
            ((40.0, 230.0), TileSlot::TopLeft),
            ((1040.0, 230.0), TileSlot::TopRight),
            ((40.0, 630.0), TileSlot::BottomLeft),
            ((1040.0, 630.0), TileSlot::BottomRight),
            ((0.0, 0.0), TileSlot::TopLeft),
            ((0.0, 430.0), TileSlot::Left),
        ] {
            assert_eq!(
                zone(
                    area(),
                    point.into(),
                    InnerEdges::default(),
                    true,
                    true,
                    false
                ),
                Some(Placement::Tiled {
                    slot,
                    output: String::new()
                })
            );
        }
        assert!(matches!(
            zone(
                area(),
                (500.0, 35.0).into(),
                InnerEdges::default(),
                true,
                true,
                false
            ),
            Some(Placement::Maximized { .. })
        ));
        for point in [
            (60.01, 430.0),
            (1019.99, 430.0),
            (500.0, 35.01),
            (500.0, 830.0),
        ] {
            assert_eq!(
                zone(
                    area(),
                    point.into(),
                    InnerEdges::default(),
                    true,
                    true,
                    false
                ),
                None
            );
        }
    }

    #[test]
    fn blocked_bands_do_not_fall_through_to_maximise() {
        let inner = InnerEdges {
            left: true,
            right: true,
            top: true,
        };
        for point in [(0.0, 0.0), (1040.0, 0.0), (500.0, 0.0)] {
            assert_eq!(zone(area(), point.into(), inner, true, true, false), None);
        }
        for (resize, max, shift) in [
            (false, true, false),
            (true, true, true),
            (true, false, false),
        ] {
            assert_eq!(
                zone(
                    area(),
                    (500.0, 0.0).into(),
                    InnerEdges::default(),
                    resize,
                    max,
                    shift
                ),
                None
            );
        }
    }

    #[test]
    fn adjacent_edges_use_output_geometry_and_positive_overlap() {
        let output = Rectangle::new((0, 0).into(), (1000, 800).into());
        let inner = InnerEdges::between(
            output,
            [
                Rectangle::new((-600, 100).into(), (600, 300).into()),
                Rectangle::new((1000, 800).into(), (600, 300).into()), // corner contact only
                Rectangle::new((100, -600).into(), (600, 600).into()),
            ],
        );
        assert!(inner.left && inner.top && !inner.right);
    }
}
