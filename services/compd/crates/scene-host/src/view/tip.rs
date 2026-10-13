// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Hover tips: a node's `tooltip` port (`{title, subtitle?, lines?}`), the
//! card it lays out to, and when it shows.
//!
//! The panel is a thin edge surface, so a tip cannot draw inside it: the
//! view decides *when* and *what* ([`Hover::pass`]) and measures the card
//! with its own fonts ([`card`]); the host draws the card on a separate
//! click-through surface beside the panel (`crate::tooltip`).
//!
//! Timing follows Plasma's panel tips: a tip shows after the pointer rests
//! on a node for [`DELAY`]; while one is up (or for [`STICKY`] after it
//! closed) moving to another node switches at once; leaving closes it; a
//! press closes it until the pointer moves to another node.

use std::time::{Duration, Instant};

use egui::{Color32, FontId, Rect};
use serde_json::Value;

/// How long the pointer rests on a node before its tip shows.
pub const DELAY: Duration = Duration::from_millis(500);
/// After a tip closes, another node's tip shows at once for this long.
pub const STICKY: Duration = Duration::from_millis(400);
/// The card's inner padding (logical px).
pub const PAD_X: f32 = 10.0;
pub const PAD_Y: f32 = 8.0;
/// The widest a card grows; longer lines elide at the end.
pub const MAX_TEXT_WIDTH: f32 = 420.0;
/// The most detail lines a card shows.
pub const MAX_LINES: usize = 8;
/// Gap between the title block and the lines below it.
const BLOCK_GAP: f32 = 4.0;
/// The largest window thumbnail box (logical px), and the gap under it.
pub const THUMB_MAX: (f32, f32) = (240.0, 150.0);
const THUMB_GAP: f32 = 8.0;

/// A live preview of a window on the card (Plasma's task tips): the window's
/// `{id, generation}` fence and its size (for the box's aspect).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Thumbnail {
    pub id: u64,
    pub generation: u64,
    pub w: f32,
    pub h: f32,
}

/// What a tip says.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Tip {
    pub title: String,
    pub subtitle: String,
    pub lines: Vec<String>,
    pub thumbnail: Option<Thumbnail>,
}

impl Tip {
    /// The tip a `tooltip` port value describes, read leniently: a string is
    /// a title; an object gives `title`, `subtitle` and `lines` (strings;
    /// anything else is ignored), and `thumbnail` ({id, generation, w, h}, a
    /// window to preview). `None` when nothing would show.
    pub fn from_port(value: &Value) -> Option<Tip> {
        let clean = |s: &str| -> String {
            s.chars()
                .filter(|c| !c.is_control())
                .take(400)
                .collect::<String>()
                .trim()
                .to_owned()
        };
        let tip = match value {
            Value::String(title) => Tip {
                title: clean(title),
                ..Tip::default()
            },
            Value::Object(map) => Tip {
                title: map
                    .get("title")
                    .and_then(Value::as_str)
                    .map(clean)
                    .unwrap_or_default(),
                subtitle: map
                    .get("subtitle")
                    .and_then(Value::as_str)
                    .map(clean)
                    .unwrap_or_default(),
                lines: map
                    .get("lines")
                    .and_then(Value::as_array)
                    .map(|lines| {
                        lines
                            .iter()
                            .filter_map(Value::as_str)
                            .map(clean)
                            .filter(|line| !line.is_empty())
                            .take(MAX_LINES)
                            .collect()
                    })
                    .unwrap_or_default(),
                thumbnail: map.get("thumbnail").and_then(Thumbnail::from_port),
            },
            _ => return None,
        };
        (!tip.title.is_empty() || !tip.subtitle.is_empty() || !tip.lines.is_empty()).then_some(tip)
    }
}

impl Thumbnail {
    /// `{id, generation, w, h, decorated?}`: whole non-negative ids, a
    /// positive content size. `decorated: true` (server-side chrome) grows the
    /// size by the installed theme's chrome, so the box has the frame's
    /// aspect, which is what compd fits into it.
    fn from_port(value: &Value) -> Option<Thumbnail> {
        let id = value.get("id")?.as_f64()?;
        let generation = value.get("generation")?.as_f64()?;
        let w = value.get("w")?.as_f64()? as f32;
        let h = value.get("h")?.as_f64()? as f32;
        let whole = |v: f64| v >= 0.0 && v.fract() == 0.0 && v < 9.0e15;
        if !(whole(id) && whole(generation) && w.is_finite() && h.is_finite() && w > 0.0 && h > 0.0)
        {
            return None;
        }
        let decorated = value.get("decorated").and_then(Value::as_bool) == Some(true);
        let (w, h) = if decorated {
            framed(
                w,
                h,
                decor::window::installed().map(|t| decor::DecoExtents::of(&t.deco)),
            )
        } else {
            (w, h)
        };
        Some(Thumbnail {
            id: id as u64,
            generation: generation as u64,
            w,
            h,
        })
    }
}

/// A window's content size grown by its server-side chrome `extents` (none
/// without an installed theme): the frame a thumbnail shows.
fn framed(w: f32, h: f32, extents: Option<decor::DecoExtents>) -> (f32, f32) {
    extents.map_or((w, h), |e| (w + e.left + e.right, h + e.top + e.bottom))
}

impl Thumbnail {
    /// The box the window fits into: its aspect, at most [`THUMB_MAX`].
    pub fn fitted(&self) -> (f32, f32) {
        let s = (THUMB_MAX.0 / self.w).min(THUMB_MAX.1 / self.h);
        ((self.w * s).round().max(1.0), (self.h * s).round().max(1.0))
    }
}

/// A tip ready to show: what it says, the node it belongs to (surface-local
/// logical px), the card's size (logical px, padding included) and where
/// on the card the window thumbnail goes (card-local logical px).
#[derive(Clone, Debug, PartialEq)]
pub struct TipAt {
    pub tip: Tip,
    pub anchor: (f32, f32, f32, f32),
    pub size: (f32, f32),
    pub thumb: Option<(f32, f32, f32, f32)>,
}

/// One laid-out line of a card.
#[derive(Clone, Debug, PartialEq)]
pub struct CardLine {
    pub text: String,
    pub size: f32,
    pub bold: bool,
    /// Secondary text (subtitle, detail lines) draws dimmer.
    pub dim: bool,
    /// Top of the line inside the card (logical px).
    pub y: f32,
    pub height: f32,
}

/// A card's lines and size, at UI text size `ui` (points).
#[derive(Clone, Debug, PartialEq)]
pub struct Card {
    pub lines: Vec<CardLine>,
    pub size: (f32, f32),
    /// The thumbnail box, card-local (logical px): above the text, centred.
    pub thumb: Option<(f32, f32, f32, f32)>,
}

/// Lay a tip out. `measure(text, font)` is the text's (width, height) in the
/// fonts the card will draw with; the same function measures in the view
/// (to size the surface) and on the card (to draw it), so both agree.
pub fn card(tip: &Tip, ui: f32, measure: &dyn Fn(&str, &FontId) -> (f32, f32)) -> Card {
    let detail = (ui - 1.0).max(9.0);
    let mut specs: Vec<(String, f32, bool, bool, bool)> = Vec::new();
    if !tip.title.is_empty() {
        specs.push((tip.title.clone(), ui + 1.0, true, false, false));
    }
    if !tip.subtitle.is_empty() {
        specs.push((tip.subtitle.clone(), ui, false, true, false));
    }
    for (index, line) in tip.lines.iter().enumerate() {
        specs.push((
            line.clone(),
            detail,
            false,
            true,
            index == 0 && !specs.is_empty(),
        ));
    }
    let mut lines = Vec::new();
    let mut y = PAD_Y;
    let mut widest: f32 = 0.0;
    // The preview sits on top, the text under it (Plasma's task tips).
    let thumb_size = tip.thumbnail.as_ref().map(Thumbnail::fitted);
    if let Some((w, h)) = thumb_size {
        widest = widest.max(w);
        y += h + THUMB_GAP;
    }
    for (text, size, bold, dim, gap) in specs {
        if gap {
            y += BLOCK_GAP;
        }
        let font = super::paint::font(size, false, bold);
        let text = elide_end(&text, &font, MAX_TEXT_WIDTH, measure);
        let (width, height) = measure(&text, &font);
        let height = height.max(size * 1.2);
        widest = widest.max(width);
        lines.push(CardLine {
            text,
            size,
            bold,
            dim,
            y,
            height,
        });
        y += height + 1.0;
    }
    let width = (widest + PAD_X * 2.0).ceil();
    let height = if lines.is_empty() {
        // A thumbnail alone: no gap under it.
        (y - THUMB_GAP + PAD_Y).ceil()
    } else {
        (y - 1.0 + PAD_Y).ceil()
    };
    Card {
        lines,
        size: (width, height),
        thumb: thumb_size.map(|(w, h)| (((width - w) / 2.0).floor(), PAD_Y, w, h)),
    }
}

/// `text` cut at the end with an ellipsis until it fits `width`.
fn elide_end(
    text: &str,
    font: &FontId,
    width: f32,
    measure: &dyn Fn(&str, &FontId) -> (f32, f32),
) -> String {
    if measure(text, font).0 <= width {
        return text.to_owned();
    }
    let chars: Vec<char> = text.chars().collect();
    let (mut low, mut high) = (0usize, chars.len());
    // The longest prefix that fits with its ellipsis (binary search).
    while low < high {
        let mid = (low + high).div_ceil(2);
        let candidate: String = chars[..mid].iter().chain(std::iter::once(&'…')).collect();
        if measure(&candidate, font).0 <= width {
            low = mid;
        } else {
            high = mid - 1;
        }
    }
    chars[..low].iter().chain(std::iter::once(&'…')).collect()
}

/// A hovered node that has a tip, as one paint pass found it.
#[derive(Clone, Debug)]
pub struct Candidate {
    pub key: String,
    pub rect: Rect,
    pub tip: Tip,
}

/// The view's tip state between passes.
#[derive(Debug, Default)]
pub struct Hover {
    /// The node the pointer rests on and since when.
    resting: Option<(String, Instant)>,
    /// The tip last sent to the host (`None` = hidden).
    sent: Option<(String, TipAt)>,
    /// When the last tip closed (for [`STICKY`]).
    closed: Option<Instant>,
    /// A press closed this node's tip; it stays closed until the pointer
    /// rests on another node.
    suppressed: Option<String>,
}

/// What a pass asks of the host and the clock.
#[derive(Debug, PartialEq)]
pub enum Outcome {
    /// Nothing changes.
    Same,
    /// Show (or move/replace) the tip.
    Show(TipAt),
    /// Hide the tip.
    Hide,
    /// Nothing yet: wake the view again after this long.
    Wait(Duration),
}

impl Hover {
    /// One pass: `candidate` is the innermost hovered node with a tip,
    /// `pressed` whether a pointer button went down this pass. `layout_of`
    /// measures a tip's card (only called when one shows).
    pub fn pass(
        &mut self,
        now: Instant,
        candidate: Option<Candidate>,
        pressed: bool,
        layout_of: &dyn Fn(&Tip) -> Card,
    ) -> Outcome {
        let Some(candidate) = candidate else {
            self.resting = None;
            self.suppressed = None;
            return self.hide(now);
        };
        if pressed {
            self.suppressed = Some(candidate.key.clone());
            self.resting = None;
            return self.hide(now);
        }
        if self.suppressed.as_deref() == Some(candidate.key.as_str()) {
            return Outcome::Same;
        }
        self.suppressed = None;
        let since = match &self.resting {
            Some((key, since)) if *key == candidate.key => *since,
            _ => {
                self.resting = Some((candidate.key.clone(), now));
                now
            }
        };
        let quick = self.sent.is_some()
            || self
                .closed
                .is_some_and(|closed| now.saturating_duration_since(closed) < STICKY);
        let rested = now.saturating_duration_since(since);
        if !quick && rested < DELAY {
            return Outcome::Wait(DELAY - rested);
        }
        let r = candidate.rect;
        let card = layout_of(&candidate.tip);
        let at = TipAt {
            size: card.size,
            thumb: card.thumb,
            tip: candidate.tip,
            anchor: (r.min.x, r.min.y, r.width(), r.height()),
        };
        if self
            .sent
            .as_ref()
            .is_some_and(|(key, sent)| *key == candidate.key && *sent == at)
        {
            return Outcome::Same;
        }
        self.sent = Some((candidate.key, at.clone()));
        Outcome::Show(at)
    }

    fn hide(&mut self, now: Instant) -> Outcome {
        if self.sent.take().is_some() {
            self.closed = Some(now);
            Outcome::Hide
        } else {
            Outcome::Same
        }
    }

    /// The tip currently shown, if any.
    pub fn shown(&self) -> Option<&TipAt> {
        self.sent.as_ref().map(|(_, at)| at)
    }
}

/// Text colour for a card line: the ink, dimmed for secondary lines.
pub fn line_colour(ink: Color32, dim: bool) -> Color32 {
    if dim { ink.gamma_multiply(0.72) } else { ink }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A decorated window's box takes its frame's size: content plus chrome.
    #[test]
    fn a_decorated_thumbnail_is_sized_from_its_frame() {
        let extents = decor::DecoExtents {
            left: 1.0,
            right: 1.0,
            top: 30.0,
            bottom: 1.0,
        };
        assert_eq!(framed(800.0, 600.0, Some(extents)), (802.0, 631.0));
        assert_eq!(framed(800.0, 600.0, None), (800.0, 600.0));
        // Without an installed theme (here) `decorated` changes nothing.
        let port = json!({"id": 1, "generation": 2, "w": 800, "h": 600, "decorated": true});
        let t = Thumbnail::from_port(&port).unwrap();
        assert_eq!((t.w, t.h), (800.0, 600.0));
    }

    /// A monospace stand-in: 7 px per character, 1.2 x size high.
    fn measure(text: &str, font: &FontId) -> (f32, f32) {
        (text.chars().count() as f32 * 7.0, font.size * 1.2)
    }

    fn candidate(key: &str, title: &str) -> Candidate {
        Candidate {
            key: key.into(),
            rect: Rect::from_min_size(egui::pos2(10.0, 4.0), egui::vec2(40.0, 40.0)),
            tip: Tip {
                title: title.into(),
                ..Tip::default()
            },
        }
    }

    fn size_of(_: &Tip) -> Card {
        Card {
            lines: Vec::new(),
            size: (100.0, 30.0),
            thumb: None,
        }
    }

    #[test]
    fn ports_read_leniently() {
        assert_eq!(
            Tip::from_port(&json!("Mail")).unwrap().title,
            "Mail",
            "a string is a title"
        );
        let tip = Tip::from_port(&json!({"title": " Term ", "subtitle": "~/src",
            "lines": ["one", 2, "", "two"], "extra": true}))
        .unwrap();
        assert_eq!(
            (tip.title.as_str(), tip.subtitle.as_str(), tip.lines),
            ("Term", "~/src", vec!["one".to_owned(), "two".to_owned()])
        );
        assert_eq!(Tip::from_port(&json!({"title": "", "lines": []})), None);
        assert_eq!(Tip::from_port(&json!(3)), None);
        assert_eq!(
            Tip::from_port(&json!({"title": "a\nb\u{7}c"}))
                .unwrap()
                .title,
            "abc",
            "control characters are dropped"
        );
        let many = json!({"lines": (0..20).map(|i| i.to_string()).collect::<Vec<_>>()});
        assert_eq!(Tip::from_port(&many).unwrap().lines.len(), MAX_LINES);
    }

    #[test]
    fn a_card_stacks_title_subtitle_and_lines() {
        let tip = Tip {
            title: "Workspace 2".into(),
            subtitle: "Mail".into(),
            lines: vec!["Thunderbird".into(), "Firefox".into()],
            thumbnail: None,
        };
        let card = card(&tip, 13.0, &measure);
        assert_eq!(card.lines.len(), 4);
        assert!(card.lines[0].bold && !card.lines[0].dim && card.lines[0].size == 14.0);
        assert!(!card.lines[1].bold && card.lines[1].dim);
        assert!(card.lines[2].size == 12.0 && card.lines[2].dim);
        let tops: Vec<f32> = card.lines.iter().map(|line| line.y).collect();
        assert!(tops.windows(2).all(|pair| pair[1] > pair[0]), "{tops:?}");
        // The block gap sits between the subtitle and the first line.
        assert!(tops[2] - tops[1] > tops[3] - tops[2]);
        assert_eq!(card.size.0, (11.0 * 7.0 + PAD_X * 2.0).ceil());
        let last = card.lines.last().unwrap();
        assert_eq!(card.size.1, (last.y + last.height + PAD_Y).ceil());
    }

    #[test]
    fn long_lines_elide_at_the_end_within_the_widest_card() {
        let tip = Tip {
            title: "x".repeat(200),
            ..Tip::default()
        };
        let card = card(&tip, 13.0, &measure);
        assert!(card.lines[0].text.ends_with('…'));
        assert!(card.size.0 <= MAX_TEXT_WIDTH + PAD_X * 2.0);
    }

    #[test]
    fn a_tip_shows_after_the_delay_then_follows_the_pointer_at_once() {
        let mut hover = Hover::default();
        let t0 = Instant::now();
        // Resting: wait out the delay, never a frame loop.
        assert_eq!(
            hover.pass(t0, Some(candidate("a", "A")), false, &size_of),
            Outcome::Wait(DELAY)
        );
        let half = t0 + DELAY / 2;
        assert_eq!(
            hover.pass(half, Some(candidate("a", "A")), false, &size_of),
            Outcome::Wait(DELAY - DELAY / 2)
        );
        let shown = hover.pass(t0 + DELAY, Some(candidate("a", "A")), false, &size_of);
        let Outcome::Show(at) = shown else {
            panic!("{shown:?}")
        };
        assert_eq!(
            (at.anchor, at.size),
            ((10.0, 4.0, 40.0, 40.0), (100.0, 30.0))
        );
        // Unchanged: nothing to send.
        assert_eq!(
            hover.pass(t0 + DELAY * 2, Some(candidate("a", "A")), false, &size_of),
            Outcome::Same
        );
        // Another node while one is up: at once.
        assert!(matches!(
            hover.pass(t0 + DELAY * 2, Some(candidate("b", "B")), false, &size_of),
            Outcome::Show(at) if at.tip.title == "B"
        ));
        // Leaving closes it.
        let left = t0 + DELAY * 3;
        assert_eq!(hover.pass(left, None, false, &size_of), Outcome::Hide);
        assert_eq!(hover.pass(left, None, false, &size_of), Outcome::Same);
        // Back within STICKY: at once; after it, the delay again.
        assert!(matches!(
            hover.pass(
                left + STICKY / 2,
                Some(candidate("c", "C")),
                false,
                &size_of
            ),
            Outcome::Show(_)
        ));
        let gone = left + STICKY;
        assert_eq!(hover.pass(gone, None, false, &size_of), Outcome::Hide);
        assert_eq!(
            hover.pass(
                gone + STICKY * 2,
                Some(candidate("d", "D")),
                false,
                &size_of
            ),
            Outcome::Wait(DELAY)
        );
    }

    #[test]
    fn a_press_closes_the_tip_until_another_node() {
        let mut hover = Hover::default();
        let t0 = Instant::now();
        hover.pass(t0, Some(candidate("a", "A")), false, &size_of);
        assert!(matches!(
            hover.pass(t0 + DELAY, Some(candidate("a", "A")), false, &size_of),
            Outcome::Show(_)
        ));
        assert_eq!(
            hover.pass(t0 + DELAY, Some(candidate("a", "A")), true, &size_of),
            Outcome::Hide
        );
        // Still on the pressed node, however long: stays closed.
        assert_eq!(
            hover.pass(t0 + DELAY * 4, Some(candidate("a", "A")), false, &size_of),
            Outcome::Same
        );
        assert!(hover.shown().is_none());
        // Another node rests its own delay (STICKY ran out long ago).
        assert_eq!(
            hover.pass(t0 + DELAY * 5, Some(candidate("b", "B")), false, &size_of),
            Outcome::Wait(DELAY)
        );
    }

    #[test]
    fn a_changed_tip_on_the_same_node_is_resent() {
        let mut hover = Hover::default();
        let t0 = Instant::now();
        hover.pass(t0, Some(candidate("a", "A")), false, &size_of);
        hover.pass(t0 + DELAY, Some(candidate("a", "A")), false, &size_of);
        assert!(matches!(
            hover.pass(t0 + DELAY, Some(candidate("a", "A2")), false, &size_of),
            Outcome::Show(at) if at.tip.title == "A2"
        ));
    }
}
