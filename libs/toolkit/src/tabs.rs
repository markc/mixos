// SPDX-License-Identifier: MIT OR Apache-2.0
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
    Color32, FontFamily, FontId, Frame, Galley, Id, Margin, Popup, Rect, Sense, Stroke, StrokeKind,
    Ui, WidgetInfo, WidgetType, pos2, vec2,
};
use std::sync::Arc;

/// The overflow chevron's width, icon size, hover opacity and hover inset
/// (§3.10).
pub const CHEVRON: f32 = 18.0;
const CHEVRON_ICON: f32 = 12.0;
const CHEVRON_HOVER_ALPHA: f32 = 0.6;
const CHEVRON_INSET: egui::Vec2 = vec2(1.0, 3.0);

/// The chevron popup's least width (§3.10).
const CHEVRON_MENU_WIDTH: f32 = 140.0;

/// Pro document strip (§3.12).
pub const STRIP: f32 = 26.0;
const PRO_SIZE: f32 = 11.5;
const PRO_PAD: f32 = 42.0;
const PRO_MIN: f32 = 72.0;
const PRO_TITLE_LEFT: f32 = 16.0;
const PRO_CLOSE: f32 = 14.0;
const PRO_CLOSE_FROM_RIGHT: f32 = 13.0;
const PRO_CLOSE_ICON: f32 = 10.0;
const PRO_HOVER_ALPHA: f32 = 0.35;

/// Studio and Classic document strip (§3.12).
const STUDIO_MARGIN: Margin = Margin {
    left: 8,
    right: 8,
    top: 6,
    bottom: 4,
};
const STUDIO_GAP: f32 = 4.0;
const STUDIO_NAME_SIZE: f32 = 12.5;
const STUDIO_META_SIZE: f32 = 10.5;
const STUDIO_PAD: f32 = 48.0;
const STUDIO_NAME_LEFT: f32 = 10.0;
const STUDIO_META_GAP: f32 = 6.0;
const STUDIO_CLOSE: f32 = 16.0;
const STUDIO_CLOSE_FROM_RIGHT: f32 = 12.0;
const STUDIO_CLOSE_ICON: f32 = 11.0;
const STUDIO_CLOSE_RADIUS: f32 = 4.0;
const STUDIO_HOVER_ALPHA: f32 = 0.5;

/// The drop-target outline (§3.12).
const DROP_OUTLINE: f32 = 1.5;

/// The progress underline of a background job. Its height is undetermined
/// by the specification (§5.4): 2 pt in `accent`, the drop line's weight.
const PROGRESS: f32 = 2.0;

/// The dirty marker after a document's name, as the window title has it.
pub const DIRTY: &str = "  •";

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
        room = available - CHEVRON;
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
        let fill = p.hover.gamma_multiply(CHEVRON_HOVER_ALPHA);
        ui.painter()
            .rect_filled(rect.shrink2(CHEVRON_INSET), chrome.metrics.radius_sm, fill);
    }
    paint_icon(
        ui,
        Icon::ChevronsRight,
        rect.center(),
        CHEVRON_ICON,
        if hot { p.text } else { p.text_dim },
    );
    let mut chosen = None;
    Popup::menu(&response).show(|ui| {
        ui.set_min_width(CHEVRON_MENU_WIDTH);
        for (index, name) in hidden {
            if ui.selectable_label(false, name).clicked() {
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
            .inner_margin(STUDIO_MARGIN)
    };
    frame
        .show(ui, |ui| {
            let (strip, _) =
                ui.allocate_exact_size(vec2(ui.available_width(), STRIP), Sense::hover());
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
        (PRO_PAD, PRO_MIN, 0.0)
    } else {
        (STUDIO_PAD, PRO_MIN, STUDIO_GAP)
    };
    let name_font = if pro {
        FontId::new(PRO_SIZE, FontFamily::Proportional)
    } else {
        crate::fonts::bound(ui.ctx(), crate::fonts::medium(STUDIO_NAME_SIZE))
    };
    let meta_font = FontId::new(STUDIO_META_SIZE, FontFamily::Proportional);
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
                STUDIO_META_GAP + text_width(&t.meta, &meta_font)
            };
            text_width(name, &name_font) + meta + pad + gap
        })
        .collect();
    let fit = fit(&naturals, minimum + gap, strip.width(), Some(selected));
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
                Stroke::new(DROP_OUTLINE, chrome.palette.accent),
                StrokeKind::Inside,
            );
        }
    }
    if fit.overflows() {
        let rect = Rect::from_min_size(pos2(x, strip.top()), vec2(CHEVRON, strip.height()));
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
        (PRO_CLOSE, PRO_CLOSE_FROM_RIGHT, PRO_CLOSE_ICON)
    } else {
        (STUDIO_CLOSE, STUDIO_CLOSE_FROM_RIGHT, STUDIO_CLOSE_ICON)
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
            PRO_HOVER_ALPHA
        } else {
            STUDIO_HOVER_ALPHA
        };
        painter.rect_filled(rect, radius, p.hover.gamma_multiply(alpha));
    }
    let left = if pro {
        PRO_TITLE_LEFT
    } else {
        STUDIO_NAME_LEFT
    };
    let text_room = close_rect.left() - (rect.left() + left);
    let (galley, cut) = label(ui, name, name_font.clone(), text_room, 0.0);
    let ink = match (pro, selected) {
        (_, true) => p.text,
        (true, false) => p.text_faint,
        (false, false) => p.text_dim,
    };
    let name_at =
        pos2(rect.left() + left, rect.center().y - galley.size().y / 2.0).round_to_pixels(ppp);
    let name_right = name_at.x + galley.size().x;
    painter.galley(name_at, galley, ink);
    if !pro && !tab.meta.is_empty() && !cut {
        let meta = painter.layout_no_wrap(tab.meta.clone(), meta_font.clone(), p.text_faint);
        if name_right + STUDIO_META_GAP + meta.size().x <= close_rect.left() {
            let at = pos2(
                name_right + STUDIO_META_GAP,
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
        painter.rect_filled(close_rect, STUDIO_CLOSE_RADIUS, p.hover);
    }
    if let Some(progress) = tab.progress {
        let line = Rect::from_min_size(
            rect.left_bottom() - vec2(0.0, PROGRESS),
            vec2(rect.width() * progress.clamp(0.0, 1.0), PROGRESS),
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

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(fit.widths.iter().flatten().sum::<f32>() <= 130.0 - CHEVRON + 0.01);
    }

    #[test]
    fn a_narrow_natural_width_is_its_own_floor() {
        let fit = fit(&[30.0, 200.0], 40.0, 100.0, None);
        assert_eq!(fit.widths[0], Some(30.0));
    }
}
