// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Where every pane goes, as plain arithmetic.
//!
//! The pane tree is the core's (`term_core::panes`), and its split ratios
//! are the only layout truth: the Bus's `term.pane.split` and a keyboard
//! split produce the same tree. This module turns that tree into rectangles
//! in the window, and the rectangles into grid sizes for the PTYs.
//!
//! Two things it adds over the core's own `PaneTree::leaves`, both because
//! the GPU grid draws glyphs texel for texel on device pixels:
//!
//! - **Every pane edge lands on a whole physical pixel.** A pane starting at
//!   x = 450.2 physical would put each glyph half a pixel off the device
//!   grid. The core's ratios are kept; only the edge is snapped.
//! - **Each pane has a border of a whole number of physical pixels** on all
//!   sides, focused or not; only its colour marks focus. So moving focus
//!   never changes a pane's interior, and never reflows its PTY. Its width
//!   and the padding inside it are settings ([`Edges`]); changing either
//!   resizes each PTY once.
//!
//! The view draws from [`panes`] and the PTYs are sized from [`panes`] and
//! [`grid`], so the two cannot disagree.

use term_core::panes::{Geometry, PaneTree, SplitDir};
use term_core::tabs::TabSet;

/// A PTY is not obliged to cope with 10,000 columns, and a grid wider than
/// 4096 physical pixels exceeds the texture size every GPU is guaranteed
/// to support.
pub const MAX_COLS: u16 = 240;
pub const MAX_ROWS: u16 = 100;
pub const MAX_TEXTURE: u32 = 4096;

/// The active tab's pane tree with the terminals taken out: what the view
/// needs, without holding the `TabSet` lock while it draws.
#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    Leaf(u64),
    Split {
        dir: SplitDir,
        ratio: f32,
        first: Box<Node>,
        second: Box<Node>,
    },
}

impl Node {
    pub fn of(tree: &PaneTree) -> Self {
        match tree {
            PaneTree::Leaf(pane) => Self::Leaf(pane.id),
            PaneTree::Split {
                dir,
                ratio,
                first,
                second,
            } => Self::Split {
                dir: *dir,
                ratio: *ratio,
                first: Box::new(Self::of(first)),
                second: Box::new(Self::of(second)),
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct TabLabel {
    pub id: u64,
    pub title: String,
    pub active: bool,
}

/// Everything the view draws that is not pixels.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Shape {
    pub tabs: Vec<TabLabel>,
    /// `None` only when the tab set is empty, i.e. while the app is exiting.
    pub tree: Option<Node>,
    pub active_pane: u64,
}

impl Shape {
    pub fn of(tabs: &TabSet) -> Self {
        if tabs.is_empty() {
            return Self::default();
        }
        let active = tabs.active_tab();
        Self {
            tabs: tabs
                .list()
                .into_iter()
                .map(|tab| TabLabel {
                    id: tab.id,
                    title: tab.title,
                    active: tab.active,
                })
                .collect(),
            tree: Some(Node::of(&active.tree)),
            active_pane: active.active_pane,
        }
    }

    /// Pane ids on screen, in tree order.
    pub fn visible(&self) -> Vec<u64> {
        let mut out = Vec::new();
        if let Some(tree) = &self.tree {
            collect_ids(tree, &mut out);
        }
        out
    }

    /// Whether pane `id` wears the focus ring. It marks which pane keys go
    /// to, so it shows only when there is a choice: a lone pane wears the
    /// plain border, as foot shows nothing at all.
    pub fn focus_ring(&self, id: u64) -> bool {
        id == self.active_pane && self.visible().len() > 1
    }
}

fn collect_ids(node: &Node, out: &mut Vec<u64>) {
    match node {
        Node::Leaf(id) => out.push(*id),
        Node::Split { first, second, .. } => {
            collect_ids(first, out);
            collect_ids(second, out);
        }
    }
}

/// `value` points, moved to the nearest whole physical pixel.
fn snap(value: f32, scale: f32) -> f32 {
    (value * scale).round() / scale
}

/// A pane's edge, from `terminal.border.width` and `terminal.padding`, in
/// whole logical pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Edges {
    /// The border's width.
    pub border: u32,
    /// The space between the pane's outer edge and its text. The border
    /// lies inside it: the text never starts nearer the edge than the
    /// border's inner side.
    pub padding: u32,
}

impl Default for Edges {
    fn default() -> Self {
        Self {
            border: 1,
            padding: 8,
        }
    }
}

/// A pane border of `width` points, rounded to whole physical pixels and
/// never zero.
pub fn border(scale: f32, width: u32) -> f32 {
    (width as f32 * scale).round().max(1.0) / scale
}

/// From a pane's outer edge to its text, in points: the padding in whole
/// physical pixels, and never less than the border.
pub fn inset(scale: f32, edges: Edges) -> f32 {
    let padding = (edges.padding as f32 * scale).round() / scale;
    padding.max(border(scale, edges.border))
}

/// The pane area inside `area` (points), shrunk to whole physical pixels on
/// every side.
pub fn content(area: egui::Rect, scale: f32) -> Geometry {
    let left = (area.min.x * scale).ceil() / scale;
    let top = (area.min.y * scale).ceil() / scale;
    let right = (area.max.x * scale).floor() / scale;
    let bottom = (area.max.y * scale).floor() / scale;
    Geometry {
        x: left,
        y: top,
        w: (right - left).max(0.0),
        h: (bottom - top).max(0.0),
    }
}

/// One split, with the shared edge on a whole physical pixel. The two halves
/// always sum to `bounds` exactly, so nested splits tile it with no gap and
/// no overlap.
pub fn split(dir: SplitDir, ratio: f32, bounds: Geometry, scale: f32) -> (Geometry, Geometry) {
    let mut a = bounds;
    let mut b = bounds;
    match dir {
        SplitDir::Vertical => {
            let edge =
                snap(bounds.x + bounds.w * ratio, scale).clamp(bounds.x, bounds.x + bounds.w);
            a.w = edge - bounds.x;
            b.x = edge;
            b.w = bounds.x + bounds.w - edge;
        }
        SplitDir::Horizontal => {
            let edge =
                snap(bounds.y + bounds.h * ratio, scale).clamp(bounds.y, bounds.y + bounds.h);
            a.h = edge - bounds.y;
            b.y = edge;
            b.h = bounds.y + bounds.h - edge;
        }
    }
    (a, b)
}

/// Every pane's outer rectangle (border included), in tree order.
pub fn panes(node: &Node, bounds: Geometry, scale: f32) -> Vec<(u64, Geometry)> {
    let mut out = Vec::new();
    place(node, bounds, scale, &mut out);
    out
}

fn place(node: &Node, bounds: Geometry, scale: f32, out: &mut Vec<(u64, Geometry)>) {
    match node {
        Node::Leaf(id) => out.push((*id, bounds)),
        Node::Split {
            dir,
            ratio,
            first,
            second,
        } => {
            let (a, b) = split(*dir, *ratio, bounds, scale);
            place(first, a, scale, out);
            place(second, b, scale, out);
        }
    }
}

/// Columns and rows that fit inside a pane's border and padding, for a
/// PHYSICAL cell of `cell` pixels.
///
/// Computed in physical pixels, not by dividing logical sizes: `cell / scale`
/// is rarely exact, and a width of exactly 80 cells must give 80 columns, not
/// 79 because 80 × (19 / 2.5) came out a hair over the width.
pub fn grid(pane: Geometry, cell: (u32, u32), scale: f32, edges: Edges) -> (u16, u16) {
    let (cell_width, cell_height) = (cell.0.max(1), cell.1.max(1));
    let inset = 2.0 * inset(scale, edges);
    let fit = |logical: f32, cell: u32| {
        (((logical - inset).max(0.0) * scale + 0.01) / cell as f32) as u32
    };
    let cols =
        fit(pane.w, cell_width).clamp(2, u32::from(MAX_COLS).min(MAX_TEXTURE / cell_width).max(2));
    let rows = fit(pane.h, cell_height)
        .clamp(1, u32::from(MAX_ROWS).min(MAX_TEXTURE / cell_height).max(1));
    (cols as u16, rows as u16)
}

/// Where pane `pane`'s grid is drawn: inside its border and padding, `grid`
/// cells of `cell` physical pixels, in points. Starts on a whole physical
/// pixel whenever the pane does.
pub fn grid_rect(
    pane: Geometry,
    grid: (u16, u16),
    cell: (u32, u32),
    scale: f32,
    edges: Edges,
) -> egui::Rect {
    let inset = inset(scale, edges);
    egui::Rect::from_min_size(
        egui::pos2(pane.x + inset, pane.y + inset),
        egui::vec2(
            f32::from(grid.0) * cell.0 as f32 / scale,
            f32::from(grid.1) * cell.1 as f32 / scale,
        ),
    )
}

pub fn rect(geometry: Geometry) -> egui::Rect {
    egui::Rect::from_min_size(
        egui::pos2(geometry.x, geometry.y),
        egui::vec2(geometry.w, geometry.h),
    )
}

/// A real PTY-backed tab set with default settings, for tests.
#[cfg(test)]
pub fn test_tabs() -> TabSet {
    TabSet::with_session(
        term_core::config::Settings {
            config: term_core::config::Config::default(),
            term: "xterm-256color".into(),
        },
        None,
    )
    .expect("a Mix PTY (/opt/mixos/bin/mix)")
}

#[cfg(test)]
mod tests {
    use super::*;
    use settings::terminal::PADDINGS;

    fn geometry(x: f32, y: f32, w: f32, h: f32) -> Geometry {
        Geometry { x, y, w, h }
    }

    fn leaf(id: u64) -> Box<Node> {
        Box::new(Node::Leaf(id))
    }

    fn on_pixel_grid(value: f32, scale: f32) -> bool {
        ((value * scale) - (value * scale).round()).abs() < 1e-3
    }

    /// A 2.5x output is where half-pixel edges come from: a third of 900
    /// points does not land on a whole pixel, and neither does a ratio a Bus
    /// caller set.
    #[test]
    fn split_edges_land_on_whole_physical_pixels_and_tile_exactly() {
        let bounds = geometry(0.0, 30.0, 900.0, 530.0);
        for scale in [1.0, 1.25, 1.5, 2.0, 2.5] {
            for ratio in [0.5, 1.0 / 3.0, 0.37, 0.9] {
                for dir in [SplitDir::Vertical, SplitDir::Horizontal] {
                    let (a, b) = split(dir, ratio, bounds, scale);
                    let (edge, far) = match dir {
                        SplitDir::Vertical => (b.x, a.w + b.w),
                        SplitDir::Horizontal => (b.y, a.h + b.h),
                    };
                    assert!(
                        on_pixel_grid(edge, scale),
                        "{dir:?} {ratio} @{scale}: edge {edge}"
                    );
                    let whole = match dir {
                        SplitDir::Vertical => bounds.w,
                        SplitDir::Horizontal => bounds.h,
                    };
                    assert!((far - whole).abs() < 1e-3, "halves must sum to the whole");
                }
            }
        }
    }

    /// The pane area starts and ends on whole physical pixels whatever the
    /// chrome above it measured.
    #[test]
    fn the_content_area_is_on_the_pixel_grid() {
        for scale in [1.0, 1.25, 1.5, 2.5] {
            let area = egui::Rect::from_min_max(egui::pos2(0.3, 57.13), egui::pos2(900.0, 560.7));
            let c = content(area, scale);
            for edge in [c.x, c.y, c.x + c.w, c.y + c.h] {
                assert!(on_pixel_grid(edge, scale), "{edge} @{scale}");
            }
            assert!(c.x >= area.min.x && c.y >= area.min.y);
            assert!(c.x + c.w <= area.max.x + 1e-4 && c.y + c.h <= area.max.y + 1e-4);
            let pane = panes(&Node::Leaf(1), c, scale)[0].1;
            for padding in PADDINGS {
                let edges = Edges { border: 1, padding };
                let r = grid_rect(pane, (10, 3), (19, 40), scale, edges);
                assert!(on_pixel_grid(r.min.x, scale) && on_pixel_grid(r.min.y, scale));
                assert!((r.width() * scale - 190.0).abs() < 1e-3);
            }
        }
    }

    /// The tree the Bus builds (split, then split the right half) must come
    /// out as three panes that cover the content area with nothing left over.
    #[test]
    fn nested_splits_cover_the_content_area() {
        let tree = Node::Split {
            dir: SplitDir::Vertical,
            ratio: 0.5,
            first: leaf(1),
            second: Box::new(Node::Split {
                dir: SplitDir::Horizontal,
                ratio: 0.5,
                first: leaf(2),
                second: leaf(3),
            }),
        };
        let bounds = content(
            egui::Rect::from_min_max(egui::pos2(0.0, 30.0), egui::pos2(900.0, 560.0)),
            1.0,
        );
        assert_eq!(bounds, geometry(0.0, 30.0, 900.0, 530.0));
        let placed = panes(&tree, bounds, 1.0);
        assert_eq!(
            placed,
            vec![
                (1, geometry(0.0, 30.0, 450.0, 530.0)),
                (2, geometry(450.0, 30.0, 450.0, 265.0)),
                (3, geometry(450.0, 295.0, 450.0, 265.0)),
            ]
        );
        let area: f32 = placed.iter().map(|(_, g)| g.w * g.h).sum();
        assert_eq!(area, bounds.w * bounds.h);
    }

    #[test]
    fn the_grid_fits_inside_the_border_in_physical_pixels() {
        let edges = Edges::default();
        assert_eq!(
            edges.padding,
            settings::terminal::Terminal::default().padding
        );
        // 8x16 cells at scale 1 with 8 px padding: 816 wide holds exactly
        // 100 columns, 815 holds 99. The border stays 1 px.
        assert_eq!(edges.border, 1);
        assert_eq!(
            grid(geometry(0.0, 0.0, 816.0, 496.0), (8, 16), 1.0, edges),
            (100, 30)
        );
        assert_eq!(
            grid(geometry(0.0, 0.0, 815.0, 495.0), (8, 16), 1.0, edges),
            (99, 29)
        );
        // 2.5x: padding is 20 physical; 20x40 physical cells.
        // (400 - 16) * 2.5 = 960 -> 48 columns.
        assert_eq!(
            grid(geometry(0.0, 0.0, 400.0, 200.0), (20, 40), 2.5, edges),
            (48, 11)
        );
        // A pane too small for anything still gets a legal PTY size.
        assert_eq!(
            grid(geometry(0.0, 0.0, 3.0, 3.0), (8, 16), 1.0, edges),
            (2, 1)
        );
        // And a huge one is held under the texture limit.
        assert_eq!(
            grid(geometry(0.0, 0.0, 9000.0, 9000.0), (8, 16), 1.0, edges),
            (240, 100)
        );
        assert_eq!(
            grid(geometry(0.0, 0.0, 9000.0, 9000.0), (40, 80), 1.0, edges),
            (102, 51)
        );
    }

    /// Padding is measured from the pane's outer edge; the border lies
    /// inside it, so the text is never nearer the edge than the border.
    #[test]
    fn padding_and_border_shrink_the_grid_in_whole_physical_pixels() {
        let pane = geometry(0.0, 0.0, 832.0, 512.0);
        let fit = |border, padding| grid(pane, (8, 16), 1.0, Edges { border, padding });
        assert_eq!(fit(1, 1), (103, 31));
        // 16 each side: 800 x 480 inside.
        assert_eq!(fit(1, 16), (100, 30));
        // A border wider than the padding sets the inset.
        assert_eq!(fit(4, 1), fit(4, 4));
        assert_eq!(fit(4, 2), (103, 31));
        assert_eq!(fit(1, 8), (102, 31));
        for scale in [1.0, 1.25, 1.5, 2.5] {
            for padding in PADDINGS {
                for width in 1..=4 {
                    let inset = inset(
                        scale,
                        Edges {
                            border: width,
                            padding,
                        },
                    );
                    assert!(on_pixel_grid(inset, scale), "{inset} @{scale}");
                    assert!(inset >= border(scale, width) - 1e-6);
                    assert!(inset * scale >= (padding as f32 * scale).round() - 1e-3);
                }
            }
        }
    }

    #[test]
    fn a_border_is_whole_physical_pixels_and_never_zero() {
        for scale in [0.5, 1.0, 1.25, 1.5, 2.0, 2.5, 3.0] {
            for width in 1..=4 {
                let border = border(scale, width);
                assert!(border * scale >= 1.0 - 1e-6);
                assert!(on_pixel_grid(border, scale), "border {border} @{scale}");
            }
            assert_eq!(
                border(scale, 1),
                scale.round().max(1.0) / scale,
                "as before"
            );
        }
    }

    #[test]
    fn the_focus_ring_shows_only_when_there_is_more_than_one_pane() {
        let lone = Shape {
            tabs: Vec::new(),
            tree: Some(Node::Leaf(7)),
            active_pane: 7,
        };
        assert!(!lone.focus_ring(7));
        let split = Shape {
            tree: Some(Node::Split {
                dir: SplitDir::Vertical,
                ratio: 0.5,
                first: leaf(7),
                second: leaf(8),
            }),
            ..lone
        };
        assert!(split.focus_ring(7));
        assert!(!split.focus_ring(8));
    }

    /// The shape the view draws comes from the real tab set, so a Bus
    /// `term.tab.new` or `term.pane.split` shows up with no keyboard involved.
    #[test]
    fn the_shape_follows_the_tab_set() {
        let mut tabs = test_tabs();
        let first = Shape::of(&tabs);
        assert_eq!(first.tabs.len(), 1);
        assert!(first.tabs[0].active);
        assert_eq!(first.visible(), vec![first.active_pane]);

        let split = tabs.split_active(SplitDir::Vertical).expect("split");
        let shape = Shape::of(&tabs);
        assert_eq!(shape.visible(), vec![first.active_pane, split]);
        assert_eq!(shape.active_pane, split, "a split focuses the new pane");

        let second = tabs.open().expect("new tab");
        let shape = Shape::of(&tabs);
        assert_eq!(shape.tabs.len(), 2);
        assert!(shape.tabs.iter().any(|tab| tab.id == second && tab.active));
        assert_eq!(shape.visible().len(), 1, "only the active tab's panes");
        assert!(!shape.visible().contains(&split));

        let removed = tabs.shutdown();
        assert_eq!(Shape::of(&tabs), Shape::default());
        drop(removed);
    }
}
