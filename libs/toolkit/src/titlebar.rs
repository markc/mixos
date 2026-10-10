// SPDX-License-Identifier: MIT OR Apache-2.0
//! The window's own title bar (client-side decorations; chrome specification
//! §3.1–3.2). An application opens its window undecorated ([`viewport`])
//! and the top row of the window is drawn here, edge to edge in the
//! `chrome` role, left to right:
//!
//! - the app mark, after the bar's inner margin;
//! - the registry's menus ([`crate::menu`]), starting one mark gap later;
//! - the window title, centred on the whole bar and sliding or eliding to
//!   keep clear of the menus and the controls;
//! - an optional right-hand group of [`Control`]s ([`show_with`]): a
//!   dropdown, icon buttons and a link, which give way as the bar narrows
//!   ([`fit`]);
//! - minimize, maximize/restore and close, [`BUTTON_WIDTH`] wide each and
//!   the full bar height, with Close flush in the top-right corner.
//!
//! Only the free gap between the menus and the caption buttons drags the
//! window, so a press on a menu title opens the menu and never moves the
//! window; a double-click there maximizes or restores. The gap is last
//! frame's, because the menus are laid out after the drag zone is
//! registered. [`edges`] adds invisible resize zones; call it last in the
//! frame so it wins at the borders. Every colour and size comes from the
//! theme's [`Chrome`].

use crate::button::{ICON_BUTTON, IconButton, Link};
use crate::chrome::{CLOSE_PRESSED_ALPHA, Chrome};
use crate::command::Registry;
use crate::icons::{self, Icon};
use crate::strings::Strings;
use egui::emath::GuiRounding;
use egui::text::{LayoutJob, TextWrapping};
use egui::{
    Align, CursorIcon, Frame, Layout, Pos2, Rect, ResizeDirection, Sense, Stroke, StrokeKind, Ui,
    UiBuilder, ViewportBuilder, ViewportCommand, WidgetInfo, WidgetType, pos2, vec2,
};

/// One caption button's width (§3.2).
pub const BUTTON_WIDTH: f32 = 46.0;

/// Thickness of a resize zone along an edge; corners are [`CORNER`] square
/// and win where they overlap the edges (§3.1).
pub const EDGE: f32 = 5.0;
pub const CORNER: f32 = 12.0;

/// The title is elided into the gap only when the gap is wider than this.
const MIN_TITLE_ROOM: f32 = 80.0;

fn label(key: &str) -> String {
    crate::strings::own(key)
}

/// An undecorated window for a MixOS app: the title bar is drawn by [`show`].
pub fn viewport(app_id: &str, title: &str) -> ViewportBuilder {
    ViewportBuilder::default()
        .with_app_id(app_id)
        .with_title(title)
        .with_decorations(false)
}

fn maximized(ui: &Ui) -> bool {
    ui.input(|i| i.viewport().maximized.unwrap_or(false))
}

/// The title bar's height: 32 pt in Pro, 38 in Studio and Classic, one
/// control plus an item gap above and below in the hue schemes.
pub fn height(ui: &Ui) -> f32 {
    Chrome::of(ui.ctx()).metrics.title_bar_height
}

/// What a caption button does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Caption {
    Minimize,
    Maximize,
    Close,
}

/// One control of the right-hand group (§3.1), which runs right to left
/// from the caption buttons in the order given. Icons and links run
/// registry commands, so their labels, tooltips and enablement come from
/// the registry like the menus'.
pub enum Control<'a> {
    /// A dropdown, [`COMBO_WIDTH`] wide, shrinking to its minimum before
    /// anything else gives way (a workspace switcher).
    Combo {
        id: &'a str,
        selected: &'a mut usize,
        options: &'a [String],
    },
    /// A 28 pt icon button; `selected` shows it toggled on.
    Icon {
        command: &'static str,
        icon: Icon,
        selected: bool,
    },
    /// A frameless text link with a 14 pt icon.
    Link { command: &'static str, icon: Icon },
}

/// The right-hand group's combo width range (§3.1).
pub const COMBO_WIDTH: egui::Rangef = egui::Rangef {
    min: 90.0,
    max: 130.0,
};

/// Spacing within the right-hand group (§3.1): an icon's slot is its 28 pt
/// plus this, the specified 34.
const CONTROL_SPACING: f32 = 6.0;

/// A link shows only while this much of the bar stays free (§3.1).
const LINK_ROOM: f32 = 120.0;

/// The theme toggle's icon: a sun while the theme is dark, else a moon.
pub fn theme_icon(dark: bool) -> Icon {
    if dark { Icon::Sun } else { Icon::Moon }
}

/// A control's room in the group, for [`fit`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Slot {
    Combo,
    /// An icon or link this wide.
    Fixed(f32),
    Link(f32),
}

/// Which controls of the group fit in `free` points: the combo's width and
/// how many controls show, nearest the caption buttons first. Controls drop
/// from the far end (§3.1: the link, then the theme toggle, then search),
/// the combo shrinks to its minimum before any does, and a link shows only
/// while [`LINK_ROOM`] stays free besides it.
pub fn fit(free: f32, slots: &[Slot]) -> (f32, usize) {
    let need = |n: usize, combo: f32| {
        let width: f32 = slots[..n]
            .iter()
            .map(|s| {
                CONTROL_SPACING
                    + match s {
                        Slot::Combo => combo,
                        Slot::Fixed(w) | Slot::Link(w) => *w,
                    }
            })
            .sum();
        let link = slots[..n].iter().any(|s| matches!(s, Slot::Link(_)));
        width + if link { LINK_ROOM } else { 0.0 }
    };
    let mut shown = slots.len();
    while shown > 0 && need(shown, COMBO_WIDTH.min) > free {
        shown -= 1;
    }
    let combo = (free - need(shown, 0.0)).clamp(COMBO_WIDTH.min, COMBO_WIDTH.max);
    (combo, shown)
}

/// Draw the title bar at the top of `ui` and return the ids of commands
/// chosen from its menus this frame. `icon` is the app's mark; `stroke` its
/// Lucide stroke width.
pub fn show<S>(
    ui: &mut Ui,
    title: &str,
    icon: Option<Icon>,
    stroke: f32,
    registry: &Registry<S>,
    state: &S,
    strings: &Strings,
) -> Vec<&'static str> {
    show_with(ui, title, icon, stroke, registry, state, strings, &mut [])
}

/// [`show`] with a right-hand group of `controls` before the caption
/// buttons. The ids of commands run from the controls join the menus'.
#[expect(
    clippy::too_many_arguments,
    reason = "the title bar's inputs, as `show` takes them"
)]
pub fn show_with<S>(
    ui: &mut Ui,
    title: &str,
    icon: Option<Icon>,
    stroke: f32,
    registry: &Registry<S>,
    state: &S,
    strings: &Strings,
    controls: &mut [Control<'_>],
) -> Vec<&'static str> {
    let chrome = Chrome::of(ui.ctx());
    let (p, m) = (chrome.palette, chrome.metrics);
    let mut fired = Vec::new();
    let panel = egui::Panel::top("toolkit-titlebar")
        .exact_size(m.title_bar_height)
        .frame(Frame::new().fill(p.chrome));
    panel.show(ui, |ui| {
        let bar = ui.max_rect();
        let gap_id = ui.id().with("titlebar-gap");
        let gap = ui.data(|d| d.get_temp::<Rect>(gap_id)).unwrap_or(bar);
        // Registered first and over the free gap only, so nothing drawn
        // later loses its clicks to it.
        let drag = ui.interact(gap, ui.id().with("titlebar-drag"), Sense::click_and_drag());

        if let Some(icon) = icon {
            let at = pos2(
                bar.left() + m.title_bar_margin,
                bar.center().y - m.mark / 2.0,
            );
            let mark = Rect::from_min_size(at, vec2(m.mark, m.mark))
                .round_to_pixels(ui.pixels_per_point());
            icons::image(icon, stroke, m.mark, p.icon).paint_at(ui, mark);
        }

        let captions = bar.right() - 3.0 * m.caption_width;
        let menus_left = bar.left() + m.title_bar_margin + m.mark + m.mark_gap;
        let room = Rect::from_min_max(pos2(menus_left, bar.top()), pos2(captions, bar.bottom()));
        let mut row = ui.new_child(
            UiBuilder::new()
                .max_rect(room)
                .layout(Layout::left_to_right(Align::Center)),
        );
        row.spacing_mut().item_spacing.x = 0.0;
        fired = registry.menus(&mut row, state, strings);
        let menus_right = row.min_rect().right().max(menus_left);

        let group_right = captions - m.caption_gap;
        let group_left = right_group(
            ui,
            controls,
            registry,
            state,
            strings,
            menus_right + m.title_gap,
            group_right,
            &mut fired,
        );

        for (index, caption) in [Caption::Minimize, Caption::Maximize, Caption::Close]
            .into_iter()
            .enumerate()
        {
            let left = captions + index as f32 * m.caption_width;
            let rect =
                Rect::from_min_size(pos2(left, bar.top()), vec2(m.caption_width, bar.height()));
            caption_button(ui, rect, caption, &chrome);
        }

        let title_right = if controls.is_empty() {
            captions
        } else {
            group_left
        };
        window_title(
            ui,
            &chrome,
            title,
            bar,
            menus_right + m.title_gap,
            title_right - m.title_gap,
        );

        let free = Rect::from_min_max(pos2(menus_right, bar.top()), pos2(group_left, bar.bottom()));
        ui.data_mut(|d| d.insert_temp(gap_id, free));
        if drag.double_clicked() {
            ui.ctx()
                .send_viewport_cmd(ViewportCommand::Maximized(!maximized(ui)));
        } else if drag.drag_started_by(egui::PointerButton::Primary) {
            ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
        }
    });
    fired
}

/// Lay out and draw the right-hand group, right to left from `right`, in
/// the room down to `left`; push the commands its controls ran. Returns
/// the group's left edge (`right` when nothing shows).
#[expect(
    clippy::too_many_arguments,
    reason = "the title bar's inputs, called from one place"
)]
fn right_group<S>(
    ui: &mut Ui,
    controls: &mut [Control<'_>],
    registry: &Registry<S>,
    state: &S,
    strings: &Strings,
    left: f32,
    right: f32,
    fired: &mut Vec<&'static str>,
) -> f32 {
    if controls.is_empty() {
        return right;
    }
    let slots: Vec<Slot> = controls
        .iter()
        .map(|c| match c {
            Control::Combo { .. } => Slot::Combo,
            Control::Icon { .. } => Slot::Fixed(ICON_BUTTON),
            Control::Link { command, .. } => {
                let label = registry
                    .get(command)
                    .map(|c| crate::command::label_text(strings, c.label))
                    .unwrap_or_default();
                Slot::Link(Link::width(ui, &label))
            }
        })
        .collect();
    let (combo_width, shown) = fit(right - left, &slots);
    let bar = ui.max_rect();
    let mut x = right;
    for control in controls.iter_mut().take(shown) {
        let width = match control {
            Control::Combo { .. } => combo_width,
            Control::Icon { .. } => ICON_BUTTON,
            Control::Link { command, .. } => {
                let label = registry
                    .get(command)
                    .map(|c| crate::command::label_text(strings, c.label))
                    .unwrap_or_default();
                Link::width(ui, &label)
            }
        };
        let rect = Rect::from_min_max(pos2(x - width, bar.top()), pos2(x, bar.bottom()));
        let mut cell = ui.new_child(
            UiBuilder::new()
                .max_rect(rect)
                .layout(Layout::left_to_right(Align::Center)),
        );
        let command = match control {
            Control::Combo {
                id,
                selected,
                options,
            } => {
                crate::combo::show(&mut cell, *id, selected, options, Some(width));
                None
            }
            Control::Icon {
                command,
                icon,
                selected,
            } => registry.get(command).map(|c| {
                let tooltip = crate::tooltip::for_command(ui.ctx(), c, strings);
                let button = IconButton::new(*icon).selected(*selected).tooltip(tooltip);
                (c, cell.add_enabled((c.enabled)(state), button).clicked())
            }),
            Control::Link { command, icon } => registry.get(command).map(|c| {
                let link = Link::new(*icon, crate::command::label_text(strings, c.label));
                (c, cell.add_enabled((c.enabled)(state), link).clicked())
            }),
        };
        if let Some((command, true)) = command {
            fired.push(command.id);
        }
        x -= width + CONTROL_SPACING;
    }
    x + CONTROL_SPACING
}

/// The window title in Inter Medium, `text_dim`: centred on the bar, slid
/// sideways to stay within `left..right`, elided with "…" from `left` when
/// it cannot fit but the gap is wider than [`MIN_TITLE_ROOM`], else omitted.
fn window_title(ui: &Ui, chrome: &Chrome, title: &str, bar: Rect, left: f32, right: f32) {
    let room = right - left;
    if title.is_empty() || room <= 0.0 {
        return;
    }
    let font = crate::fonts::bound(ui.ctx(), crate::fonts::medium(chrome.metrics.title_size));
    let painter = ui.painter();
    let mut galley =
        painter.layout_no_wrap(title.to_owned(), font.clone(), chrome.palette.text_dim);
    let width = galley.size().x;
    let x = if width <= room {
        // Centred on the bar inside its inner margin (measured: 645 on a
        // 1280 pt bar with a 10 pt margin).
        let centre = (bar.left() + chrome.metrics.title_bar_margin + bar.right()) / 2.0;
        (centre - width / 2.0).clamp(left, right - width)
    } else if room > MIN_TITLE_ROOM {
        let mut job = LayoutJob::simple_singleline(title.to_owned(), font, chrome.palette.text_dim);
        job.wrap = TextWrapping::truncate_at_width(room);
        galley = painter.layout_job(job);
        left
    } else {
        return;
    };
    let at =
        Pos2::new(x, bar.center().y - galley.size().y / 2.0).round_to_pixels(ui.pixels_per_point());
    painter.galley(at, galley, chrome.palette.text_dim);
}

fn caption_button(ui: &mut Ui, rect: Rect, caption: Caption, chrome: &Chrome) {
    let max = maximized(ui);
    let key = match caption {
        Caption::Minimize => "window-minimize",
        Caption::Maximize if max => "window-restore",
        Caption::Maximize => "window-maximize",
        Caption::Close => "window-close",
    };
    let name = label(key);
    let response = ui.interact(rect, ui.id().with(("toolkit-caption", key)), Sense::click());
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, &name));
    let p = &chrome.palette;
    let pressed = response.is_pointer_button_down_on();
    let hovered = response.hovered() || pressed;
    // Flat fills, no animation (§3.2, §4).
    let (fill, glyph) = match caption {
        Caption::Close if pressed => (
            Some(p.caption_close.gamma_multiply(CLOSE_PRESSED_ALPHA)),
            p.caption_close_text,
        ),
        Caption::Close if hovered => (Some(p.caption_close), p.caption_close_text),
        _ if pressed => (Some(p.pressed), p.icon),
        _ if hovered => (Some(p.hover), p.icon),
        _ => (None, p.icon),
    };
    let painter = ui.painter();
    if let Some(fill) = fill {
        painter.rect_filled(rect, 0.0, fill);
    }
    let size = chrome.metrics.caption_glyph;
    let b = Rect::from_center_size(rect.center(), vec2(size, size));
    let line = Stroke::new(1.0, glyph);
    match caption {
        Caption::Minimize => {
            painter.line_segment([b.left_center(), b.right_center()], line);
        }
        Caption::Maximize if max => {
            // The front square (the box less 2 pt at the top and right) and
            // the visible corner of the square behind it.
            let inset = 2.0;
            let front = Rect::from_min_max(
                pos2(b.left(), b.top() + inset),
                pos2(b.right() - inset, b.bottom()),
            );
            painter.rect_stroke(front, 0.0, line, StrokeKind::Middle);
            let back = [
                pos2(front.left() + inset, front.top()),
                pos2(front.left() + inset, b.top()),
                b.right_top(),
                pos2(b.right(), b.bottom() - inset),
                pos2(front.right(), b.bottom() - inset),
            ];
            painter.add(egui::Shape::line(back.to_vec(), line));
        }
        Caption::Maximize => {
            painter.rect_stroke(b, 0.0, line, StrokeKind::Middle);
        }
        Caption::Close => {
            painter.line_segment([b.left_top(), b.right_bottom()], line);
            painter.line_segment([b.right_top(), b.left_bottom()], line);
        }
    }
    if response.on_hover_text(&name).clicked() {
        let command = match caption {
            Caption::Minimize => ViewportCommand::Minimized(true),
            Caption::Maximize => ViewportCommand::Maximized(!max),
            Caption::Close => ViewportCommand::Close,
        };
        ui.ctx().send_viewport_cmd(command);
    }
}

/// Invisible resize zones along the window's edges and corners. Call last in
/// the frame, after everything else, so the zones take the border clicks.
/// A maximized or full-screen window has none, so its top-right pixel hits
/// Close.
pub fn edges(ui: &mut Ui) {
    let fullscreen = ui.input(|i| i.viewport().fullscreen.unwrap_or(false));
    if maximized(ui) || fullscreen {
        return;
    }
    let m = Chrome::of(ui.ctx()).metrics;
    let (edge, corner) = (m.resize_edge, m.resize_corner);
    let r = ui.ctx().content_rect();
    use ResizeDirection as D;
    let zones = [
        (
            D::NorthWest,
            Rect::from_min_size(r.left_top(), vec2(corner, corner)),
            CursorIcon::ResizeNorthWest,
        ),
        (
            D::NorthEast,
            Rect::from_min_size(r.right_top() - vec2(corner, 0.0), vec2(corner, corner)),
            CursorIcon::ResizeNorthEast,
        ),
        (
            D::SouthWest,
            Rect::from_min_size(r.left_bottom() - vec2(0.0, corner), vec2(corner, corner)),
            CursorIcon::ResizeSouthWest,
        ),
        (
            D::SouthEast,
            Rect::from_min_size(
                r.right_bottom() - vec2(corner, corner),
                vec2(corner, corner),
            ),
            CursorIcon::ResizeSouthEast,
        ),
        (
            D::North,
            Rect::from_min_max(
                r.left_top() + vec2(corner, 0.0),
                r.right_top() + vec2(-corner, edge),
            ),
            CursorIcon::ResizeNorth,
        ),
        (
            D::South,
            Rect::from_min_max(
                r.left_bottom() + vec2(corner, -edge),
                r.right_bottom() - vec2(corner, 0.0),
            ),
            CursorIcon::ResizeSouth,
        ),
        (
            D::West,
            Rect::from_min_max(
                r.left_top() + vec2(0.0, corner),
                r.left_bottom() + vec2(edge, -corner),
            ),
            CursorIcon::ResizeWest,
        ),
        (
            D::East,
            Rect::from_min_max(
                r.right_top() + vec2(-edge, corner),
                r.right_bottom() - vec2(0.0, corner),
            ),
            CursorIcon::ResizeEast,
        ),
    ];
    for (direction, zone, cursor) in zones {
        let response = ui.interact(
            zone,
            ui.id().with(("toolkit-resize", format!("{direction:?}"))),
            Sense::drag(),
        );
        if response
            .on_hover_cursor(cursor)
            .drag_started_by(egui::PointerButton::Primary)
        {
            ui.ctx()
                .send_viewport_cmd(ViewportCommand::BeginResize(direction));
        }
    }
}
