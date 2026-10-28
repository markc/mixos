// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Timed edge move zones. No corner-detector state is read here.

use comp_model::tiling::TilingConfig;
use std::time::{Duration, Instant};

use smithay::backend::renderer::element::{Id, Kind, solid::SolidColorRenderElement};
use smithay::backend::renderer::utils::CommitCounter;
use smithay::desktop::{Space, Window};
use smithay::utils::{Logical, Physical, Point, Rectangle};

use super::{CompState, FloatGeometry, Placement, TileSlot, geometry};
use crate::state::Loop;

pub const PREVIEW_ALPHA: f32 = 0.25;
const OUTLINE_WIDTH: f64 = 2.0;

#[derive(Clone, Debug, PartialEq)]
pub struct Armed {
    pub placement: Placement,
    /// Exact outer frame in output-layout logical coordinates.
    pub rectangle: Rectangle<i32, Logical>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    pub target: Armed,
    pub shared: bool,
}

#[derive(Clone, Debug)]
struct Pending {
    candidate: Candidate,
    since: Instant,
    from: Rectangle<i32, Logical>,
    follow_frame: bool,
    ready: Instant,
    duration: Duration,
}

#[derive(Clone, Debug)]
struct Animation {
    from: Rectangle<i32, Logical>,
    to: Rectangle<i32, Logical>,
    started: Instant,
    duration: Duration,
}

impl Animation {
    fn rectangle(&self, now: Instant) -> Rectangle<i32, Logical> {
        let t = if self.duration.is_zero() {
            1.0
        } else {
            (now.saturating_duration_since(self.started).as_secs_f64()
                / self.duration.as_secs_f64())
            .min(1.0)
        };
        let ease = 1.0 - (1.0 - t).powi(3);
        let axis = |a, b| (f64::from(a) + f64::from(b - a) * ease).round() as i32;
        Rectangle::new(
            (
                axis(self.from.loc.x, self.to.loc.x),
                axis(self.from.loc.y, self.to.loc.y),
            )
                .into(),
            (
                axis(self.from.size.w, self.to.size.w),
                axis(self.from.size.h, self.to.size.h),
            )
                .into(),
        )
    }
}

#[derive(Clone, Debug)]
pub struct Drag {
    pub placement: Placement,
    pub float_geometry: Option<FloatGeometry>,
    /// Immutable geometry captured before the executor applies any travel.
    pub start: Option<Rectangle<i32, Logical>>,
    pub start_output: Option<String>,
    pub armed: Option<Armed>,
    pending: Option<Pending>,
    animation: Option<Animation>,
    pub preview_rectangle: Option<Rectangle<i32, Logical>>,
}

impl Drag {
    pub fn new(placement: Placement, float_geometry: Option<FloatGeometry>) -> Self {
        Self {
            placement,
            float_geometry,
            start: None,
            start_output: None,
            armed: None,
            pending: None,
            animation: None,
            preview_rectangle: None,
        }
    }

    pub fn disarm(&mut self) {
        self.armed = None;
        self.pending = None;
        self.animation = None;
        self.preview_rectangle = None;
    }

    pub fn sample(
        &mut self,
        candidate: Option<Candidate>,
        frame: Rectangle<i32, Logical>,
        config: TilingConfig,
        now: Instant,
    ) -> bool {
        let before = self.preview_rectangle;
        let Some(candidate) = candidate else {
            self.disarm();
            return before.is_some();
        };
        let current = self.animation.as_ref().map(|a| a.rectangle(now)).unwrap_or(frame);
        if self.pending.as_ref().is_none_or(|p| p.candidate != candidate) {
            let follow_frame = self.animation.is_none();
            self.disarm();
            self.pending = Some(Pending {
                candidate,
                since: now,
                from: current,
                follow_frame,
                ready: now,
                duration: Duration::ZERO,
            });
        }
        let pending = self.pending.as_mut().expect("sampled zone");
        if self.armed.is_none() && pending.follow_frame {
            pending.from = frame;
        }
        pending.ready = pending.since + Duration::from_millis(
            if pending.candidate.shared { config.shared_edge_dwell_ms } else { 0 }
                + config.preview_delay_ms,
        );
        let duration = Duration::from_millis(config.preview_animation_ms);
        if pending.duration != duration
            && let Some(animation) = self.animation.as_mut()
        {
            animation.from = current;
            animation.started = now;
            animation.duration = duration;
        }
        pending.duration = duration;
        self.tick(now);
        before != self.preview_rectangle
    }

    /// A timer advances only the last sampled zone, never pointer or corner state.
    pub fn tick(&mut self, now: Instant) -> bool {
        let before = self.preview_rectangle;
        if let Some(pending) = &self.pending {
            if now < pending.ready {
                self.armed = None;
                self.animation = None;
                self.preview_rectangle = None;
                return before.is_some();
            }
            if self.armed.is_none() {
                self.armed = Some(pending.candidate.target.clone());
                self.animation = Some(Animation {
                    from: pending.from,
                    to: pending.candidate.target.rectangle,
                    started: pending.ready,
                    duration: pending.duration,
                });
            }
        }
        self.preview_rectangle = self.animation.as_ref().map(|a| a.rectangle(now));
        before != self.preview_rectangle
    }

    pub fn next_deadline(&self, now: Instant) -> Option<Instant> {
        let pending = self.pending.as_ref()?;
        if self.armed.is_none() {
            return Some(pending.ready);
        }
        let animation = self.animation.as_ref()?;
        let end = animation.started + animation.duration;
        (now < end).then(|| {
            let step = now.saturating_duration_since(animation.started).as_millis() as u64 / 16 + 1;
            (animation.started + Duration::from_millis(step * 16)).min(end)
        })
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
    config: TilingConfig,
    resizable: bool,
    maximizable: bool,
    shift: bool,
) -> Option<Placement> {
    if !config.edge_tiling || !resizable || shift || area.size.w <= 0 || area.size.h <= 0 {
        return None;
    }
    let (x, y) = (pointer.x, pointer.y);
    let (left, top) = (f64::from(area.loc.x), f64::from(area.loc.y));
    let (width, height) = (f64::from(area.size.w), f64::from(area.size.h));
    let side = if x <= left + config.side_band_px {
        Some(false)
    } else if x >= left + width - config.side_band_px {
        Some(true)
    } else {
        None
    };
    if let Some(right) = side {
        let slot = if y <= top + config.corner_ratio * height {
            if right {
                TileSlot::TopRight
            } else {
                TileSlot::TopLeft
            }
        } else if y >= top + height - config.corner_ratio * height {
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
    } else if y <= top + config.top_band_px && maximizable && config.top_maximize {
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
) -> Option<Candidate> {
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
    let placement = zone(
        local_area,
        local,
        comp.tiling,
        resizable,
        maximizable,
        shift,
    )?
    .on_output(output.name());
    let shared = match &placement {
        Placement::Tiled {
            slot: TileSlot::Left | TileSlot::TopLeft | TileSlot::BottomLeft,
            ..
        } => inner.left,
        Placement::Tiled { .. } => inner.right,
        Placement::Maximized { .. } => inner.top,
        Placement::Floating => false,
    };
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
    Some(Candidate {
        target: Armed {
            placement,
            rectangle,
        },
        shared,
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
    let Some(rectangle) = comp.drag.as_ref().and_then(|d| d.preview_rectangle) else {
        return Vec::new();
    };
    let local = Rectangle::new(rectangle.loc - outer.loc, rectangle.size);
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
                    TilingConfig::default(),
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
                TilingConfig::default(),
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
                    TilingConfig::default(),
                    true,
                    true,
                    false
                ),
                None
            );
        }
    }

    #[test]
    fn shift_and_size_hints_disable_zones() {
        for (resize, max, shift) in [
            (false, true, false),
            (true, true, true),
            (true, false, false),
        ] {
            assert_eq!(
                zone(
                    area(),
                    (500.0, 0.0).into(),
                    TilingConfig::default(),
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
