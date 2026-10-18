// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! A [`Document`] drawn into egui, every colour and size from the theme.
//!
//! Each run of inline text is laid out as one galley and added as one
//! selectable label. Links are ranges of that galley: the pointer's
//! character decides whether it is over one, so a link wraps with its
//! paragraph and the paragraph stays one selection.

use std::ops::Range;
use std::sync::Arc;

use egui::text::{CCursor, LayoutJob, TextFormat};
use egui::{
    Color32, CursorIcon, FontId, Frame, Galley, Label, Layout, Margin, Rect, ScrollArea, Sense,
    Stroke, TextStyle, Ui, Vec2, pos2, vec2,
};

use super::{Block, Document, Inline, Item, Link};
use crate::chrome::{Chrome, Palette};
use crate::strings::{own, own_with};

/// Body line height, in multiples of the text size.
const LINE: f32 = 1.45;

/// Heading line height, in multiples of the heading size.
const HEADING_LINE: f32 = 1.25;

/// Heading sizes for levels 1 to 6, in multiples of the `Heading` style.
const HEADING_SCALE: [f32; 6] = [1.6, 1.3, 1.1, 1.0, 0.9, 0.85];

/// The widest a text column grows, in multiples of the body size (about
/// 75 characters of Inter): longer lines are hard to read.
const MEASURE: f32 = 46.0;

/// Space between blocks, before a heading, and after one, in multiples of
/// the body size.
const GAP: f32 = 0.75;
const GAP_BEFORE_HEADING: f32 = 1.2;
const GAP_AFTER_HEADING: f32 = 0.35;

/// Space between list items, in multiples of the body size.
const ITEM_GAP: f32 = 0.25;

/// A list's marker gutter and a quote's indent, in multiples of the body
/// size.
const GUTTER: f32 = 1.6;
const QUOTE_INDENT: f32 = 1.0;

/// A quote's rule width, and a code block's padding in multiples of the
/// body size.
const QUOTE_RULE: f32 = 3.0;
const CODE_PADDING: Vec2 = vec2(0.8, 0.6);

/// Inline code runs a little smaller than the text round it.
const INLINE_CODE_SCALE: f32 = 0.92;

/// A bullet's radius, in multiples of the body size.
const BULLET: f32 = 0.17;

/// Draw `document` into `ui`; returns the link activated this frame. An
/// [`openable`](super::openable) link has also been handed to
/// [`egui::Context::open_url`].
pub fn show(ui: &mut Ui, document: &Document) -> Option<String> {
    let mut view = View::new(ui);
    let width = ui.available_width().min(view.body.size * MEASURE);
    ui.vertical(|ui| {
        ui.set_max_width(width);
        ui.spacing_mut().item_spacing.y = 0.0;
        view.blocks(ui, &document.blocks, 0, false);
    });
    view.clicked
}

/// The text a span draws with.
#[derive(Clone)]
struct Base {
    font: FontId,
    /// The face for strong text at the same size.
    strong: FontId,
    color: Color32,
    line: f32,
    wrap: bool,
}

struct View {
    body: FontId,
    mono: FontId,
    heading: f32,
    palette: Palette,
    link: Color32,
    code_bg: Color32,
    radius: egui::CornerRadius,
    /// The text colour of the block being drawn (dimmer inside a quote).
    color: Color32,
    clicked: Option<String>,
}

impl View {
    fn new(ui: &Ui) -> Self {
        let style = ui.style();
        let palette = Chrome::of(ui.ctx()).palette;
        Self {
            body: TextStyle::Body.resolve(style),
            mono: TextStyle::Monospace.resolve(style),
            heading: TextStyle::Heading.resolve(style).size,
            link: style.visuals.hyperlink_color,
            code_bg: style.visuals.code_bg_color,
            radius: style.visuals.widgets.noninteractive.corner_radius,
            color: palette.text,
            palette,
            clicked: None,
        }
    }

    fn em(&self, n: f32) -> f32 {
        self.body.size * n
    }

    fn strong(&self, ui: &Ui, size: f32) -> FontId {
        crate::fonts::bound(ui.ctx(), crate::fonts::heading(size))
    }

    fn body_base(&self, ui: &Ui) -> Base {
        Base {
            font: self.body.clone(),
            strong: self.strong(ui, self.body.size),
            color: self.color,
            line: self.em(LINE),
            wrap: true,
        }
    }

    /// `blocks` with the theme's spacing between them; `depth` is the list
    /// nesting, for the bullet shape. Inside a list item the blocks sit as
    /// close as the items do.
    fn blocks(&mut self, ui: &mut Ui, blocks: &[Block], depth: usize, in_item: bool) {
        for (index, block) in blocks.iter().enumerate() {
            if index > 0 {
                let gap = match (&blocks[index - 1], block) {
                    (Block::Heading { .. }, _) => GAP_AFTER_HEADING,
                    (_, Block::Heading { .. }) => GAP_BEFORE_HEADING,
                    _ if in_item => ITEM_GAP,
                    _ => GAP,
                };
                ui.add_space(self.em(gap));
            }
            ui.push_id(index, |ui| self.block(ui, block, depth));
        }
    }

    fn block(&mut self, ui: &mut Ui, block: &Block, depth: usize) {
        match block {
            Block::Heading { level, text } => {
                let index = usize::from((*level).clamp(1, 6) - 1);
                let size = self.heading * HEADING_SCALE[index];
                let font = self.strong(ui, size);
                let color = if *level >= 6 {
                    self.palette.text_dim
                } else {
                    self.color
                };
                let base = Base {
                    font: font.clone(),
                    strong: font,
                    color,
                    line: size * HEADING_LINE,
                    wrap: true,
                };
                self.inline(ui, text, &base);
            }
            Block::Paragraph(text) => {
                let base = self.body_base(ui);
                self.inline(ui, text, &base);
            }
            Block::Code { text, .. } => self.code(ui, text),
            Block::Html(text) => {
                let base = Base {
                    font: self.mono.clone(),
                    strong: self.mono.clone(),
                    color: self.palette.text_dim,
                    line: self.mono.size * LINE,
                    wrap: true,
                };
                self.plain(ui, text, &base);
            }
            Block::Quote(blocks) => self.quote(ui, blocks, depth),
            Block::List { start, items } => self.list(ui, *start, items, depth),
            Block::Table { head, rows, .. } => self.table(ui, head, rows),
            Block::Rule => {
                ui.add_space(self.em(0.25));
                crate::bars::hairline(ui);
                ui.add_space(self.em(0.25));
            }
        }
    }

    /// Literal text (raw HTML): never interpreted.
    fn plain(&mut self, ui: &mut Ui, text: &str, base: &Base) {
        let inline = Inline {
            spans: vec![super::Span {
                text: text.to_owned(),
                marks: super::Marks::default(),
                link: None,
            }],
        };
        self.inline(ui, &inline, base);
    }

    /// One run of inline text as one selectable label, its links live; the
    /// label's rect, unless the run is empty.
    fn inline(&mut self, ui: &mut Ui, inline: &Inline, base: &Base) -> Option<Rect> {
        if inline.is_empty() {
            return None;
        }
        let mut job = LayoutJob::default();
        job.wrap.max_width = if base.wrap {
            ui.available_width()
        } else {
            f32::INFINITY
        };
        let mut links: Vec<(Range<usize>, &Link)> = Vec::new();
        let mut chars = 0;
        for span in &inline.spans {
            let marks = span.marks;
            let mut format = TextFormat {
                font_id: if marks.strong {
                    base.strong.clone()
                } else {
                    base.font.clone()
                },
                color: base.color,
                line_height: Some(base.line),
                italics: marks.emphasis,
                ..TextFormat::default()
            };
            if marks.code {
                format.font_id =
                    FontId::new(base.font.size * INLINE_CODE_SCALE, self.mono.family.clone());
                format.background = self.code_bg;
            }
            if marks.strike {
                format.strikethrough = Stroke::new(1.0, base.color);
            }
            let text = match &span.link {
                Some(link) => {
                    format.color = self.link;
                    format.underline = Stroke::new(1.0, self.link);
                    if link.image {
                        image_label(&span.text)
                    } else {
                        span.text.clone()
                    }
                }
                None => span.text.clone(),
            };
            let count = text.chars().count();
            if let Some(link) = &span.link {
                links.push((chars..chars + count, link));
            }
            chars += count;
            job.append(&text, 0.0, format);
        }
        let galley = ui.painter().layout_job(job);
        let response = ui.add(Label::new(galley.clone()).selectable(true));
        let rect = response.rect;
        self.follow(ui, response, &galley, &links);
        Some(rect)
    }

    /// The link under the pointer of the label `response` shows `galley`
    /// in: its address on hover, opened on a click.
    fn follow(
        &mut self,
        ui: &Ui,
        response: egui::Response,
        galley: &Arc<Galley>,
        links: &[(Range<usize>, &Link)],
    ) {
        if links.is_empty() {
            return;
        }
        let Some(pointer) = response
            .interact_pointer_pos()
            .or_else(|| response.hover_pos())
        else {
            return;
        };
        let Some(link) = hit(galley, pointer - response.rect.min, links) else {
            return;
        };
        let hover = if link.image {
            own_with("markdown-image-hover", &[("url", &*link.url)])
        } else {
            link.url.to_string()
        };
        if super::openable(&link.url) {
            ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
        }
        let response = response.on_hover_text_at_pointer(hover);
        if response.clicked() {
            if super::openable(&link.url) {
                ui.ctx().open_url(egui::OpenUrl::new_tab(&link.url));
            }
            self.clicked = Some(link.url.to_string());
        }
    }

    /// A code block: the monospace text on the field surface, scrolled
    /// sideways rather than wrapped.
    fn code(&mut self, ui: &mut Ui, text: &str) {
        let padding = Margin::symmetric(
            self.em(CODE_PADDING.x).round() as i8,
            self.em(CODE_PADDING.y).round() as i8,
        );
        Frame::new()
            .fill(self.palette.field)
            .stroke(Stroke::new(1.0, self.palette.separator))
            .corner_radius(self.radius)
            .inner_margin(padding)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ScrollArea::horizontal()
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        let base = Base {
                            font: self.mono.clone(),
                            strong: self.mono.clone(),
                            color: self.palette.text,
                            line: self.mono.size * LINE,
                            wrap: false,
                        };
                        self.plain(ui, text, &base);
                    });
            });
    }

    /// A quote: its blocks indented after a rule, in the dim text colour.
    fn quote(&mut self, ui: &mut Ui, blocks: &[Block], depth: usize) {
        let saved = self.color;
        self.color = self.palette.text_dim;
        let indent = self.em(QUOTE_INDENT);
        let row = ui.with_layout(Layout::left_to_right(egui::Align::Min), |ui| {
            ui.add_space(indent);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                self.blocks(ui, blocks, depth, false);
            });
        });
        self.color = saved;
        let rect = row.response.rect;
        let rule = Rect::from_min_size(rect.min, vec2(QUOTE_RULE, rect.height()));
        ui.painter()
            .rect_filled(rule, QUOTE_RULE / 2.0, self.palette.text_faint);
    }

    fn list(&mut self, ui: &mut Ui, start: Option<u64>, items: &[Item], depth: usize) {
        let gutter = self.em(GUTTER);
        let line = self.em(LINE);
        for (index, item) in items.iter().enumerate() {
            if index > 0 {
                ui.add_space(self.em(ITEM_GAP));
            }
            ui.push_id(index, |ui| {
                ui.with_layout(Layout::left_to_right(egui::Align::Min), |ui| {
                    let (marker, _) = ui.allocate_exact_size(vec2(gutter, line), Sense::hover());
                    self.marker(ui, marker, start, index, item.task, depth);
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        self.blocks(ui, &item.blocks, depth + 1, true);
                    });
                });
            });
        }
    }

    /// An item's marker in the gutter `at`, centred on its first line: the
    /// task box, the number, or the bullet for the nesting depth.
    fn marker(
        &self,
        ui: &Ui,
        at: Rect,
        start: Option<u64>,
        index: usize,
        task: Option<bool>,
        depth: usize,
    ) {
        let painter = ui.painter();
        let ink = self.color;
        if let Some(done) = task {
            let side = crate::toggle::BOX;
            let b = Rect::from_center_size(
                pos2(at.left() + side / 2.0, at.center().y),
                Vec2::splat(side),
            );
            // Filled in the link colour: a hue scheme's light accent is a
            // tint too pale to read as "done" under the white tick.
            let palette = Palette {
                accent: self.link,
                ..self.palette
            };
            crate::toggle::paint_box(painter, &palette, b, done, self.palette.text_faint);
            return;
        }
        match start {
            Some(first) => {
                let number = format!("{}.", first.saturating_add(index as u64));
                let galley = painter.layout_no_wrap(number, self.body.clone(), ink);
                let y = at.center().y - galley.size().y / 2.0;
                let x = at.right() - galley.size().x - self.em(0.45);
                painter.galley(pos2(x, y), galley, ink);
            }
            None => {
                let r = self.em(BULLET);
                let centre = pos2(at.left() + self.em(0.55), at.center().y);
                match depth % 3 {
                    0 => painter.circle_filled(centre, r, ink),
                    1 => painter.circle_stroke(centre, r, Stroke::new(1.0, ink)),
                    _ => {
                        let square = Rect::from_center_size(centre, Vec2::splat(r * 1.8));
                        painter.rect_filled(square, 0.0, ink)
                    }
                };
            }
        }
    }

    /// A table: a striped grid with a strong header row, in a frame that
    /// scrolls sideways when it is wider than the column. Cells are
    /// unwrapped and start-aligned: egui's grid sizes a column from its
    /// cells, so a right-aligned cell would claim the whole row.
    fn table(&mut self, ui: &mut Ui, head: &[Inline], rows: &[Vec<Inline>]) {
        let spacing = vec2(self.em(1.0), self.em(0.35));
        Frame::new()
            .stroke(Stroke::new(1.0, self.palette.separator))
            .corner_radius(self.radius)
            .inner_margin(Margin::symmetric(
                self.em(0.6).round() as i8,
                self.em(0.4).round() as i8,
            ))
            .show(ui, |ui| {
                ScrollArea::horizontal()
                    .auto_shrink([true, true])
                    .show(ui, |ui| {
                        let mut header_bottom = f32::NEG_INFINITY;
                        let grid = egui::Grid::new("table")
                            .striped(true)
                            .spacing(spacing)
                            .show(ui, |ui| {
                                let mut header = self.body_base(ui);
                                header.font = header.strong.clone();
                                header.wrap = false;
                                for cell in head {
                                    let rect = self.cell(ui, cell, &header);
                                    header_bottom = header_bottom.max(rect.bottom());
                                }
                                ui.end_row();
                                let mut base = self.body_base(ui);
                                base.wrap = false;
                                for row in rows {
                                    for cell in row {
                                        self.cell(ui, cell, &base);
                                    }
                                    ui.end_row();
                                }
                            });
                        // The header's rule, midway to the first row.
                        if header_bottom.is_finite() && !rows.is_empty() {
                            let y = ui
                                .painter()
                                .round_to_pixel_center(header_bottom + spacing.y / 2.0);
                            ui.painter().hline(
                                grid.response.rect.x_range(),
                                y,
                                Stroke::new(1.0, self.palette.separator),
                            );
                        }
                    });
            });
    }

    /// One table cell; an empty one still takes its column.
    fn cell(&mut self, ui: &mut Ui, cell: &Inline, base: &Base) -> Rect {
        match self.inline(ui, cell, base) {
            Some(rect) => rect,
            None => ui.label("").rect,
        }
    }
}

/// An image's alt text as its link reads ("Image: a cat").
fn image_label(alt: &str) -> String {
    let alt = alt.trim();
    if alt.is_empty() {
        own("markdown-image-untitled")
    } else {
        own_with("markdown-image", &[("alt", alt)])
    }
}

/// The link under `at` (relative to the galley), if the pointer is on one
/// of its characters rather than merely nearest one.
fn hit<'a>(galley: &Arc<Galley>, at: Vec2, links: &[(Range<usize>, &'a Link)]) -> Option<&'a Link> {
    let index = galley.cursor_from_pos(at).index.0;
    for i in [index, index.saturating_sub(1)] {
        let Some((_, link)) = links.iter().find(|(range, _)| range.contains(&i)) else {
            continue;
        };
        let a = galley.pos_from_cursor(CCursor::new(i));
        let b = galley.pos_from_cursor(CCursor::new(i + 1));
        if (a.top() - b.top()).abs() > 0.5 {
            continue;
        }
        let glyph = Rect::from_min_max(
            pos2(a.left().min(b.left()), a.top()),
            pos2(a.left().max(b.left()), a.bottom()),
        );
        if glyph.contains(at.to_pos2()) {
            return Some(link);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markdown::Document;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    const SAMPLE: &str = "\
# Title

Some **strong** and _slanted_ text with `code` and a [link](https://mixos.dev/).

- one
- [x] done
  1. nested

> quoted

```
let x = 1;
```

| a | b |
|---|--:|
| 1 | 2 |

---

![a cat](https://track.invalid/p.gif) <script>alert(1)</script>
";

    /// The document, the link `show` reported and the URLs it asked egui
    /// to open.
    struct Seen {
        document: Document,
        clicked: Option<String>,
        opened: Vec<String>,
    }

    fn harness(document: Document) -> Harness<'static, Seen> {
        let mut h = Harness::builder()
            .with_size(egui::vec2(520.0, 640.0))
            .build_ui_state(
                |ui, seen: &mut Seen| {
                    if let Some(url) = show(ui, &seen.document) {
                        seen.clicked = Some(url);
                    }
                    ui.ctx().output(|o| {
                        for command in &o.commands {
                            if let egui::OutputCommand::OpenUrl(open) = command {
                                seen.opened.push(open.url.clone());
                            }
                        }
                    });
                },
                Seen {
                    document,
                    clicked: None,
                    opened: Vec::new(),
                },
            );
        h.run();
        h
    }

    /// A press and release at `at`, in one pass, after a hover there.
    fn click_at(h: &mut Harness<'static, Seen>, at: egui::Pos2) {
        h.hover_at(at);
        h.run();
        for pressed in [true, false] {
            h.event(egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            });
        }
        h.run();
    }

    #[test]
    fn every_element_draws_its_text() {
        let h = harness(Document::parse(SAMPLE));
        // The table cells are one-letter labels; the rest are unique.
        for text in [
            "Title",
            "one",
            "done",
            "nested",
            "quoted",
            "let x = 1;",
            "a",
            "2",
        ] {
            assert!(h.query_by_label(text).is_some(), "{text}");
        }
        // The paragraph is one label, its link inside it.
        assert!(
            h.query_by_label("Some strong and slanted text with code and a link.")
                .is_some()
        );
        // Raw HTML is text, never markup; the image is its alt text.
        let tail = format!("{} <script>alert(1)</script>", image_label("a cat"));
        assert!(h.query_by_label(&tail).is_some(), "{tail}");
    }

    #[test]
    fn a_click_on_the_link_reports_and_opens_it() {
        let mut h = harness(Document::parse("see [docs](https://mixos.dev/) here"));
        let rect = h.get_by_label("see docs here").rect();
        // "docs" is characters 4 to 8 of 13: the middle of the label.
        let at = egui::pos2(rect.left() + rect.width() * 0.45, rect.center().y);
        click_at(&mut h, at);
        assert_eq!(h.state().clicked.as_deref(), Some("https://mixos.dev/"));
        assert_eq!(h.state().opened, ["https://mixos.dev/"]);
    }

    #[test]
    fn a_click_off_the_link_reports_nothing() {
        let mut h = harness(Document::parse("see [docs](https://mixos.dev/) here"));
        let rect = h.get_by_label("see docs here").rect();
        click_at(&mut h, egui::pos2(rect.left() + 2.0, rect.center().y));
        assert_eq!(h.state().clicked, None);
        assert!(h.state().opened.is_empty());
    }

    #[test]
    fn a_link_that_cannot_open_is_reported_but_not_opened() {
        let mut h = harness(Document::parse("[run](file:///bin/sh)"));
        let rect = h.get_by_label("run").rect();
        click_at(&mut h, rect.center());
        assert_eq!(h.state().clicked.as_deref(), Some("file:///bin/sh"));
        assert!(h.state().opened.is_empty());
    }
}
