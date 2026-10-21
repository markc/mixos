// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Tab strips (chrome specification §3.10–3.12): the overflow rules shared
//! by panel tabs, Studio pills and document tabs, and the document tabs.
//!
//! **Overflow** ([`fit`], §3.10). When the tabs do not fit:
//!
//! 1. they shrink, widest first, to a common cap, but never below
//!    min(natural, minimum);
//! 2. labels elide with "…", the padding giving way down to a third before
//!    the label is cut ([`label`]);
//! 3. tabs that still do not fit move into an 18 pt "»" chevron menu at the
//!    end of the strip ([`chevron`]); the selected tab always stays;
//! 4. a cut tab shows its full name as a tooltip.
//!
//! **Document tabs** ([`documents`], §3.12). Pro: a 26 pt `tab_strip` strip
//! of edge-to-edge tabs in Inter 11.5, title plus 42 wide (at least 72),
//! each ending in a 1 pt `separator`; the selected tab is `chrome` with
//! `text`, a hovered one `hover` at 35%, the rest bare, both in
//! `text_faint`; a 14 pt close box 13 pt from the right with a 10 pt ×.
//! Studio and Classic: a `canvas` strip (margins 8, 6 top, 4 bottom) of
//! tabs 4 pt apart, name plus meta plus 48 wide: the name in Inter Medium
//! 12.5 from 10 pt in, the meta in Inter 10.5 `text_faint` 6 pt after; the
//! selected tab is `card` with a 1 pt `card_border` inside, a hovered one
//! `hover` at 50%; a 16 pt close box 12 pt from the right, filled `hover`
//! at radius 4 when hovered, with an 11 pt ×. A tab under a layer drag
//! gets a 1.5 pt `accent` outline; a background job shows a progress
//! underline.

use crate::button::paint_icon;
use crate::chrome::Chrome;
use crate::icons::Icon;
use design::family::style::DocumentTabs;
use egui::emath::GuiRounding;
use egui::text::{LayoutJob, TextWrapping};
use egui::{
    Color32, FontFamily, FontId, Frame, Galley, Id, Margin, Popup, Rect, Sense, Shape, Stroke,
    StrokeKind, Ui, WidgetInfo, WidgetType, pos2, vec2,
};
use std::f32::consts::{FRAC_PI_2, PI};
use std::sync::Arc;

/// The overflow chevron's width, icon size, hover opacity and hover inset
/// (§3.10).

/// The chevron popup's least width (§3.10).

/// Pro document strip (§3.12).

/// Studio and Classic document strip (§3.12).

/// The drop-target outline (§3.12).

/// The progress underline of a background job. Its height is undetermined
/// by the specification (§5.4): 2 pt in `accent`, the drop line's weight.

/// The dirty marker after a document's name, as the window title has it.
pub const DIRTY: &str = "  •";

/// A status marker's icon side, and the gap between it and the name. Undetermined
/// by the specification (§5.4): sized to the 11.5 pt Pro title.

/// How a strip's tabs fit: each tab's width, or `None` when it moved into
/// the chevron menu.
#[derive(Clone, Debug, PartialEq)]
pub struct Fit {
    pub widths: Vec<Option<f32>>,
}

impl Fit {
    /// Whether any tab is in the chevron menu.
    pub fn overflows(&self) -> bool {
        self.widths.iter().any(Option::is_none)
    }
}

/// Fit tabs of natural widths `naturals` (each at least `minimum` unless its
/// natural width is less) into `available`, keeping `selected` on the strip
/// and leaving [`CHEVRON`] for the chevron when any must move (§3.10).
pub fn fit(naturals: &[f32], minimum: f32, available: f32, selected: Option<usize>) -> Fit {
    fit_with_chevron(naturals, minimum, available, selected, crate::style::default_tokens().tabs_chevron as f32)
}

fn fit_with_chevron(naturals: &[f32], minimum: f32, available: f32, selected: Option<usize>, chevron: f32) -> Fit {
    let floors: Vec<f32> = naturals.iter().map(|n| n.min(minimum)).collect();
    let mut shown: Vec<bool> = vec![true; naturals.len()];
    let total = |shown: &[bool], of: &[f32]| {
        shown
            .iter()
            .zip(of)
            .filter(|(s, _)| **s)
            .map(|(_, w)| w)
            .sum::<f32>()
    };
    let mut room = available;
    if total(&shown, &floors) > available {
        room = available - chevron;
        // The rightmost tabs go first, never the selected one.
        for index in (0..naturals.len()).rev() {
            if total(&shown, &floors) <= room {
                break;
            }
            if Some(index) != selected {
                shown[index] = false;
            }
        }
    }
    let widths = if total(&shown, naturals) <= room {
        naturals.to_vec()
    } else {
        let width = |cap: f32| -> Vec<f32> {
            naturals
                .iter()
                .zip(&floors)
                .map(|(n, f)| cap.min(*n).max(*f))
                .collect()
        };
        // The common cap, by bisection: the widths shrink monotonically with it.
        let (mut low, mut high) = (0.0_f32, naturals.iter().copied().fold(0.0, f32::max));
        for _ in 0..32 {
            let mid = (low + high) / 2.0;
            if total(&shown, &width(mid)) > room {
                high = mid
            } else {
                low = mid
            }
        }
        width(low)
    };
    Fit {
        widths: widths
            .into_iter()
            .zip(shown)
            .map(|(w, s)| s.then_some(w))
            .collect(),
    }
}

/// `text` laid out for a tab `width` wide whose natural padding is
/// `padding`: whole while the padding can give way to a third of itself,
/// else elided with "…". Returns the galley and whether the label was cut.
pub fn label(ui: &Ui, text: &str, font: FontId, width: f32, padding: f32) -> (Arc<Galley>, bool) {
    let painter = ui.painter();
    let whole = painter.layout_no_wrap(text.to_owned(), font.clone(), Color32::PLACEHOLDER);
    let room = width - padding / 3.0;
    if whole.size().x <= room {
        return (whole, false);
    }
    let mut job = LayoutJob::simple_singleline(text.to_owned(), font, Color32::PLACEHOLDER);
    job.wrap = TextWrapping::truncate_at_width(room.max(0.0));
    (painter.layout_job(job), true)
}

/// The "»" chevron in `rect` and its menu of `hidden` tabs (index and
/// label). Returns the index chosen from the menu.
pub fn chevron(ui: &mut Ui, id: Id, rect: Rect, hidden: &[(usize, String)]) -> Option<usize> {
    let response = ui.interact(rect, id, Sense::click());
    response.widget_info(|| {
        WidgetInfo::labeled(
            WidgetType::Button,
            ui.is_enabled(),
            crate::strings::own("more-tabs"),
        )
    });
    let chrome = Chrome::of(ui.ctx());
    let p = &chrome.palette;
    let hot =
        response.hovered() || Popup::is_id_open(ui.ctx(), Popup::default_response_id(&response));
    if hot {
        let fill = p.hover.gamma_multiply(crate::chrome::Chrome::of(ui.ctx()).style.tabs_chevron_hover_alpha as f32);
        ui.painter()
            .rect_filled(rect.shrink2(vec2(crate::chrome::Chrome::of(ui.ctx()).style.tabs_chevron_inset_x as f32, crate::chrome::Chrome::of(ui.ctx()).style.tabs_chevron_inset_y as f32)), chrome.metrics.radius_sm, fill);
    }
    paint_icon(
        ui,
        Icon::ChevronsRight,
        rect.center(),
        crate::chrome::Chrome::of(ui.ctx()).style.tabs_chevron_icon as f32,
        if hot { p.text } else { p.text_dim },
    );
    let mut chosen = None;
    Popup::menu(&response).show(|ui| {
        ui.set_min_width(crate::chrome::Chrome::of(ui.ctx()).style.tabs_chevron_menu_width as f32);
        for (index, name) in hidden {
            if crate::selectable(ui, false, name).clicked() {
                chosen = Some(*index);
            }
        }
    });
    chosen
}

/// One document tab.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DocTab {
    /// The Pro title ("name @ zoom% (mode/bits)").
    pub title: String,
    /// The Studio name and meta ("mode/bits").
    pub name: String,
    pub meta: String,
    pub dirty: bool,
    /// A background job's progress, 0 to 1.
    pub progress: Option<f32>,
    /// A leading status marker: the turn, in radians, of the half-filled
    /// disc drawn before the name (see [`split_marker`]).
    pub marker: Option<f32>,
}

/// The turn of the half-filled disc for a status glyph, or `None` when
/// `glyph` is not one. The shipped `contrast` icon is the right half filled,
/// which is ◑ as drawn; every other glyph is that icon turned a quarter or
/// half turn, clockwise on screen.
pub fn half_disc_turn(glyph: char) -> Option<f32> {
    match glyph {
        '\u{25D1}' => Some(0.0),             // ◑ right half
        '\u{25D3}' => Some(FRAC_PI_2),       // ◓ lower half
        '\u{25D0}' => Some(PI),              // ◐ left half
        '\u{25D2}' => Some(3.0 * FRAC_PI_2), // ◒ upper half
        _ => None,
    }
}

/// Splits a leading status glyph (a program's spinner) from its title: the
/// marker's turn and the rest of the title, with its leading spaces removed.
pub fn split_marker(title: &str) -> (Option<f32>, &str) {
    let mut chars = title.chars();
    match chars.next().and_then(half_disc_turn) {
        Some(turn) => (Some(turn), chars.as_str().trim_start()),
        None => (None, title),
    }
}

/// The room a tab's marker takes before its name, or zero without one.
fn marker_room(ui: &Ui, tab: &DocTab) -> f32 {
    if tab.marker.is_some() {
        (Chrome::of(ui.ctx()).style.tabs_marker as f32) + (Chrome::of(ui.ctx()).style.tabs_marker_gap as f32)
    } else {
        0.0
    }
}

/// What happened on a document strip this frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DocEvents {
    /// A tab was clicked (or chosen from the chevron menu).
    pub selected: Option<usize>,
    /// A tab's close box was clicked.
    pub closed: Option<usize>,
}

/// The document strip across `ui`'s width: `tabs`, `selected` drawn as the
/// selected one, `drop_target` outlined for a drag in progress.
pub fn documents(
    ui: &mut Ui,
    id: Id,
    tabs: &[DocTab],
    selected: usize,
    drop_target: Option<usize>,
) -> DocEvents {
    let chrome = Chrome::of(ui.ctx());
    let pro = is_strip(&chrome);
    let frame = if pro {
        Frame::new().fill(chrome.palette.tab_strip)
    } else {
        Frame::new()
            .fill(chrome.palette.canvas)
            .inner_margin(Margin { left: crate::chrome::Chrome::of(ui.ctx()).style.tabs_studio_margin_left as i8, right: crate::chrome::Chrome::of(ui.ctx()).style.tabs_studio_margin_right as i8, top: crate::chrome::Chrome::of(ui.ctx()).style.tabs_studio_margin_top as i8, bottom: crate::chrome::Chrome::of(ui.ctx()).style.tabs_studio_margin_bottom as i8 })
    };
    frame
        .show(ui, |ui| {
            let (strip, _) =
                ui.allocate_exact_size(vec2(ui.available_width(), crate::chrome::Chrome::of(ui.ctx()).style.tabs_strip as f32), Sense::hover());
            strip_tabs(ui, id, &chrome, strip, tabs, selected, drop_target)
        })
        .inner
}

fn strip_tabs(
    ui: &mut Ui,
    id: Id,
    chrome: &Chrome,
    strip: Rect,
    tabs: &[DocTab],
    selected: usize,
    drop_target: Option<usize>,
) -> DocEvents {
    let pro = is_strip(chrome);
    let (pad, minimum, gap) = if pro {
        (crate::chrome::Chrome::of(ui.ctx()).style.tabs_pro_pad as f32, crate::chrome::Chrome::of(ui.ctx()).style.tabs_pro_min as f32, 0.0)
    } else {
        (crate::chrome::Chrome::of(ui.ctx()).style.tabs_studio_pad as f32, crate::chrome::Chrome::of(ui.ctx()).style.tabs_pro_min as f32, crate::chrome::Chrome::of(ui.ctx()).style.tabs_studio_gap as f32)
    };
    let name_font = if pro {
        FontId::new(crate::chrome::Chrome::of(ui.ctx()).style.tabs_pro_size as f32, FontFamily::Proportional)
    } else {
        crate::fonts::bound(ui.ctx(), crate::fonts::medium(crate::chrome::Chrome::of(ui.ctx()).style.tabs_studio_name_size as f32))
    };
    let meta_font = FontId::new(crate::chrome::Chrome::of(ui.ctx()).style.tabs_studio_meta_size as f32, FontFamily::Proportional);
    let painter = ui.painter();
    let text_width = |text: &str, font: &FontId| {
        painter
            .layout_no_wrap(text.to_owned(), font.clone(), Color32::PLACEHOLDER)
            .size()
            .x
    };
    let names: Vec<String> = tabs
        .iter()
        .map(|t| {
            let base = if pro { &t.title } else { &t.name };
            if t.dirty {
                format!("{base}{DIRTY}")
            } else {
                base.clone()
            }
        })
        .collect();
    let naturals: Vec<f32> = tabs
        .iter()
        .zip(&names)
        .map(|(t, name)| {
            let meta = if pro || t.meta.is_empty() {
                0.0
            } else {
                crate::chrome::Chrome::of(ui.ctx()).style.tabs_studio_meta_gap as f32 + text_width(&t.meta, &meta_font)
            };
            text_width(name, &name_font) + meta + pad + gap + marker_room(ui, t)
        })
        .collect();
    let fit = fit_with_chevron(&naturals, minimum + gap, strip.width(), Some(selected), Chrome::of(ui.ctx()).style.tabs_chevron as f32);
    let mut events = DocEvents::default();
    let mut x = strip.left();
    let mut hidden = Vec::new();
    for (index, (tab, width)) in tabs.iter().zip(&fit.widths).enumerate() {
        let Some(width) = *width else {
            hidden.push((index, names[index].clone()));
            continue;
        };
        let rect = Rect::from_min_size(pos2(x, strip.top()), vec2(width - gap, strip.height()));
        x += width;
        let tab_events = doc_tab(
            ui,
            id.with(index),
            chrome,
            rect,
            tab,
            &names[index],
            (&name_font, &meta_font),
            index == selected,
        );
        if tab_events.0 {
            events.selected = Some(index);
        }
        if tab_events.1 {
            events.closed = Some(index);
        }
        if drop_target == Some(index) {
            let corner = if pro { 0 } else { chrome.metrics.radius_sm };
            ui.painter().rect_stroke(
                rect,
                corner,
                Stroke::new(crate::chrome::Chrome::of(ui.ctx()).style.tabs_drop_outline as f32, chrome.palette.accent),
                StrokeKind::Inside,
            );
        }
    }
    if fit.overflows() {
        let rect = Rect::from_min_size(pos2(x, strip.top()), vec2(crate::chrome::Chrome::of(ui.ctx()).style.tabs_chevron as f32, strip.height()));
        if let Some(index) = chevron(ui, id.with("overflow"), rect, &hidden) {
            events.selected = Some(index);
        }
    }
    events
}

/// Whether the style draws document tabs as a flush strip (Pro) rather than
/// as cards.
fn is_strip(chrome: &Chrome) -> bool {
    chrome.style.document_tabs == DocumentTabs::Strip
}

/// One document tab in `rect`: (clicked, close clicked).
#[expect(
    clippy::too_many_arguments,
    reason = "one tab's inputs, called from one place"
)]
fn doc_tab(
    ui: &mut Ui,
    id: Id,
    chrome: &Chrome,
    rect: Rect,
    tab: &DocTab,
    name: &str,
    (name_font, meta_font): (&FontId, &FontId),
    selected: bool,
) -> (bool, bool) {
    let (p, pro) = (&chrome.palette, is_strip(chrome));
    let response = ui.interact(rect, id, Sense::click());
    response
        .widget_info(|| WidgetInfo::selected(WidgetType::Button, ui.is_enabled(), selected, name));
    let (close_size, close_from_right, close_icon) = if pro {
        (crate::chrome::Chrome::of(ui.ctx()).style.tabs_pro_close as f32, crate::chrome::Chrome::of(ui.ctx()).style.tabs_pro_close_from_right as f32, crate::chrome::Chrome::of(ui.ctx()).style.tabs_pro_close_icon as f32)
    } else {
        (crate::chrome::Chrome::of(ui.ctx()).style.tabs_studio_close as f32, crate::chrome::Chrome::of(ui.ctx()).style.tabs_studio_close_from_right as f32, crate::chrome::Chrome::of(ui.ctx()).style.tabs_studio_close_icon as f32)
    };
    let close_rect = Rect::from_center_size(
        pos2(rect.right() - close_from_right, rect.center().y),
        vec2(close_size, close_size),
    );
    let close = ui.interact(close_rect, id.with("close"), Sense::click());
    close.widget_info(|| {
        WidgetInfo::labeled(
            WidgetType::Button,
            ui.is_enabled(),
            crate::strings::own_with("close-document", &[("name", name)]),
        )
    });
    let hovered = response.hovered() || close.hovered();
    let ppp = ui.pixels_per_point();
    let painter = ui.painter();
    let radius = if pro { 0 } else { chrome.metrics.radius_sm };
    if selected {
        painter.rect_filled(rect, radius, if pro { p.chrome } else { p.card });
        if !pro {
            painter.rect_stroke(
                rect,
                radius,
                Stroke::new(1.0, p.card_border),
                StrokeKind::Inside,
            );
        }
    } else if hovered {
        let alpha = if pro {
            crate::chrome::Chrome::of(ui.ctx()).style.tabs_pro_hover_alpha as f32
        } else {
            crate::chrome::Chrome::of(ui.ctx()).style.tabs_studio_hover_alpha as f32
        };
        painter.rect_filled(rect, radius, p.hover.gamma_multiply(alpha));
    }
    let left = if pro {
        crate::chrome::Chrome::of(ui.ctx()).style.tabs_pro_title_left as f32
    } else {
        crate::chrome::Chrome::of(ui.ctx()).style.tabs_studio_name_left as f32
    };
    let lead = marker_room(ui, tab);
    let text_room = close_rect.left() - (rect.left() + left + lead);
    let (galley, cut) = label(ui, name, name_font.clone(), text_room, 0.0);
    let ink = match (pro, selected) {
        (_, true) => p.text,
        (true, false) => p.text_faint,
        (false, false) => p.text_dim,
    };
    if let Some(turn) = tab.marker {
        // A ring with a solid half-disc inside it, touching the ring's inner edge
        // (no gap). The half is turned in place; the ring never moves.
        let centre = pos2(rect.left() + left + crate::chrome::Chrome::of(ui.ctx()).style.tabs_marker as f32 / 2.0, rect.center().y).round_to_pixels(ppp);
        let scale = crate::chrome::Chrome::of(ui.ctx()).style.tabs_marker as f32 / 24.0;
        let ring = 10.0 * scale;
        let stroke = 2.0 * scale;
        painter.circle_stroke(centre, ring, Stroke::new(stroke, ink));
        let inner = ring - stroke / 2.0;
        let steps = 16;
        let mut points = Vec::with_capacity(steps + 2);
        points.push(centre);
        for step in 0..=steps {
            let angle = -FRAC_PI_2 + turn + PI * step as f32 / steps as f32;
            points.push(centre + vec2(angle.cos(), angle.sin()) * inner);
        }
        painter.add(Shape::convex_polygon(points, ink, Stroke::NONE));
    }
    let name_at = pos2(
        rect.left() + left + lead,
        rect.center().y - galley.size().y / 2.0,
    )
    .round_to_pixels(ppp);
    let name_right = name_at.x + galley.size().x;
    painter.galley(name_at, galley, ink);
    if !pro && !tab.meta.is_empty() && !cut {
        let meta = painter.layout_no_wrap(tab.meta.clone(), meta_font.clone(), p.text_faint);
        if name_right + crate::chrome::Chrome::of(ui.ctx()).style.tabs_studio_meta_gap as f32 + meta.size().x <= close_rect.left() {
            let at = pos2(
                name_right + crate::chrome::Chrome::of(ui.ctx()).style.tabs_studio_meta_gap as f32,
                rect.center().y - meta.size().y / 2.0,
            )
            .round_to_pixels(ppp);
            painter.galley(at, meta, p.text_faint);
        }
    }
    if pro {
        let x = painter.round_to_pixel_center(rect.right() - 0.5);
        painter.vline(x, rect.y_range(), Stroke::new(1.0, p.separator));
    } else if close.hovered() {
        painter.rect_filled(close_rect, crate::chrome::Chrome::of(ui.ctx()).style.tabs_studio_close_radius as f32, p.hover);
    }
    if let Some(progress) = tab.progress {
        let line = Rect::from_min_size(
            rect.left_bottom() - vec2(0.0, crate::chrome::Chrome::of(ui.ctx()).style.tabs_progress as f32),
            vec2(rect.width() * progress.clamp(0.0, 1.0), crate::chrome::Chrome::of(ui.ctx()).style.tabs_progress as f32),
        );
        painter.rect_filled(line, 0.0, p.accent);
    }
    paint_icon(
        ui,
        Icon::X,
        close_rect.center(),
        close_icon,
        if close.hovered() {
            p.text
        } else {
            p.text_faint
        },
    );
    let response = if cut {
        response.on_hover_text(name)
    } else {
        response
    };
    (response.clicked(), close.clicked())
}


/// The installed value of the former `CHEVRON` constant.
pub fn chevron_of(ctx: &egui::Context) -> f32 { crate::chrome::Chrome::of(ctx).style.tabs_chevron as f32 }

/// The installed value of the former `STRIP` constant.
pub fn strip_of(ctx: &egui::Context) -> f32 { crate::chrome::Chrome::of(ctx).style.tabs_strip as f32 }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_glyphs_split_off_as_turns() {
        assert_eq!(split_marker("\u{25D1} build"), (Some(0.0), "build"));
        assert_eq!(split_marker("\u{25D0}  build"), (Some(PI), "build"));
        assert_eq!(split_marker("\u{25D3} build"), (Some(FRAC_PI_2), "build"));
        assert_eq!(
            split_marker("\u{25D2} build"),
            (Some(3.0 * FRAC_PI_2), "build")
        );
    }

    #[test]
    fn other_titles_keep_their_text() {
        assert_eq!(split_marker("build"), (None, "build"));
        assert_eq!(split_marker("\u{25A1} build"), (None, "\u{25A1} build"));
        assert_eq!(split_marker(""), (None, ""));
    }

    #[test]
    fn tabs_that_fit_keep_their_natural_widths() {
        let fit = fit(&[60.0, 80.0], 40.0, 200.0, Some(0));
        assert_eq!(fit.widths, vec![Some(60.0), Some(80.0)]);
        assert!(!fit.overflows());
    }

    #[test]
    fn the_widest_shrink_first_to_a_common_cap() {
        let fit = fit(&[50.0, 100.0, 150.0], 40.0, 240.0, Some(0));
        let widths: Vec<f32> = fit.widths.iter().map(|w| w.unwrap().round()).collect();
        assert_eq!(widths, [50.0, 95.0, 95.0], "50 + 2 x 95 = 240");
    }

    #[test]
    fn tabs_never_shrink_below_their_floor_and_then_overflow() {
        // Four 40 pt floors need 160; 130 leaves 112 after the chevron.
        let fit = fit(&[90.0, 90.0, 90.0, 90.0], 40.0, 130.0, Some(3));
        assert_eq!(fit.widths.iter().filter(|w| w.is_some()).count(), 2);
        assert!(fit.widths[3].is_some(), "the selected tab stays");
        assert!(fit.widths.iter().flatten().all(|w| *w >= 40.0));
        assert!(fit.widths.iter().flatten().sum::<f32>() <= 130.0 - crate::style::default_tokens().tabs_chevron as f32 + 0.01);
    }

    #[test]
    fn a_narrow_natural_width_is_its_own_floor() {
        let fit = fit(&[30.0, 200.0], 40.0, 100.0, None);
        assert_eq!(fit.widths[0], Some(30.0));
    }
}
