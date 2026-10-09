// SPDX-License-Identifier: MIT OR Apache-2.0
//! The window's own title bar (client-side decorations; chrome specification
//! §3.1–3.2). An application opens its window undecorated ([`viewport`])
//! and the top row of the window is drawn here, edge to edge in the
//! `chrome` role, left to right:
//!
//! - the app mark, after the bar's inner margin;
//! - the registry's menus ([`crate::menu`]), starting one mark gap later;
//! - the window title, centred on the whole bar and sliding or eliding to
//!   keep clear of the menus and the caption buttons;
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

use crate::chrome::{CLOSE_PRESSED_ALPHA, Chrome};
use crate::command::Registry;
use crate::icons::{self, Icon};
use crate::strings::Strings;
use egui::emath::GuiRounding;
use egui::text::{LayoutJob, TextWrapping};
use egui::{
    Align, CursorIcon, Frame, Layout, Pos2, Rect, ResizeDirection, Sense, Stroke, StrokeKind, Ui, UiBuilder,
    ViewportBuilder, ViewportCommand, WidgetInfo, WidgetType, pos2, vec2,
};

/// One caption button's width (§3.2).
pub const BUTTON_WIDTH: f32 = 46.0;

/// Thickness of a resize zone along an edge; corners are [`CORNER`] square
/// and win where they overlap the edges (§3.1).
pub const EDGE: f32 = 5.0;
pub const CORNER: f32 = 12.0;

/// The title is elided into the gap only when the gap is wider than this.
const MIN_TITLE_ROOM: f32 = 80.0;

const CATALOGUE: &str = include_str!("../i18n/en/toolkit.ftl");

thread_local! {
    static STRINGS: Strings = Strings::new(CATALOGUE);
}

fn label(key: &str) -> String {
    STRINGS.with(|s| s.get(key))
}

/// An undecorated window for a MixOS app: the title bar is drawn by [`show`].
pub fn viewport(app_id: &str, title: &str) -> ViewportBuilder {
    ViewportBuilder::default().with_app_id(app_id).with_title(title).with_decorations(false)
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
    let chrome = Chrome::of(ui.ctx());
    let (p, m) = (chrome.palette, chrome.metrics);
    let mut fired = Vec::new();
    let panel = egui::Panel::top("toolkit-titlebar").exact_size(m.title_bar_height).frame(Frame::new().fill(p.chrome));
    panel.show(ui, |ui| {
        let bar = ui.max_rect();
        let gap_id = ui.id().with("titlebar-gap");
        let gap = ui.data(|d| d.get_temp::<Rect>(gap_id)).unwrap_or(bar);
        // Registered first and over the free gap only, so nothing drawn
        // later loses its clicks to it.
        let drag = ui.interact(gap, ui.id().with("titlebar-drag"), Sense::click_and_drag());

        if let Some(icon) = icon {
            let at = pos2(bar.left() + m.title_bar_margin, bar.center().y - m.mark / 2.0);
            let mark = Rect::from_min_size(at, vec2(m.mark, m.mark)).round_to_pixels(ui.pixels_per_point());
            icons::image(icon, stroke, m.mark, p.icon).paint_at(ui, mark);
        }

        let captions = bar.right() - 3.0 * m.caption_width;
        let menus_left = bar.left() + m.title_bar_margin + m.mark + m.mark_gap;
        let room = Rect::from_min_max(pos2(menus_left, bar.top()), pos2(captions, bar.bottom()));
        let mut row = ui.new_child(UiBuilder::new().max_rect(room).layout(Layout::left_to_right(Align::Center)));
        row.spacing_mut().item_spacing.x = 0.0;
        fired = registry.menus(&mut row, state, strings);
        let menus_right = row.min_rect().right().max(menus_left);

        for (index, caption) in [Caption::Minimize, Caption::Maximize, Caption::Close].into_iter().enumerate() {
            let left = captions + index as f32 * m.caption_width;
            let rect = Rect::from_min_size(pos2(left, bar.top()), vec2(m.caption_width, bar.height()));
            caption_button(ui, rect, caption, &chrome);
        }

        window_title(ui, &chrome, title, bar, menus_right + m.title_gap, captions - m.title_gap);

        let free = Rect::from_min_max(pos2(menus_right, bar.top()), pos2(captions - m.caption_gap, bar.bottom()));
        ui.data_mut(|d| d.insert_temp(gap_id, free));
        if drag.double_clicked() {
            ui.ctx().send_viewport_cmd(ViewportCommand::Maximized(!maximized(ui)));
        } else if drag.drag_started_by(egui::PointerButton::Primary) {
            ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
        }
    });
    fired
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
    let mut galley = painter.layout_no_wrap(title.to_owned(), font.clone(), chrome.palette.text_dim);
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
    let at = Pos2::new(x, bar.center().y - galley.size().y / 2.0).round_to_pixels(ui.pixels_per_point());
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
        Caption::Close if pressed => (Some(p.caption_close.gamma_multiply(CLOSE_PRESSED_ALPHA)), p.caption_close_text),
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
            let front = Rect::from_min_max(pos2(b.left(), b.top() + inset), pos2(b.right() - inset, b.bottom()));
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
        (D::NorthWest, Rect::from_min_size(r.left_top(), vec2(corner, corner)), CursorIcon::ResizeNorthWest),
        (D::NorthEast, Rect::from_min_size(r.right_top() - vec2(corner, 0.0), vec2(corner, corner)), CursorIcon::ResizeNorthEast),
        (D::SouthWest, Rect::from_min_size(r.left_bottom() - vec2(0.0, corner), vec2(corner, corner)), CursorIcon::ResizeSouthWest),
        (D::SouthEast, Rect::from_min_size(r.right_bottom() - vec2(corner, corner), vec2(corner, corner)), CursorIcon::ResizeSouthEast),
        (D::North, Rect::from_min_max(r.left_top() + vec2(corner, 0.0), r.right_top() + vec2(-corner, edge)), CursorIcon::ResizeNorth),
        (D::South, Rect::from_min_max(r.left_bottom() + vec2(corner, -edge), r.right_bottom() - vec2(corner, 0.0)), CursorIcon::ResizeSouth),
        (D::West, Rect::from_min_max(r.left_top() + vec2(0.0, corner), r.left_bottom() + vec2(edge, -corner)), CursorIcon::ResizeWest),
        (D::East, Rect::from_min_max(r.right_top() + vec2(-edge, corner), r.right_bottom() - vec2(0.0, corner)), CursorIcon::ResizeEast),
    ];
    for (direction, zone, cursor) in zones {
        let response = ui.interact(zone, ui.id().with(("toolkit-resize", format!("{direction:?}"))), Sense::drag());
        if response.on_hover_cursor(cursor).drag_started_by(egui::PointerButton::Primary) {
            ui.ctx().send_viewport_cmd(ViewportCommand::BeginResize(direction));
        }
    }
}
