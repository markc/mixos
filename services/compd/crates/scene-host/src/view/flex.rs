// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Mix Scenes laid out as flexbox with taffy, the engine Quoin lays its
//! scenes out with. Every drawn node gets one rectangle here; the painter
//! draws and hit-tests at exactly that rectangle and `shell.scene.layout`
//! reports it, so what is measured is what is drawn.
//!
//! The mapping from scene ports to flexbox, written down because CSS
//! defaults alone are not Quoin's:
//!
//! - `row` / `column` are flex containers along x / y. `gap` is the main-axis
//!   gap (`column_gap` in a row, `row_gap` in a column, overriding it);
//!   `padding` and `padding_*` pad; `justify` is justify-content
//!   (`between`/`around`/`evenly`, with their `space_*` spellings); `align` is
//!   align-items and defaults to `start`, not CSS's `stretch`.
//! - `fill`, `grow: n` and an unsized `spacer` grow along the parent's main
//!   axis from a zero basis (`grow` is the share). A `width`/`height` of
//!   `"fill"` grows on the main axis and stretches on the cross axis.
//! - Numbers are fixed sizes; `min_*`/`max_*` bound them, and equal bounds
//!   fix the size. `square` (or a width of `"height"`/`"square"`) makes the
//!   width the height. A fixed-height row centring its content without a
//!   width is at least square, so a compact cell keeps a square hit target.
//! - A fixed-height container keeps its children's intrinsic height and
//!   gives up vertical padding to fit, as Quoin does.
//! - Leaves are measured: `text` with the installed fonts (one line; `elide`
//!   lets it shrink below its width, the painter elides the middle),
//!   `button` as its label plus padding, `field` as a line plus padding and
//!   160 wide unless sized, `toggle` as a switch plus its label, `image` at
//!   its `w` x `h` (16 by default), a sized `spacer` at its size.
//! - A vertical `list` is a column of `row_height` cells, `max_rows` (8)
//!   tall unless sized, scrolling past that; a horizontal list is a row of
//!   cells that shrink when the row is crowded (an `elide` text in them
//!   gives up its width), each down to its instance's floor: an explicit
//!   `min_width` or a fixed width keeps its width. Each cell holds one
//!   instance of the row template.
//! - Hidden nodes and their subtrees are left out (reported hidden, zero
//!   size). The panel's recovered glyphs are the one exception (see
//!   `crate::images::panel_fallback`).

use std::collections::BTreeMap;

use egui::{Pos2, Rect, Vec2, pos2, vec2};
// taffy's prelude also names a Rect; ours is egui's.
use scene::Node;
use taffy::prelude::{
    AlignItems, AlignSelf, Dimension, Display, FlexDirection, JustifyContent, LengthPercentageAuto,
    NodeId, Size, Style, TaffyTree, length, zero,
};
use taffy::{AvailableSpace, LayoutInput, LayoutOutput, Overflow};

use super::{Content, Nodes, TITLEBAR, flag, number};
use crate::templates::{children, rows, text as port_text};

/// Text metrics a layout is measured with: the scene's ui and mono styles.
#[derive(Clone, Copy, Debug)]
pub(crate) struct TextStyles {
    pub ui: TextStyle,
    pub mono: TextStyle,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct TextStyle {
    pub size: f32,
    pub line_height: Option<f32>,
}

impl TextStyle {
    pub fn line(&self, size: f32) -> f32 {
        self.line_height.unwrap_or(size * 1.2)
    }
}

/// Measures one line of text: `(text, size, mono, bold) -> (width, height)`.
pub(crate) type Measurer<'a> = &'a dyn Fn(&str, f32, bool, bool) -> Vec2;

/// Where a node's ports live: the document, or one list row's instance.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Source {
    Tree,
    Row { list: String, item: String },
}

/// One drawn node.
#[derive(Clone, Debug)]
pub(crate) struct Placed {
    /// The instance key local state (edits, toggles) is kept under.
    pub key: String,
    pub id: String,
    pub source: Source,
    pub rect: Rect,
    /// Set for the cell wrapping one list row (the row's click target).
    pub cell: bool,
    /// The list this node is drawn inside, for its scroll offset.
    pub in_list: Option<String>,
}

/// A scrolling vertical list: its box and the height of what it holds.
#[derive(Clone, Debug)]
pub(crate) struct ListBox {
    pub rect: Rect,
    pub content_height: f32,
}

/// A scene laid out on a surface.
#[derive(Clone, Debug, Default)]
pub(crate) struct SceneLayout {
    /// Drawn nodes in paint order (parents before children).
    pub placed: Vec<Placed>,
    /// Document nodes (not list instances) by id.
    pub nodes: BTreeMap<String, Rect>,
    /// List row cells: list id -> item id -> cell.
    pub instances: BTreeMap<String, BTreeMap<String, Rect>>,
    /// Vertical lists that scroll.
    pub lists: BTreeMap<String, ListBox>,
    /// The dialog frame's close button, when framed.
    pub close: Option<Rect>,
}

#[derive(Clone, Copy, PartialEq)]
enum Axis {
    Vertical,
    Horizontal,
}

#[derive(Clone, Copy)]
struct Parent {
    main: Axis,
}

/// What a leaf measures as.
#[derive(Clone, Debug)]
enum Leaf {
    Text {
        text: String,
        size: f32,
        mono: bool,
        bold: bool,
        elide: bool,
        wrap: bool,
        max_lines: usize,
    },
    Button {
        label: String,
    },
    Field,
    Toggle {
        label: String,
    },
    Fixed,
}

#[derive(Clone, Debug)]
struct Meta {
    key: String,
    id: String,
    source: Source,
    cell: bool,
    list_inner: Option<String>,
    leaf: Option<Leaf>,
}

struct Origin<'a> {
    source: &'a Source,
    key: &'a str,
    list: Option<&'a str>,
}

struct Builder<'a> {
    content: &'a Content,
    styles: TextStyles,
    images: &'a crate::images::Prepared,
    tree: TaffyTree<Meta>,
}

fn len(v: f32) -> Dimension {
    Dimension::length(v)
}

fn lpa(v: f32) -> LengthPercentageAuto {
    LengthPercentageAuto::length(v)
}

/// How narrow a row instance in a crowded horizontal list may get: its
/// explicit minimum, else its fixed width (equal `min_`/`max_` bounds, a
/// number, or square on a fixed height, as `sizing` fixes them),
/// else the square minimum of a fixed-height centred row, else nothing.
fn width_floor(node: &Node) -> f32 {
    let height = number(node, "height");
    let square = matches!(port_text(node, "width"), "height" | "square") || flag(node, "square");
    let elides = node.family == "text" && (flag(node, "elide") || flag(node, "wrap"));
    if let Some(min) = number(node, "min_width") {
        // An explicit minimum wins over a maximum, as in flexbox.
        return min;
    }
    let fixed = if elides {
        // An eliding text may always shrink below its width.
        0.0
    } else if let Some(width) = number(node, "width") {
        width
    } else if let (true, Some(h)) = (square, height) {
        h
    } else if let (true, Some(h)) = (
        node.family == "row" && port_text(node, "justify") == "center",
        height,
    ) {
        h
    } else {
        0.0
    };
    // Never above what the node's maximum lets it be.
    number(node, "max_width").map_or(fixed, |max| fixed.min(max))
}

impl<'a> Builder<'a> {
    fn text_style(&self, mono: bool) -> TextStyle {
        if mono {
            self.styles.mono
        } else {
            self.styles.ui
        }
    }

    /// The intrinsic height of a subtree, for the fixed-height padding rule.
    fn intrinsic_height(&self, nodes: Nodes<'_>, id: &str, depth: usize) -> Option<f32> {
        if depth > 64 {
            return None;
        }
        let node = nodes.get(id)?;
        if flag(node, "hidden") {
            return None;
        }
        if let Some(height) = number(node, "height") {
            return Some(height);
        }
        Some(match node.family.as_str() {
            "text" => {
                let style = self.text_style(flag(node, "mono"));
                style.line(number(node, "size").unwrap_or(style.size))
            }
            "image" => number(node, "h").unwrap_or(16.0),
            "row" => children(node)
                .filter_map(|id| self.intrinsic_height(nodes, id, depth + 1))
                .fold(0.0_f32, f32::max),
            "column" => {
                let heights: Vec<_> = children(node)
                    .filter_map(|id| self.intrinsic_height(nodes, id, depth + 1))
                    .collect();
                heights.iter().sum::<f32>()
                    + heights.len().saturating_sub(1) as f32 * number(node, "gap").unwrap_or(0.0)
            }
            _ => 0.0,
        })
    }

    /// The node's own box: size, growth and bounds, against its parent.
    fn sizing(node: &Node, parent: Parent) -> Style {
        let mut style = Style::default();
        // Auto min-content widths propagate through nested flex containers.
        // A scene is bounded by its surface, even with start alignment.
        if matches!(node.family.as_str(), "row" | "column") {
            style.min_size.width = lpa(0.0);
            style.max_size.width = LengthPercentageAuto::percent(1.0);
        }
        let fill =
            flag(node, "fill") || (node.family == "spacer" && number(node, "size").is_none());
        let grow = number(node, "grow").filter(|g| *g > 0.0);
        if fill || grow.is_some() {
            style.flex_grow = grow.unwrap_or(1.0);
            style.flex_basis = len(0.0);
        }
        // The schema's remaining flex-item ports, as CSS has them.
        if let Some(shrink) = number(node, "shrink").filter(|s| *s >= 0.0) {
            style.flex_shrink = shrink;
        }
        if let Some(basis) = number(node, "basis") {
            style.flex_basis = len(basis);
        }
        style.align_self = match port_text(node, "align_self") {
            "start" => Some(AlignSelf::FLEX_START),
            "center" => Some(AlignSelf::CENTER),
            "end" => Some(AlignSelf::FLEX_END),
            "stretch" => Some(AlignSelf::STRETCH),
            _ => style.align_self,
        };
        if let Some(w) = number(node, "width") {
            style.size.width = len(w);
        }
        if let Some(h) = number(node, "height") {
            style.size.height = len(h);
            // Compact centred cells (pager, chips) have at least a square hit
            // target; a shrink row would otherwise fit only its narrow digit.
            if node.family == "row"
                && port_text(node, "justify") == "center"
                && !node.ports.contains_key("width")
            {
                style.min_size.width = lpa(h);
            }
        }
        for (port, horizontal) in [("width", true), ("height", false)] {
            let on_main = (parent.main == Axis::Horizontal) == horizontal;
            if port_text(node, port) == "fill" {
                if on_main {
                    style.flex_grow = style.flex_grow.max(1.0);
                    style.flex_basis = len(0.0);
                } else {
                    style.align_self = Some(AlignSelf::STRETCH);
                }
            }
            let min = number(node, &format!("min_{port}"));
            let max = number(node, &format!("max_{port}"));
            let (size, min_size, max_size) = if horizontal {
                (
                    &mut style.size.width,
                    &mut style.min_size.width,
                    &mut style.max_size.width,
                )
            } else {
                (
                    &mut style.size.height,
                    &mut style.min_size.height,
                    &mut style.max_size.height,
                )
            };
            match (min, max) {
                (Some(min), Some(max)) if min == max => *size = len(min),
                _ => {
                    if let Some(min) = min {
                        *min_size = lpa(min);
                    }
                    if let Some(max) = max {
                        *max_size = lpa(max);
                    }
                }
            }
        }
        if (matches!(port_text(node, "width"), "height" | "square") || flag(node, "square"))
            && let Some(h) = number(node, "height")
        {
            style.size.width = len(h);
        }
        match node.family.as_str() {
            "spacer" => {
                if let Some(size) = number(node, "size") {
                    style.size = Size {
                        width: len(size),
                        height: len(size),
                    };
                }
            }
            "image" => {
                style.size = Size {
                    width: len(number(node, "w").unwrap_or(16.0)),
                    height: len(number(node, "h").unwrap_or(16.0)),
                };
            }
            "field" if !node.ports.contains_key("width") && !flag(node, "fill") => {
                style.size.width = len(160.0);
            }
            "text" if flag(node, "elide") || flag(node, "wrap") => {
                // May shrink below its text; the painter elides the middle.
                style.min_size.width = lpa(0.0);
                style.max_size.width = LengthPercentageAuto::percent(1.0);
                if flag(node, "wrap") {
                    // Fill in a column means horizontal space, never a zero
                    // vertical basis: the measured lines own their height.
                    if parent.main == Axis::Vertical && flag(node, "fill") {
                        style.flex_grow = 0.0;
                        style.flex_basis = Dimension::auto();
                        style.align_self = Some(AlignSelf::STRETCH);
                    }
                    if parent.main == Axis::Vertical {
                        style.flex_shrink = 0.0;
                    }
                }
            }
            _ => {}
        }
        style
    }

    fn container_style(
        &self,
        nodes: Nodes<'_>,
        node: &Node,
        mut style: Style,
        main: Axis,
    ) -> Style {
        style.display = Display::Flex;
        style.flex_direction = match main {
            Axis::Horizontal => FlexDirection::Row,
            Axis::Vertical => FlexDirection::Column,
        };
        let gap = number(node, "gap").unwrap_or(0.0);
        style.gap = match main {
            Axis::Vertical => Size {
                width: zero(),
                height: length(number(node, "row_gap").unwrap_or(gap)),
            },
            Axis::Horizontal => Size {
                width: length(number(node, "column_gap").unwrap_or(gap)),
                height: zero(),
            },
        };
        let all = number(node, "padding").unwrap_or(0.0);
        let mut top = number(node, "padding_top").unwrap_or(all);
        let mut bottom = number(node, "padding_bottom").unwrap_or(all);
        // A fixed height keeps its children's intrinsic height; the
        // vertical padding gives way.
        if let Some(height) = number(node, "height") {
            let content = children(node)
                .filter_map(|id| self.intrinsic_height(nodes, id, 0))
                .fold(0.0_f32, f32::max);
            let available = (height - content).max(0.0);
            let vertical = top + bottom;
            if vertical > 0.0 {
                let ratio = (available / vertical).min(1.0);
                top *= ratio;
                bottom *= ratio;
            }
        }
        style.padding = taffy::Rect {
            left: length(number(node, "padding_left").unwrap_or(all)),
            right: length(number(node, "padding_right").unwrap_or(all)),
            top: length(top),
            bottom: length(bottom),
        };
        style.align_items = Some(match port_text(node, "align") {
            "center" => AlignItems::CENTER,
            "end" => AlignItems::FLEX_END,
            "stretch" => AlignItems::STRETCH,
            _ => AlignItems::FLEX_START,
        });
        style.justify_content = Some(match port_text(node, "justify") {
            "center" => JustifyContent::CENTER,
            "end" => JustifyContent::FLEX_END,
            "between" | "space_between" => JustifyContent::SPACE_BETWEEN,
            "around" | "space_around" => JustifyContent::SPACE_AROUND,
            "evenly" | "space_evenly" => JustifyContent::SPACE_EVENLY,
            _ => JustifyContent::FLEX_START,
        });
        style
    }

    /// Whether `id` is drawn: not hidden, or a recovered panel glyph.
    fn drawn(&self, id: &str, node: &Node, in_row: bool) -> bool {
        let image_key = crate::images::key(id, None);
        let panel = self.content.tree.name == "panel" && !in_row;
        let recovered = panel
            && crate::images::panel_fallback(id).is_some()
            && self.images.contains_key(&image_key);
        let recovered_dot = panel
            && crate::images::fallback_image(id)
                .is_some_and(|image| self.images.contains_key(&crate::images::key(image, None)));
        !((flag(node, "hidden") && !recovered) || recovered_dot)
    }

    fn element(
        &mut self,
        nodes: Nodes<'_>,
        id: &str,
        parent: Parent,
        origin: Origin<'_>,
        depth: usize,
    ) -> Option<NodeId> {
        let Origin { source, key, list } = origin;
        if depth > 64 {
            return None;
        }
        let node = nodes.get(id)?;
        if !self.drawn(id, node, *source != Source::Tree) {
            return None;
        }
        let key = format!("{key}/{id}");
        let style = Self::sizing(node, parent);
        let meta = |leaf: Option<Leaf>| Meta {
            key: key.clone(),
            id: id.to_owned(),
            source: source.clone(),
            cell: false,
            list_inner: list.map(str::to_owned),
            leaf,
        };
        let created = match node.family.as_str() {
            "column" | "row" => {
                let main = if node.family == "row" {
                    Axis::Horizontal
                } else {
                    Axis::Vertical
                };
                let style = self.container_style(nodes, node, style, main);
                let inner = Parent { main };
                let kids: Vec<NodeId> = children(node)
                    .filter_map(|child| {
                        self.element(
                            nodes,
                            child,
                            inner,
                            Origin {
                                source,
                                key: &key,
                                list,
                            },
                            depth + 1,
                        )
                    })
                    .collect();
                let created = self.tree.new_with_children(style, &kids).ok()?;
                self.tree.set_node_context(created, Some(meta(None))).ok()?;
                created
            }
            "list" => return self.list(id, node, style, &key, list, depth),
            "text" => {
                let mono = flag(node, "mono");
                let size = number(node, "size").unwrap_or(self.text_style(mono).size);
                let leaf = Leaf::Text {
                    text: port_text(node, "text").to_owned(),
                    size,
                    mono,
                    bold: flag(node, "bold"),
                    elide: flag(node, "elide"),
                    wrap: flag(node, "wrap"),
                    max_lines: number(node, "max_lines").unwrap_or(3.0) as usize,
                };
                self.tree
                    .new_leaf_with_context(style, meta(Some(leaf)))
                    .ok()?
            }
            "button" => {
                let leaf = Leaf::Button {
                    label: port_text(node, "label").to_owned(),
                };
                self.tree
                    .new_leaf_with_context(style, meta(Some(leaf)))
                    .ok()?
            }
            "field" => self
                .tree
                .new_leaf_with_context(style, meta(Some(Leaf::Field)))
                .ok()?,
            "toggle" => {
                let leaf = Leaf::Toggle {
                    label: port_text(node, "label").to_owned(),
                };
                self.tree
                    .new_leaf_with_context(style, meta(Some(leaf)))
                    .ok()?
            }
            "image" | "spacer" => self
                .tree
                .new_leaf_with_context(style, meta(Some(Leaf::Fixed)))
                .ok()?,
            // The window envelope draws nothing (Quoin: height 0).
            _ => return None,
        };
        Some(created)
    }

    fn list(
        &mut self,
        id: &str,
        node: &Node,
        mut style: Style,
        key: &str,
        _outer: Option<&str>,
        depth: usize,
    ) -> Option<NodeId> {
        let items = rows(node);
        if items.is_empty() && flag(node, "hidden_if_empty") {
            return None;
        }
        let data = self.content.lists.get(id)?;
        let horizontal = port_text(node, "flow") == "horizontal";
        let template = port_text(node, "row").to_owned();
        let row_height = number(node, "row_height").unwrap_or(24.0);
        let gap = number(node, "gap").unwrap_or(0.0);
        let mut cells = Vec::with_capacity(items.len());
        // The sum of the cells' floors (see below): the list's own minimum.
        let mut floors = 0.0;
        for item in items {
            let item_id = item["id"].as_str().unwrap_or_default().to_owned();
            let Some(instance) = data.instances.get(&item_id) else {
                continue;
            };
            let source = Source::Row {
                list: id.to_owned(),
                item: item_id.clone(),
            };
            let inner = Parent {
                main: if horizontal {
                    Axis::Horizontal
                } else {
                    Axis::Vertical
                },
            };
            let row_key = format!("{key}#{item_id}");
            let Some(content) = self.element(
                Nodes::Row(instance),
                &template,
                inner,
                Origin {
                    source: &source,
                    key: &row_key,
                    list: Some(id),
                },
                depth + 1,
            ) else {
                continue;
            };
            // The cell: the row's click target, the full width of a vertical
            // list at `row_height`, its content's size in a horizontal one.
            let mut cell_style = Style {
                display: Display::Flex,
                flex_direction: if horizontal {
                    FlexDirection::Row
                } else {
                    FlexDirection::Column
                },
                align_items: Some(AlignItems::STRETCH),
                flex_shrink: 0.0,
                ..Style::default()
            };
            if horizontal {
                // A crowded horizontal list (the panel's tasks) shrinks its
                // cells rather than running off its row, each down to its
                // instance's floor (an explicit minimum or a fixed width):
                // an `elide` text inside gives up its width, a fixed cell
                // keeps it.
                let floor = instance.get(&template).map_or(0.0, width_floor);
                floors += floor;
                cell_style.flex_shrink = 1.0;
                cell_style.min_size.width = lpa(floor);
                let mut inner_style = self.tree.style(content).ok()?.clone();
                inner_style.min_size.width = lpa(floor);
                self.tree.set_style(content, inner_style).ok()?;
            } else {
                cell_style.size.height = len(row_height);
            }
            let cell = self.tree.new_with_children(cell_style, &[content]).ok()?;
            self.tree
                .set_node_context(
                    cell,
                    Some(Meta {
                        key: row_key,
                        id: id.to_owned(),
                        source,
                        cell: true,
                        list_inner: Some(id.to_owned()),
                        leaf: None,
                    }),
                )
                .ok()?;
            cells.push(cell);
        }
        style.display = Display::Flex;
        if horizontal {
            style.flex_direction = FlexDirection::Row;
            style.gap = Size {
                width: length(gap),
                height: zero(),
            };
            style.align_items = Some(match port_text(node, "align") {
                "center" => AlignItems::CENTER,
                "end" => AlignItems::FLEX_END,
                _ => AlignItems::FLEX_START,
            });
            if !node.ports.contains_key("min_width") {
                let gaps = gap * cells.len().saturating_sub(1) as f32;
                style.min_size.width = lpa(floors + gaps);
            }
        } else {
            style.flex_direction = FlexDirection::Column;
            style.gap = Size {
                width: zero(),
                height: length(gap),
            };
            style.align_items = Some(AlignItems::STRETCH);
            style.overflow = taffy::Point {
                x: Overflow::Visible,
                y: Overflow::Scroll,
            };
            style.scrollbar_width = 0.0;
            if style.size.height == Dimension::auto() && style.flex_grow == 0.0 {
                style.size.height = len(super::list_height(node));
            }
        }
        let list = self.tree.new_with_children(style, &cells).ok()?;
        self.tree
            .set_node_context(
                list,
                Some(Meta {
                    key: format!("{key}/{id}"),
                    id: id.to_owned(),
                    source: Source::Tree,
                    cell: false,
                    list_inner: None,
                    leaf: None,
                }),
            )
            .ok()?;
        Some(list)
    }
}

/// The size a leaf measures as, given what the parent already fixed.
fn leaf_size(
    leaf: &Leaf,
    known: Size<Option<f32>>,
    available: Size<AvailableSpace>,
    styles: TextStyles,
    measure: Measurer<'_>,
) -> Size<f32> {
    let line = |mono: bool, size: f32| {
        let style = if mono { styles.mono } else { styles.ui };
        style.line(size)
    };
    let measured = match leaf {
        Leaf::Text {
            text,
            size,
            mono,
            bold,
            elide,
            wrap,
            max_lines,
        } => {
            let w = measure(text, *size, *mono, *bold).x;
            let bound = known.width.or(match available.width {
                AvailableSpace::Definite(width) => Some(width.max(0.0)),
                AvailableSpace::MinContent => Some(0.0),
                AvailableSpace::MaxContent => None,
            });
            if *wrap {
                let width = bound.unwrap_or(w.ceil());
                let lines = wrap_text(text, width, *max_lines, &|s| {
                    measure(s, *size, *mono, *bold).x
                });
                return Size {
                    width: known.width.unwrap_or(w.ceil().min(width)),
                    height: known
                        .height
                        .unwrap_or(line(*mono, *size) * lines.len() as f32),
                };
            }
            Size {
                width: if *elide {
                    match available.width {
                        AvailableSpace::Definite(width) => w.ceil().min(width.max(0.0)),
                        AvailableSpace::MinContent => 0.0,
                        AvailableSpace::MaxContent => w.ceil(),
                    }
                } else {
                    w.ceil()
                },
                height: line(*mono, *size),
            }
        }
        Leaf::Button { label } => {
            let w = measure(label, styles.ui.size, false, false).x;
            Size {
                width: (w + 20.0).ceil(),
                height: line(false, styles.ui.size) + 8.0,
            }
        }
        Leaf::Field => Size {
            width: 160.0,
            height: line(false, styles.ui.size) + 8.0,
        },
        Leaf::Toggle { label } => {
            let h = line(false, styles.ui.size);
            let w = measure(label, styles.ui.size, false, false).x;
            let switch = h * 1.8;
            Size {
                width: (switch + if label.is_empty() { 0.0 } else { 8.0 + w }).ceil(),
                height: h,
            }
        }
        Leaf::Fixed => Size {
            width: 0.0,
            height: 0.0,
        },
    };
    Size {
        width: known.width.unwrap_or(measured.width),
        height: known.height.unwrap_or(measured.height),
    }
}

/// Pixel-measured wrapping shared by layout and paint. Break words when a
/// single word is wider than the box; only the last allowed line elides.
pub(super) fn wrap_text(
    text: &str,
    width: f32,
    max_lines: usize,
    measure: &dyn Fn(&str) -> f32,
) -> Vec<String> {
    let max_lines = max_lines.max(1);
    let mut lines = Vec::new();
    for paragraph in text.split('\n') {
        let mut current = String::new();
        for word in paragraph.split_whitespace() {
            let candidate = if current.is_empty() {
                word.to_owned()
            } else {
                format!("{current} {word}")
            };
            if measure(&candidate) <= width {
                current = candidate;
                continue;
            }
            if !current.is_empty() {
                lines.push(std::mem::take(&mut current));
            }
            for ch in word.chars() {
                let candidate = format!("{current}{ch}");
                if !current.is_empty() && measure(&candidate) > width {
                    lines.push(std::mem::take(&mut current));
                }
                current.push(ch);
            }
        }
        lines.push(current);
    }
    if lines.len() > max_lines {
        lines.truncate(max_lines);
        let last = lines.last_mut().unwrap();
        while !last.is_empty() && measure(&format!("{}…", last.trim_end())) > width {
            last.pop();
        }
        *last = if measure("…") <= width {
            format!("{}…", last.trim_end())
        } else {
            String::new()
        };
    }
    // A box narrower than a glyph cannot paint even that glyph safely.
    for line in &mut lines {
        if measure(line) > width {
            line.clear();
        }
    }
    lines
}

/// Lay `content` out on a surface of `size` points.
pub(crate) fn layout(
    content: &Content,
    size: Vec2,
    styles: TextStyles,
    images: &crate::images::Prepared,
    measure: Measurer<'_>,
) -> SceneLayout {
    let top = if content.frame.is_some() {
        TITLEBAR
    } else {
        0.0
    };
    let body = Rect::from_min_size(pos2(0.0, top), vec2(size.x, (size.y - top).max(0.0)));
    // The close button: 28 x 24, 11 in from the right of the title bar.
    let close = content.frame.is_some().then(|| {
        Rect::from_center_size(pos2(size.x - 11.0 - 14.0, TITLEBAR / 2.0), vec2(28.0, 24.0))
    });
    let mut builder = Builder {
        content,
        styles,
        images,
        tree: TaffyTree::new(),
    };
    let tree = &content.tree;
    let root_id = tree
        .nodes
        .get("root")
        .map(|_| "root".to_owned())
        .or_else(|| {
            tree.nodes
                .keys()
                .find(|id| !tree.templates.contains(*id))
                .cloned()
        });
    let top_parent = Parent {
        main: Axis::Vertical,
    };
    let root = root_id.and_then(|id| {
        builder.element(
            Nodes::Tree(tree),
            &id,
            top_parent,
            Origin {
                source: &Source::Tree,
                key: "",
                list: None,
            },
            0,
        )
    });
    // The page: a column filling the body, stretching the root across it.
    let page_style = Style {
        display: Display::Flex,
        flex_direction: FlexDirection::Column,
        align_items: Some(AlignItems::STRETCH),
        size: Size {
            width: len(body.width()),
            height: len(body.height()),
        },
        ..Style::default()
    };
    let kids: Vec<NodeId> = root.into_iter().collect();
    let Ok(page) = builder.tree.new_with_children(page_style, &kids) else {
        return SceneLayout {
            close,
            ..SceneLayout::default()
        };
    };
    let Builder { mut tree, .. } = builder;
    let _ = tree.compute_layout_with_measure(
        page,
        Size {
            width: AvailableSpace::Definite(body.width()),
            height: AvailableSpace::Definite(body.height()),
        },
        |inputs: LayoutInput, _id, context: Option<&mut Meta>, style: &Style| -> LayoutOutput {
            let leaf = context.and_then(|meta| meta.leaf.clone());
            taffy::compute_leaf_layout(
                inputs,
                style,
                |_, _| 0.0,
                |known, available| match &leaf {
                    Some(leaf) => leaf_size(leaf, known, available, styles, measure),
                    None => Size::ZERO,
                },
            )
        },
    );
    let mut out = SceneLayout {
        close,
        ..SceneLayout::default()
    };
    collect(&tree, page, body.min, &mut out);
    out
}

/// Walk the laid-out tree, absolute rects in paint order.
fn collect(tree: &TaffyTree<Meta>, node: NodeId, origin: Pos2, out: &mut SceneLayout) {
    let Ok(layout) = tree.layout(node) else {
        return;
    };
    let rect = Rect::from_min_size(
        origin + vec2(layout.location.x, layout.location.y),
        vec2(layout.size.width, layout.size.height),
    );
    if let Some(meta) = tree.get_node_context(node) {
        if meta.cell {
            if let Source::Row { list, item } = &meta.source {
                out.instances
                    .entry(list.clone())
                    .or_default()
                    .insert(item.clone(), rect);
            }
        } else if meta.source == Source::Tree {
            out.nodes.insert(meta.id.clone(), rect);
        }
        out.placed.push(Placed {
            key: meta.key.clone(),
            id: meta.id.clone(),
            source: meta.source.clone(),
            rect,
            cell: meta.cell,
            in_list: meta.list_inner.clone(),
        });
        // A scrolling list: its children overflow; remember how far.
        if meta.leaf.is_none()
            && !meta.cell
            && meta.source == Source::Tree
            && tree
                .style(node)
                .is_ok_and(|style| style.overflow.y == Overflow::Scroll)
        {
            let content_height = layout
                .scrollable_overflow_rect
                .bottom
                .max(layout.size.height);
            out.lists.insert(
                meta.id.clone(),
                ListBox {
                    rect,
                    content_height,
                },
            );
        }
    }
    if let Ok(children) = tree.children(node) {
        for child in children {
            collect(tree, child, rect.min, out);
        }
    }
}
