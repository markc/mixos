// SPDX-License-Identifier: MIT OR Apache-2.0
//! The editor widget: input, scrolling and caret following over visual rows,
//! and drawing.
//!
//! Draw order: background · current row · find matches · selection · other
//! origins' selections · change tints · text · squiggles · gutter (numbers,
//! origin strip, lint column) · preedit · other origins' carets · own caret ·
//! scrollbars · hover labels.
//!
//! Text sits on the cell grid in logical order: consecutive ASCII clusters of
//! one class are one galley, every other cluster is drawn alone at its cell,
//! and a cluster over 4 KiB draws only its first 256 bytes (a visual cap; the
//! measurement never splits clusters).
//!
//! Widget state is view mechanics only, kept in egui's memory under the
//! widget's id: the effective scroll (ahead of the model by at most one
//! round trip), drag state, the input-method guard and the measurement
//! caches.

use std::collections::{HashMap, VecDeque};
use std::ops::Range;
use std::sync::{Arc, Mutex};

use edit::origin::{Origin, OriginKind};
use edit::text::Text;
use edit::view::{MeasureCfg, prev_grapheme};
use editor_model::diag::Severity;
use editor_model::highlight::SliceBudget;
use editor_model::model::{clamp_offset, line_of};
use egui::output::IMEOutput;
use egui::{
    Color32, CursorIcon, EventFilter, FontId, Galley, IMEPurpose, Painter, PointerButton, Pos2,
    Rect, Response, Sense, Stroke, TextStyle, Ui, Vec2, pos2, vec2,
};

use crate::layout::{self as geo, CARET_MARGIN_ROWS, Geometry, Metrics, STRIP_W};
use crate::lines::{self, Checkpoints, LineCells, Version};
use crate::rows::{Pos, RowStart, Rows};
use crate::{
    Doc, EditCommand, Event, HlClass, LayoutReport, Motion, Output, Palette, Scroll, Selection,
    View, Wrap, ime, input,
};

/// Change tints last this long, in seconds.
const TINT: f64 = 2.0;
/// Wheel notch, in rows, when the platform reports lines.
const WHEEL_ROWS: f32 = 3.0;
/// Rows read whole for drawing up to this many bytes; longer ones per cluster.
const MAX_ROW_TEXT: usize = 64 * 1024;
/// Clusters longer than this draw only [`CLUSTER_DRAW_CAP`] bytes.
const HUGE_CLUSTER: usize = 4096;
const CLUSTER_DRAW_CAP: usize = 256;
/// Published scrolls remembered while waiting for the model's echo.
const MAX_ECHO: usize = 64;
/// Presses this close in time (seconds) and space (points) count as one
/// double or triple click, as egui counts them.
const CLICK_SECS: f64 = 0.3;
const CLICK_SLOP: f32 = 6.0;

#[derive(Debug, Clone, Copy, PartialEq)]
enum Drag {
    /// Selecting text from a press.
    Text,
    /// Selecting whole lines from the gutter, anchored at a line.
    Lines(usize),
    /// Dragging a scrollbar thumb, grabbed this far into it.
    VBar(f32),
    HBar(f32),
}

#[derive(Default)]
struct State {
    document: Option<u64>,
    /// The first visual row drawn.
    top: Pos,
    /// Horizontal scroll in cells (without wrap).
    x_cells: usize,
    /// Scrolls published and not yet echoed back by the model.
    echo: VecDeque<Scroll>,
    last_model_scroll: Option<Scroll>,
    seen_head: Option<usize>,
    seen_version: Version,
    caret_visible: bool,
    drag: Option<Drag>,
    drag_offset: Option<usize>,
    wheel: Vec2,
    ime: ime::Composition,
    was_focused: bool,
    last_report: Option<LayoutReport>,
    ck: Checkpoints,
    rows: Rows,
    /// When each marker rev was first drawn (tints last [`TINT`] from then).
    tint_seen: HashMap<u64, f64>,
    /// Widest visible row (cells) at the last draw: the horizontal extent.
    max_cells: usize,
    /// The column kept by vertical motions over wrapped rows, valid while
    /// the caret stays where the last such motion put it.
    goal: Option<(usize, f32)>,
    /// Input held, in order, until the owner applies the previous frame's
    /// commands (see `needs_current_state`).
    pending: Vec<egui::Event>,
    /// The primary press under way: where, and its click count (1-3).
    press: Option<(Pos2, u8)>,
    /// The last completed click: when, where, and its click count.
    last_click: Option<(f64, Pos2, u8)>,
}

/// The measurement context for one frame.
struct Measure<'s> {
    text: &'s Text,
    cfg: MeasureCfg,
    ck: &'s mut Checkpoints,
    rows: &'s mut Rows,
}

impl Measure<'_> {
    fn start(&mut self, p: Pos) -> RowStart {
        self.rows
            .start(self.text, &self.cfg, self.ck, p.line, p.row)
    }
    fn end(&mut self, p: Pos) -> usize {
        self.rows.end(self.text, &self.cfg, self.ck, p.line, p.row)
    }
    fn last(&mut self, p: Pos) -> bool {
        p.row + 1 >= self.rows.count(self.text, &self.cfg, self.ck, p.line)
    }
    fn pos_of(&mut self, offset: usize) -> Pos {
        self.rows.pos_of(self.text, &self.cfg, self.ck, offset)
    }
    fn step(&mut self, p: Pos, delta: isize) -> Pos {
        self.rows.step(self.text, &self.cfg, self.ck, p, delta)
    }
    fn distance(&mut self, a: Pos, b: Pos) -> usize {
        self.rows.distance(self.text, &self.cfg, self.ck, a, b)
    }
    fn cells_of(&mut self, offset: usize) -> usize {
        lines::cells_of(self.text, &self.cfg, self.ck, offset).1
    }
}

/// One drawn row.
struct Row {
    pos: Pos,
    y: f32,
    /// The cell drawn at the text area's left edge.
    origin: usize,
    /// The row's clusters; `content` is the row's byte span.
    cells: LineCells,
    /// The last row of its line.
    last: bool,
    text: Option<String>,
}

/// Show `doc` filling the space left in `ui`. `id_salt` keeps the view state
/// (scroll, drag, composition) apart from other editors in the same `ui`.
pub fn show(
    ui: &mut Ui,
    id_salt: impl std::hash::Hash + std::fmt::Debug,
    doc: &Doc<'_>,
    palette: &Palette,
    view: &View,
) -> Output {
    let id = ui.make_persistent_id(id_salt);
    let rect = ui.available_rect_before_wrap();
    let response = ui.interact(rect, id, Sense::click_and_drag());
    ui.advance_cursor_after_rect(rect);
    let shared = ui.data_mut(|d| {
        d.get_temp_mut_or_insert_with(id, || {
            Arc::new(Mutex::new(State {
                caret_visible: true,
                ..State::default()
            }))
        })
        .clone()
    });
    let mut guard = shared.lock().unwrap_or_else(|e| e.into_inner());
    let mut pane = Pane {
        ui,
        doc,
        palette,
        view,
        st: &mut guard,
        events: Vec::new(),
        response: &response,
        now: 0.0,
        projected: None,
    };
    pane.run(rect);
    let events = pane.events;
    if !events.is_empty() {
        response.ctx.request_repaint();
    }
    let held = !guard.pending.is_empty();
    Output {
        response,
        events,
        held,
    }
}

struct Pane<'u, 'a> {
    ui: &'u mut Ui,
    doc: &'u Doc<'a>,
    palette: &'u Palette,
    view: &'u View,
    st: &'u mut State,
    events: Vec<Event>,
    response: &'u Response,
    now: f64,
    /// The head after this frame's own vertical motions.
    projected: Option<usize>,
}

impl Pane<'_, '_> {
    fn text(&self) -> &Text {
        self.doc.text
    }

    fn line_count(&self) -> usize {
        self.doc.text.line_count().max(1)
    }

    fn wraps(&self) -> bool {
        self.view.wrap == Wrap::Words
    }

    fn measure(&mut self) -> Measure<'_> {
        Measure {
            text: self.doc.text,
            cfg: self.view.measure(),
            ck: &mut self.st.ck,
            rows: &mut self.st.rows,
        }
    }

    fn font(&self) -> FontId {
        let size = self.view.font_size.unwrap_or_else(|| {
            self.ui
                .style()
                .text_styles
                .get(&TextStyle::Monospace)
                .map_or(13.0, |f| f.size)
        });
        FontId::monospace(size)
    }

    fn run(&mut self, rect: Rect) {
        self.now = self.ui.input(|i| i.time);
        let font = self.font();
        let cell_w = self
            .ui
            .ctx()
            .fonts_mut(|f| f.glyph_width(&font, '0'))
            .max(1.0);
        let metrics = Metrics {
            cell_w,
            row_h: (font.size * self.view.line_height).round().max(1.0),
        };
        let g = Geometry::new(rect, metrics, self.line_count(), self.view.line_numbers);
        let version = (self.doc.identity, self.doc.revision, self.doc.text.len());
        let cfg = self.view.measure();
        // A switch resets the state, so it comes before this frame's layout.
        self.switch_document();
        self.st.ck.sync(version, &cfg);
        self.st.rows.sync(version, &cfg, self.view.wrap, g.cols());
        self.focus();
        self.sync_composition();
        self.adopt_model_scroll();
        // A resize or an edit can leave the top past its line's rows: clamp
        // it, and tell the owner, so a stored scroll never restores a row
        // that no longer exists.
        self.scroll_to(self.st.top, self.st.x_cells);
        self.input_events(&g);
        self.pointer(&g);
        self.follow(&g, version);
        self.paint(&g, &font);
        self.output(&g);
    }

    fn switch_document(&mut self) {
        let identity = self.doc.identity;
        if self.st.document == Some(identity) {
            return;
        }
        let active = self.st.ime.active();
        let mut ime = std::mem::take(&mut self.st.ime);
        // Input queued for the previous document must not reach this one,
        // known session or not.
        ime.interrupt();
        let scroll = self.doc.model.scroll;
        *self.st = State {
            document: Some(identity),
            top: Pos {
                line: scroll.first_line.max(1),
                row: scroll.row,
            },
            x_cells: scroll.x_cells,
            last_model_scroll: Some(self.doc.model.scroll),
            caret_visible: true,
            ime,
            was_focused: self.st.was_focused,
            ..State::default()
        };
        if active {
            self.events.push(Event::Preedit(String::new()));
        }
    }

    fn focus(&mut self) {
        let r = self.response;
        if r.clicked() || r.drag_started() || r.is_pointer_button_down_on() {
            r.request_focus();
        }
        let focused = r.has_focus();
        if focused {
            let id = r.id;
            self.ui.memory_mut(|m| {
                m.set_focus_lock_filter(
                    id,
                    EventFilter {
                        tab: true,
                        horizontal_arrows: true,
                        vertical_arrows: true,
                        escape: false,
                    },
                )
            });
        }
        if focused != self.st.was_focused {
            self.events.push(Event::Focus(focused));
            if focused {
                // A composition begun in another widget or document must not
                // commit here: interrupt it before taking input-method events.
                self.st.ime.interrupt();
            } else if self.st.ime.active() {
                self.st.ime.cancel();
                self.events.push(Event::Preedit(String::new()));
            } else {
                self.st.ime.cancel();
            }
            self.drop_held_ime();
            self.st.was_focused = focused;
        }
    }

    /// A remote edit over the composition clears the model's anchor: the
    /// preedit is cancelled before any of its input is taken.
    fn sync_composition(&mut self) {
        if !self.st.ime.active() {
            return;
        }
        if self.doc.model.composition.is_some() {
            self.st.ime.anchored = true;
        } else if self.st.ime.anchored {
            self.st.ime.cancel();
            self.drop_held_ime();
            self.events.push(Event::Preedit(String::new()));
        }
    }

    /// Forget held input-method events: their composition was cancelled.
    fn drop_held_ime(&mut self) {
        self.st
            .pending
            .retain(|e| !matches!(e, egui::Event::Ime(_)));
    }

    /// Take a scroll the model was given from elsewhere; ignore echoes of our own.
    fn adopt_model_scroll(&mut self) {
        let model = self.doc.model.scroll;
        // The owner holds the latest scroll published: every echo is in.
        if self.st.echo.back() == Some(&model) {
            self.st.echo.clear();
            self.st.last_model_scroll = Some(model);
            return;
        }
        if self.st.last_model_scroll == Some(model) {
            return;
        }
        if let Some(i) = self.st.echo.iter().position(|s| *s == model) {
            self.st.echo.drain(..=i);
        } else {
            self.st.top = Pos {
                line: model.first_line.max(1),
                row: if self.wraps() { model.row } else { 0 },
            };
            self.st.x_cells = model.x_cells;
            self.st.echo.clear();
        }
        self.st.last_model_scroll = Some(model);
    }

    fn published(&self, top: Pos, x_cells: usize) -> Scroll {
        Scroll {
            first_line: top.line,
            row: if self.wraps() { top.row } else { 0 },
            x_cells: if self.wraps() { 0 } else { x_cells },
        }
    }

    fn scroll_to(&mut self, top: Pos, x_cells: usize) {
        let top = self.measure().step(top, 0);
        if top == self.st.top && x_cells == self.st.x_cells {
            return;
        }
        let published = self.published(self.st.top, self.st.x_cells);
        self.st.top = top;
        self.st.x_cells = x_cells;
        let scroll = self.published(top, x_cells);
        if scroll != published {
            // An owner that never stores the scroll never echoes it back.
            if self.st.echo.len() >= MAX_ECHO {
                self.st.echo.pop_front();
            }
            self.st.echo.push_back(scroll);
            self.events.push(Event::Scrolled(scroll));
        }
        self.ui.ctx().request_repaint();
    }

    fn edit(&mut self, c: EditCommand) {
        // The owner applies commands after this frame: a later motion this
        // frame can only build on a head this pane computed itself.
        self.projected = None;
        let moves = matches!(
            c,
            EditCommand::Move { .. }
                | EditCommand::SelectAll
                | EditCommand::SelectWord(_)
                | EditCommand::SelectLine(_)
        ) || matches!(c, EditCommand::SetSelection(_));
        if moves || !self.view.read_only {
            self.events.push(Event::Command(c));
        }
    }

    /// Copy or Cut. The model's selection is current only while this frame
    /// has sent no command; otherwise it runs next frame, after the owner
    /// has applied them.
    fn clipboard(&mut self, cut: bool) {
        if let Some(s) = self.selected_text() {
            self.ui.ctx().copy_text(s);
            if cut {
                self.edit(EditCommand::Delete);
            }
        }
    }

    /// Whether `ev` reads the model's selection, which is current only while
    /// this frame has sent no command: Copy, Cut, and a wrapped vertical
    /// motion that cannot build on a head this pane projected itself.
    fn needs_current_state(&self, ev: &egui::Event, g: &Geometry) -> bool {
        let rows = g.full_rows();
        let sent = self
            .events
            .iter()
            .any(|e| matches!(e, Event::Command(_) | Event::Undo | Event::Redo));
        if !sent {
            return false;
        }
        match ev {
            egui::Event::Copy | egui::Event::Cut => true,
            // A press hit-tests the text, which the commands may change.
            egui::Event::PointerButton {
                pos,
                button: PointerButton::Primary,
                pressed: true,
                ..
            } => self.ours(g, *pos),
            egui::Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } if self.wraps() && self.projected.is_none() => matches!(
                input::key(*key, *modifiers, rows),
                Some(Event::Command(EditCommand::Move {
                    to: Motion::Up | Motion::Down | Motion::PageUp(_) | Motion::PageDown(_),
                    ..
                }))
            ),
            _ => false,
        }
    }

    fn selected_text(&self) -> Option<String> {
        let sel = self.doc.model.sel;
        let r = sel_range(sel);
        if r.is_empty() || r.end > self.text().len() {
            return None;
        }
        let mut s = String::with_capacity(r.len());
        self.text().read(r, &mut s);
        Some(s)
    }

    /// Hold `ev` and the rest of the batch, in order, for the next frame.
    fn hold(&mut self, ev: egui::Event, rest: impl Iterator<Item = egui::Event>) {
        self.st.pending.push(ev);
        self.st.pending.extend(rest);
        if self.st.ime.resetting() {
            // Arriving during an interrupt, they belong to the composition
            // being dropped: holding must not save them.
            self.drop_held_ime();
        }
        self.ui.ctx().request_repaint();
    }

    /// A press at `pos` lands on this editor: inside it, on its layer, and
    /// not under anything drawn above it.
    fn ours(&self, g: &Geometry, pos: Pos2) -> bool {
        self.ui.is_enabled()
            && g.bounds.contains(pos)
            && self.ui.clip_rect().contains(pos)
            && self.ui.ctx().layer_id_at(pos) == Some(self.ui.layer_id())
    }

    /// A primary press on the editor, counting double and triple clicks.
    /// Only completed clicks (released near their press) count toward the
    /// next press's click count, as in egui.
    fn press_at(&mut self, g: &Geometry, pos: Pos2, shift: bool) {
        let count = match self.st.last_click {
            Some((t, at, n)) if self.now - t <= CLICK_SECS && at.distance(pos) <= CLICK_SLOP => {
                n % 3 + 1
            }
            _ => 1,
        };
        self.st.press = Some((pos, count));
        if count == 1 || !g.text_rect().contains(pos) {
            self.press(g, pos, shift);
            return;
        }
        let o = self.offset_at(g, pos);
        self.edit(if count == 3 {
            EditCommand::SelectLine(o)
        } else {
            EditCommand::SelectWord(o)
        });
        // Dragging on from a word or line extends the selection from it.
        self.st.drag = Some(Drag::Text);
        self.st.drag_offset = Some(o);
    }

    fn release_at(&mut self, pos: Pos2) {
        if self.st.drag.take().is_some() {
            self.st.drag_offset = None;
        }
        self.st.last_click = match self.st.press.take() {
            Some((at, count)) if at.distance(pos) <= CLICK_SLOP => Some((self.now, at, count)),
            _ => None,
        };
    }

    /// Keys, text, clipboard, input-method and pointer-button events, in the
    /// order they arrived.
    fn input_events(&mut self, g: &Geometry) {
        let focused = self.response.has_focus();
        // Held input belongs to a focused editor; focus moved away drops it.
        let held = if focused {
            std::mem::take(&mut self.st.pending)
        } else {
            self.st.pending.clear();
            Vec::new()
        };
        let mut events = held;
        events.extend(self.ui.input(|i| i.events.clone()));
        let mut events = events.into_iter();
        while let Some(ev) = events.next() {
            // A press that wanders past the slop at any point is a drag, not
            // a click, even when it comes back within the frame.
            if let egui::Event::PointerMoved(p) = ev {
                if let Some((at, _)) = self.st.press
                    && at.distance(p) > CLICK_SLOP
                {
                    self.st.press = None;
                }
                continue;
            }
            if let egui::Event::PointerButton {
                pos,
                button: PointerButton::Primary,
                pressed,
                modifiers,
            } = ev
            {
                if pressed && self.ours(g, pos) {
                    if self.needs_current_state(&ev, g) {
                        self.hold(ev, events);
                        return;
                    }
                    self.press_at(g, pos, modifiers.shift);
                } else if !pressed {
                    self.release_at(pos);
                }
                continue;
            }
            if !focused {
                continue;
            }
            // Input that reads the model waits, with everything after it in
            // order, until the owner has applied this frame's commands.
            if self.needs_current_state(&ev, g) {
                self.hold(ev, events);
                return;
            }
            match ev {
                egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } if !self.st.ime.active() => match input::key(key, modifiers, g.full_rows()) {
                    Some(Event::Command(EditCommand::Move { to, extend })) if self.wraps() => {
                        self.vertical(to, extend)
                    }
                    Some(Event::Command(c)) => self.edit(c),
                    Some(e @ (Event::Undo | Event::Redo)) if !self.view.read_only => {
                        self.projected = None;
                        self.events.push(e)
                    }
                    _ => {}
                },
                egui::Event::Text(s) if !self.st.ime.active() => {
                    if let Some(Event::Command(c)) = input::text(&s) {
                        self.edit(c);
                    }
                }
                egui::Event::Copy => self.clipboard(false),
                egui::Event::Cut => self.clipboard(true),
                egui::Event::Paste(s) if !s.is_empty() => self.edit(EditCommand::Insert(s)),
                egui::Event::Ime(egui::ImeEvent::Preedit { text, .. }) => {
                    if self.st.ime.preedit(&text) {
                        // The owner anchors a non-empty preedit before the
                        // next frame; if the anchor is gone by then, a
                        // remote edit took it, even in between.
                        self.st.ime.anchored = !text.is_empty();
                        self.events.push(Event::Preedit(text));
                    }
                }
                egui::Event::Ime(egui::ImeEvent::Commit(s)) => {
                    let was = self.st.ime.active();
                    if self.st.ime.commit() {
                        if was {
                            self.events.push(Event::Preedit(String::new()));
                        }
                        if !s.is_empty() {
                            self.edit(EditCommand::Insert(s));
                        }
                    }
                }
                _ => {}
            }
        }
    }

    /// Up, Down, Page Up and Page Down by visual rows (wrap mode); other
    /// motions go to the model unchanged.
    fn vertical(&mut self, to: Motion, extend: bool) {
        let delta = match to {
            Motion::Up => -1,
            Motion::Down => 1,
            Motion::PageUp(n) => -(n as isize),
            Motion::PageDown(n) => n as isize,
            other => return self.edit(EditCommand::Move { to: other, extend }),
        };
        // Several motions in one frame build on each other, not on the
        // model's head from before them.
        let head = self.projected.unwrap_or(self.doc.model.sel.head);
        let head = clamp_offset(self.text(), head);
        let goal = self.st.goal.filter(|(at, _)| *at == head).map(|(_, x)| x);
        let len = self.text().len();
        let mut m = self.measure();
        let pos = m.pos_of(head);
        let start = m.start(pos);
        let x = goal.unwrap_or_else(|| (m.cells_of(head) - start.cells) as f32);
        let target = m.step(pos, delta);
        let offset = if target == pos {
            if delta < 0 { 0 } else { len }
        } else {
            let s = m.start(target);
            let end = m.end(target);
            let last = m.last(target);
            let o =
                lines::offset_from(m.text, &m.cfg, (s.offset, s.cells), end, s.cells as f32 + x);
            if o == end && !last && o > s.offset {
                prev_grapheme(m.text, o)
            } else {
                o
            }
        };
        self.st.goal = Some((offset, x));
        self.edit(EditCommand::Move {
            to: Motion::To(offset),
            extend,
        });
        self.projected = Some(offset);
    }

    /// The offset under `p` (rows past the end give the text end).
    fn offset_at(&mut self, g: &Geometry, p: Pos2) -> usize {
        let (i, cells) = g.hit(p);
        let wraps = self.wraps();
        let x_cells = self.st.x_cells;
        let top = self.st.top;
        let len = self.text().len();
        let mut m = self.measure();
        let target = m.step(top, i);
        if i > 0 && m.distance(top, target) < i as usize {
            return len;
        }
        if wraps {
            let s = m.start(target);
            let end = m.end(target);
            lines::offset_from(
                m.text,
                &m.cfg,
                (s.offset, s.cells),
                end,
                s.cells as f32 + cells,
            )
        } else {
            lines::offset_at(m.text, &m.cfg, m.ck, target.line, x_cells as f32 + cells)
        }
    }

    fn line_start(&self, line: usize) -> usize {
        self.text().line_start(line).unwrap_or(self.text().len())
    }

    fn pointer(&mut self, g: &Geometry) {
        let r = self.response;
        let (down, pos) = self
            .ui
            .input(|i| (i.pointer.primary_down(), i.pointer.interact_pos()));
        // Presses and releases are taken from the event list, in order with
        // keys and text (`events`); this continues a drag under way.
        if down {
            // A press that wanders past the slop is a drag, not a click.
            if let (Some((at, _)), Some(p)) = (self.st.press, pos)
                && at.distance(p) > CLICK_SLOP
            {
                self.st.press = None;
            }
            if let (Some(_), Some(p)) = (self.st.drag, pos) {
                self.drag_to(g, p);
                self.auto_scroll(g, p);
            }
        } else if self.st.drag.take().is_some() {
            self.st.drag_offset = None;
        }
        self.wheel(g);
        if let Some(p) = r.hover_pos() {
            let icon = match self.st.drag {
                Some(Drag::Text) => CursorIcon::Text,
                Some(Drag::VBar(_) | Drag::HBar(_)) => CursorIcon::Grabbing,
                _ if g.vbar_track().contains(p)
                    || self.hbar_shown(g) && g.hbar_track().contains(p) =>
                {
                    CursorIcon::Default
                }
                _ if p.x < g.text_rect().left() => CursorIcon::Default,
                _ => CursorIcon::Text,
            };
            self.ui.ctx().set_cursor_icon(icon);
        }
    }

    fn hbar_shown(&self, g: &Geometry) -> bool {
        !self.wraps()
            && geo::thumb(
                g.hbar_track().width(),
                self.h_extent(g),
                g.cols(),
                self.st.x_cells,
            )
            .is_some()
    }

    fn h_extent(&self, g: &Geometry) -> usize {
        self.st.max_cells.max(self.st.x_cells + g.cols())
    }

    /// The vertical scrollbar: total units (rows when the document is small
    /// enough to wrap whole, else lines, plus a screen so the end can reach
    /// the top) and the top's index.
    fn vbar(&mut self, g: &Geometry) -> (usize, usize) {
        let top = self.st.top;
        let m = self.measure();
        let (total, first) = m.rows.extent(m.text, &m.cfg, m.ck, top);
        (total + g.full_rows() - 1, first)
    }

    fn press(&mut self, g: &Geometry, p: Pos2, shift: bool) {
        if self.press_bar(g, p) {
            return;
        }
        if p.x < g.text_rect().left() {
            let o = self.offset_at(g, pos2(g.text_rect().left(), p.y));
            let line = line_of(self.text(), o);
            self.st.drag = Some(Drag::Lines(line));
            self.st.drag_offset = None;
            self.drag_to(g, p);
            return;
        }
        let o = self.offset_at(g, p);
        self.edit(EditCommand::Move {
            to: Motion::To(o),
            extend: shift,
        });
        self.st.drag = Some(Drag::Text);
        self.st.drag_offset = Some(o);
    }

    fn drag_to(&mut self, g: &Geometry, p: Pos2) {
        match self.st.drag {
            Some(Drag::Text) => {
                let o = self.offset_at(g, p);
                if self.st.drag_offset != Some(o) {
                    self.st.drag_offset = Some(o);
                    self.edit(EditCommand::Move {
                        to: Motion::To(o),
                        extend: true,
                    });
                }
            }
            Some(Drag::Lines(anchor)) => {
                let o = self.offset_at(g, pos2(g.text_rect().left(), p.y));
                let line = line_of(self.text(), o).min(self.line_count());
                let sel = if line >= anchor {
                    Selection {
                        anchor: self.line_start(anchor),
                        head: self.line_start(line + 1),
                    }
                } else {
                    Selection {
                        anchor: self.line_start(anchor + 1),
                        head: self.line_start(line),
                    }
                };
                if self.st.drag_offset != Some(sel.head) {
                    self.st.drag_offset = Some(sel.head);
                    self.edit(EditCommand::SetSelection(sel));
                }
            }
            Some(Drag::VBar(grab)) => {
                let track = g.vbar_track();
                let (total, _) = self.vbar(g);
                let index = geo::thumb_to_first(
                    track.height(),
                    total,
                    g.full_rows(),
                    p.y - track.top() - grab,
                );
                let m = self.measure();
                let top = m.rows.at_index(m.text, &m.cfg, m.ck, index);
                self.scroll_to(top, self.st.x_cells);
            }
            Some(Drag::HBar(grab)) => {
                let track = g.hbar_track();
                let x = geo::thumb_to_first(
                    track.width(),
                    self.h_extent(g),
                    g.cols(),
                    p.x - track.left() - grab,
                );
                self.scroll_to(self.st.top, x);
            }
            None => {}
        }
    }

    /// While selecting outside the text area, scroll one step per frame
    /// (whole-line selections from the gutter scroll vertically only).
    fn auto_scroll(&mut self, g: &Geometry, p: Pos2) {
        let lines = match self.st.drag {
            Some(Drag::Text) => false,
            Some(Drag::Lines(_)) => true,
            _ => return,
        };
        let t = g.text_rect();
        let step = |d: f32| 1 + (d / g.metrics.row_h / 2.0) as isize;
        let dy = if p.y < t.top() {
            -step(t.top() - p.y)
        } else if p.y > t.bottom() {
            step(p.y - t.bottom())
        } else {
            0
        };
        let mut x = self.st.x_cells;
        if !self.wraps() && !lines {
            if p.x < t.left() {
                x = x.saturating_sub(1);
            } else if p.x > t.right() {
                x += 1;
            }
        }
        let before = (self.st.top, self.st.x_cells);
        if dy != 0 || x != before.1 {
            let top = self.measure().step(before.0, dy);
            self.scroll_to(top, x);
        }
        // Keep stepping only while the view can still move.
        if (self.st.top, self.st.x_cells) != before {
            self.drag_to(g, p);
            self.ui.ctx().request_repaint();
        }
    }

    /// Press on a scrollbar: grab the thumb, or page toward the press.
    fn press_bar(&mut self, g: &Geometry, p: Pos2) -> bool {
        let rows = g.full_rows();
        let v = g.vbar_track();
        let (total, first) = self.vbar(g);
        if v.contains(p)
            && let Some((off, len)) = geo::thumb(v.height(), total, rows, first)
        {
            let top = v.top() + off;
            if p.y >= top && p.y <= top + len {
                self.st.drag = Some(Drag::VBar(p.y - top));
            } else {
                let delta = if p.y < top {
                    -(rows as isize)
                } else {
                    rows as isize
                };
                let current = self.st.top;
                let next = self.measure().step(current, delta);
                self.scroll_to(next, self.st.x_cells);
            }
            return true;
        }
        let h = g.hbar_track();
        let cols = g.cols();
        if self.hbar_shown(g)
            && h.contains(p)
            && let Some((off, len)) = geo::thumb(h.width(), self.h_extent(g), cols, self.st.x_cells)
        {
            let left = h.left() + off;
            if p.x >= left && p.x <= left + len {
                self.st.drag = Some(Drag::HBar(p.x - left));
            } else {
                let x = if p.x < left {
                    self.st.x_cells.saturating_sub(cols)
                } else {
                    self.st.x_cells + cols
                };
                self.scroll_to(self.st.top, x);
            }
            return true;
        }
        false
    }

    fn wheel(&mut self, g: &Geometry) {
        if !self.response.contains_pointer() {
            return;
        }
        // Raw wheel events, in rows and cells; Shift turns a vertical wheel
        // horizontal.
        let page = vec2(g.cols() as f32, g.full_rows() as f32);
        let delta = self.ui.input(|i| {
            i.events.iter().fold(Vec2::ZERO, |sum, e| match e {
                egui::Event::MouseWheel {
                    unit,
                    delta,
                    modifiers,
                    ..
                } if !modifiers.command => {
                    // Shift turns a vertical wheel horizontal, before scaling,
                    // so a page across is a screenful of cells.
                    let delta = if modifiers.shift && delta.x == 0.0 {
                        vec2(delta.y, 0.0)
                    } else {
                        *delta
                    };
                    sum + match unit {
                        egui::MouseWheelUnit::Point => {
                            vec2(delta.x / g.metrics.cell_w, delta.y / g.metrics.row_h)
                        }
                        egui::MouseWheelUnit::Line => delta * WHEEL_ROWS,
                        // A page is a screenful: cells across, rows down.
                        egui::MouseWheelUnit::Page => delta * page,
                    }
                }
                _ => sum,
            })
        });
        if delta == Vec2::ZERO {
            return;
        }
        // The editor scrolls itself; a surrounding scroll area must not.
        self.ui.input_mut(|i| i.smooth_scroll_delta = Vec2::ZERO);
        self.st.wheel -= delta;
        let (dx, dy) = (self.st.wheel.x.trunc(), self.st.wheel.y.trunc());
        self.st.wheel -= vec2(dx, dy);
        let top = self.st.top;
        let top = self.measure().step(top, dy as isize);
        let x = if self.wraps() {
            0
        } else {
            (self.st.x_cells as i64 + dx as i64).max(0) as usize
        };
        self.scroll_to(top, x);
    }

    /// Follow the caret after a motion, or after an edit while the caret was
    /// on screen (an agent editing elsewhere must not move the view).
    fn follow(&mut self, g: &Geometry, version: Version) {
        let head = clamp_offset(self.text(), self.doc.model.sel.head);
        let moved = self.st.seen_head != Some(head);
        let edited = self.st.seen_version != version;
        let rows = g.full_rows();
        let wraps = self.wraps();
        let first_seen = self.st.seen_head.is_none();
        let caret_visible = self.st.caret_visible;
        let mut top = self.st.top;
        let mut x = self.st.x_cells;
        let mut m = self.measure();
        let caret = m.pos_of(head);
        if (moved || edited) && ((moved && !edited) || caret_visible) && !first_seen {
            let margin = CARET_MARGIN_ROWS.min(rows.saturating_sub(1) / 2);
            if caret < top || m.distance(top, caret) < margin {
                top = m.step(caret, -(margin as isize));
            } else if m.distance(top, caret) + margin >= rows {
                top = m.step(caret, -((rows - 1 - margin) as isize));
            }
            if !wraps {
                x = geo::follow_x(x, m.cells_of(head), g.cols());
            }
        }
        let top = m.step(top, 0);
        let mut visible = caret >= top && m.distance(top, caret) < rows;
        if wraps {
            x = 0;
        } else {
            let cells = m.cells_of(head);
            visible &= cells >= x && cells < x + g.cols();
        }
        self.st.seen_head = Some(head);
        self.st.seen_version = version;
        self.scroll_to(top, x);
        self.st.caret_visible = visible;
    }

    /// The caret's rectangle (at the composition start while composing).
    fn caret_rect(&mut self, g: &Geometry) -> Rect {
        let at = self
            .doc
            .model
            .composition
            .as_ref()
            .map_or(self.doc.model.sel.head, |c| c.start);
        let at = clamp_offset(self.text(), at);
        let (top, wraps, x_cells) = (self.st.top, self.wraps(), self.st.x_cells);
        let mut m = self.measure();
        let pos = m.pos_of(at);
        let cells = m.cells_of(at);
        let origin = if wraps { m.start(pos).cells } else { x_cells };
        let i = if pos >= top {
            m.distance(top, pos) as f32
        } else {
            -(m.distance(pos, top) as f32)
        };
        Rect::from_min_size(
            pos2(
                g.cell_x(cells, origin),
                g.bounds.top() + i * g.metrics.row_h,
            ),
            vec2(2.0, g.metrics.row_h),
        )
    }

    fn visible_rows(&mut self, g: &Geometry) -> Vec<Row> {
        let wraps = self.wraps();
        let (x0, x1) = (self.st.x_cells, self.st.x_cells + g.cols() + 1);
        let line_count = self.line_count();
        let mut pos = self.st.top;
        let mut out = Vec::new();
        let mut m = self.measure();
        for i in 0..g.drawn_rows() {
            if pos.line > line_count || pos.line > m.text.line_count() {
                break;
            }
            let start = m.start(pos);
            let end = m.end(pos);
            let last = m.last(pos);
            let cells = if wraps {
                lines::walk_from(
                    m.text,
                    &m.cfg,
                    start.offset..end,
                    (start.offset, start.cells),
                    start.cells,
                    usize::MAX,
                )
            } else {
                lines::walk(m.text, &m.cfg, m.ck, pos.line, x0, x1)
            };
            let text = match (cells.placed.first(), cells.placed.last()) {
                (Some(a), Some(b)) if b.range.end - a.range.start <= MAX_ROW_TEXT => {
                    let mut s = String::with_capacity(b.range.end - a.range.start);
                    m.text.read(a.range.start..b.range.end, &mut s);
                    Some(s)
                }
                (None, None) => Some(String::new()),
                _ => None,
            };
            out.push(Row {
                pos,
                y: g.row_y(i),
                origin: if wraps { start.cells } else { x0 },
                cells,
                last,
                text,
            });
            let next = m.step(pos, 1);
            if next == pos {
                break;
            }
            pos = next;
        }
        out
    }

    fn paint(&mut self, g: &Geometry, font: &FontId) {
        let rows = self.visible_rows(g);
        self.st.max_cells = if self.wraps() {
            0
        } else {
            rows.iter()
                .map(|r| r.cells.end_cells.unwrap_or(self.st.x_cells + 2 * g.cols()))
                .max()
                .unwrap_or(0)
        };
        let caret = self.caret_rect(g);
        let tints = self.tints();
        let vbar = self.vbar(g);
        let row_font_h = self.ui.ctx().fonts_mut(|f| f.row_height(font));
        let painter = self.ui.painter_at(g.bounds);
        painter.rect_filled(g.bounds, 0.0, self.palette.background);
        let text_painter = painter.with_clip_rect(g.text_rect().intersect(g.bounds));
        let d = Draw {
            p: self,
            g,
            rows: &rows,
            font,
            row_font_h,
            tints,
            vbar,
        };
        d.backgrounds(&text_painter);
        d.text(&text_painter);
        d.squiggles(&text_painter);
        d.gutter(&painter);
        d.preedit(&painter, caret);
        d.remote_carets(&text_painter);
        d.own_caret(&text_painter, caret);
        d.scrollbars(&painter);
        d.hover_labels(&painter);
        let live = self
            .st
            .tint_seen
            .values()
            .map(|t| t + TINT)
            .filter(|e| *e > self.now)
            .fold(f64::INFINITY, f64::min);
        if live.is_finite() {
            self.ui
                .ctx()
                .request_repaint_after_secs((live - self.now) as f32);
        }
        // A cold highlight seek is time-sliced: keep drawing until it catches up.
        if self.doc.highlight.is_some_and(|h| h.behind()) {
            self.ui.ctx().request_repaint();
        }
    }

    /// Spans other origins inserted, tinted for [`TINT`] after first drawn.
    fn tints(&mut self) -> Vec<(Range<usize>, OriginKind)> {
        let now = self.now;
        let markers = &self.doc.model.markers.changed;
        let seen = &mut self.st.tint_seen;
        seen.retain(|rev, _| markers.iter().any(|m| m.2 == *rev));
        let mut live = Vec::new();
        for (range, origin, rev) in markers {
            let first = *seen.entry(*rev).or_insert(now);
            if !range.is_empty() && now - first < TINT {
                live.push((range.clone(), origin.kind));
            }
        }
        live
    }

    fn output(&mut self, g: &Geometry) {
        let caret = self.caret_rect(g);
        if self.response.has_focus() {
            let interrupt = self.st.ime.take_interrupt();
            self.ui.ctx().output_mut(|o| {
                o.ime = Some(IMEOutput {
                    purpose: IMEPurpose::Normal,
                    rect: g.text_rect(),
                    cursor_rect: caret,
                    should_interrupt_composition: interrupt,
                })
            });
        }
        let report = LayoutReport {
            editor: rect4(g.bounds),
            gutter_width: g.gutter_w,
            row_height: g.metrics.row_h,
            cell_width: g.metrics.cell_w,
            first_line: self.st.top.line,
            visible_rows: g.full_rows(),
            caret: rect4(caret),
        };
        if self.st.last_report != Some(report) {
            self.st.last_report = Some(report);
            self.events.push(Event::Layout(report));
        }
    }
}

fn rect4(r: Rect) -> [f32; 4] {
    [r.left(), r.top(), r.width(), r.height()]
}

fn sel_range(s: Selection) -> Range<usize> {
    s.anchor.min(s.head)..s.anchor.max(s.head)
}

fn rank(s: Severity) -> u8 {
    match s {
        Severity::Note => 0,
        Severity::Warning => 1,
        Severity::Error => 2,
    }
}

fn floor_boundary(s: &str, max: usize) -> &str {
    let mut i = max.min(s.len());
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    &s[..i]
}

fn ago(secs: u64) -> String {
    match secs {
        0..=59 => format!("{secs}s ago"),
        60..=3599 => format!("{}m ago", secs / 60),
        _ => format!("{}h ago", secs / 3600),
    }
}

struct Draw<'d, 'u, 'a> {
    p: &'d Pane<'u, 'a>,
    g: &'d Geometry,
    rows: &'d [Row],
    font: &'d FontId,
    /// The font's own row height, centred in the editor row.
    row_font_h: f32,
    /// Live change tints.
    tints: Vec<(Range<usize>, OriginKind)>,
    /// The vertical scrollbar's total and top index ([`Pane::vbar`]).
    vbar: (usize, usize),
}

impl Draw<'_, '_, '_> {
    fn x(&self, row: &Row, cell: usize) -> f32 {
        self.g.cell_x(cell, row.origin)
    }

    /// The first cell past the drawn window: offsets beyond the measured
    /// part of a long row measure here, never at an unbounded cell.
    fn x1(&self, row: &Row) -> usize {
        row.origin + self.g.cols() + 2
    }

    fn row_rect(&self, row: &Row, left: f32, right: f32) -> Rect {
        Rect::from_min_max(pos2(left, row.y), pos2(right, row.y + self.g.metrics.row_h))
    }

    /// The row holding `offset` (a soft break belongs to the next row).
    fn row_of(&self, offset: usize) -> Option<&Row> {
        self.rows.iter().find(|r| {
            let span = &r.cells.content;
            (span.start <= offset && offset < span.end) || (offset == span.end && r.last)
        })
    }

    /// `[left, right)` of the part of `range` on `row`; a range running past
    /// the line end covers half a cell more (the newline).
    fn span_on(&self, row: &Row, range: &Range<usize>) -> Option<(f32, f32)> {
        let span = &row.cells.content;
        let text = self.p.doc.text;
        let next = if row.last {
            text.line_start(row.pos.line + 1).unwrap_or(usize::MAX)
        } else {
            span.end
        };
        if range.is_empty() || range.start >= next || range.end <= span.start {
            return None;
        }
        let x1 = self.x1(row);
        let a = row.cells.cell_of(range.start.max(span.start), x1);
        let b = row.cells.cell_of(range.end.min(span.end), x1);
        let left = self.x(row, a);
        let mut right = self.x(row, b);
        if row.last && range.end > span.end && next != usize::MAX {
            right += self.g.metrics.cell_w * 0.5;
        }
        (right > left).then_some((left, right))
    }

    fn fill_range(&self, painter: &Painter, range: &Range<usize>, colour: Color32) {
        for row in self.rows {
            if let Some((a, b)) = self.span_on(row, range) {
                painter.rect_filled(self.row_rect(row, a, b), 0.0, colour);
            }
        }
    }

    fn origin_colour(&self, origin: &Origin) -> Color32 {
        self.kind_colour(origin.kind)
    }

    fn kind_colour(&self, kind: OriginKind) -> Color32 {
        match kind {
            OriginKind::Human => self.p.palette.human_other,
            OriginKind::Agent | OriginKind::Tool => self.p.palette.agent,
        }
    }

    fn backgrounds(&self, painter: &Painter) {
        let pal = self.p.palette;
        let model = self.p.doc.model;
        let t = self.g.text_rect();
        let sel = sel_range(model.sel);
        if sel.is_empty()
            && let Some(row) = self.row_of(sel.start)
        {
            painter.rect_filled(
                self.row_rect(row, t.left(), t.right()),
                0.0,
                pal.current_line,
            );
        }
        // Find matches, under the selection. Ascending: stop past the view.
        let visible = self.rows.first().map_or(0, |r| r.cells.content.start)
            ..self.rows.last().map_or(0, |r| r.cells.content.end + 1);
        let matches = pal.caret.gamma_multiply(crate::palette::MATCH_ALPHA);
        for m in self.p.view.matches.iter().filter(|m| !m.is_empty()) {
            if m.start > visible.end {
                break;
            }
            if m.end > visible.start {
                self.fill_range(painter, m, matches);
            }
        }
        self.fill_range(painter, &sel, pal.selection);
        if self.p.view.remote_carets {
            for (origin, sels) in &model.remote {
                let colour = self
                    .origin_colour(origin)
                    .gamma_multiply(crate::palette::REMOTE_ALPHA);
                for s in sels {
                    self.fill_range(painter, &sel_range(*s), colour);
                }
            }
        }
        for (range, kind) in &self.tints {
            self.fill_range(
                painter,
                range,
                self.kind_colour(*kind)
                    .gamma_multiply(crate::palette::MATCH_ALPHA),
            );
        }
    }

    fn text(&self, painter: &Painter) {
        let doc = self.p.doc;
        let mut budget = SliceBudget::default();
        let mut buf = String::new();
        let dy = ((self.g.metrics.row_h - self.row_font_h) / 2.0).round();
        for row in self.rows {
            // Every visible line is asked for, empty or not, so a cold seek
            // left over from another viewport settles on this one.
            let spans: Vec<(Range<usize>, HlClass)> = doc
                .highlight
                .map(|h| {
                    h.with_spans(doc.text, row.pos.line, &mut budget, |s| {
                        s.map(<[_]>::to_vec).unwrap_or_default()
                    })
                })
                .unwrap_or_default();
            let (Some(first), Some(last)) = (row.cells.placed.first(), row.cells.placed.last())
            else {
                continue;
            };
            let base = first.range.start;
            let bytes = if let Some(bytes) = &row.text {
                bytes.as_str()
            } else {
                buf.clear();
                doc.text.read(base..last.range.end, &mut buf);
                buf.as_str()
            };
            let y = row.y + dy;
            let mut si = 0;
            let mut run: Option<(usize, usize, usize, HlClass)> = None; // start cell, start byte, end byte, class
            let flush = |run: &mut Option<(usize, usize, usize, HlClass)>| {
                if let Some((cell, a, b, class)) = run.take()
                    && a < b
                {
                    self.glyphs(painter, &bytes[a..b], pos2(self.x(row, cell), y), class);
                }
            };
            for pc in &row.cells.placed {
                while si < spans.len() && spans[si].0.end <= pc.range.start {
                    si += 1;
                }
                let class = spans
                    .get(si)
                    .filter(|(r, _)| r.start <= pc.range.start)
                    .map_or(HlClass::Plain, |s| s.1);
                let (a, b) = (pc.range.start - base, pc.range.end - base);
                if pc.is_tab || (pc.ascii && pc.cells == 0) {
                    flush(&mut run);
                    continue;
                }
                if pc.ascii {
                    match &mut run {
                        Some((_, _, end, c)) if *c == class && *end == a => *end = b,
                        _ => {
                            flush(&mut run);
                            run = Some((pc.cell, a, b, class));
                        }
                    }
                } else {
                    flush(&mut run);
                    let slice = &bytes[a..b];
                    let shown = if slice.len() > HUGE_CLUSTER {
                        floor_boundary(slice, CLUSTER_DRAW_CAP)
                    } else {
                        slice
                    };
                    self.glyphs(painter, shown, pos2(self.x(row, pc.cell), y), class);
                }
            }
            flush(&mut run);
            if self.p.view.whitespace {
                self.whitespace(painter, row, bytes, y);
            }
        }
    }

    fn glyphs(&self, painter: &Painter, s: &str, at: Pos2, class: HlClass) {
        let pal = self.p.palette;
        let colour = if class == HlClass::Plain {
            pal.text
        } else {
            pal.hl(class)
        };
        let galley: Arc<Galley> = painter.layout_no_wrap(s.to_owned(), self.font.clone(), colour);
        painter.galley(at, galley, colour);
    }

    /// `·` per space and `→` per tab, in the gutter text colour.
    fn whitespace(&self, painter: &Painter, row: &Row, bytes: &str, y: f32) {
        let Some(first) = row.cells.placed.first() else {
            return;
        };
        let colour = self.p.palette.gutter_text;
        for pc in &row.cells.placed {
            let mark = if pc.is_tab {
                "→"
            } else if pc.ascii
                && &bytes[pc.range.start - first.range.start..pc.range.end - first.range.start]
                    == " "
            {
                "·"
            } else {
                continue;
            };
            let galley = painter.layout_no_wrap(mark.to_owned(), self.font.clone(), colour);
            painter.galley(pos2(self.x(row, pc.cell), y), galley, colour);
        }
    }

    fn squiggles(&self, painter: &Painter) {
        let pal = self.p.palette;
        let cw = self.g.metrics.cell_w;
        let t = self.g.text_rect();
        // Each diagnostic on every visible row it crosses, clipped to the
        // view: the zigzag is drawn in steps, so its length must be bounded.
        let crossings = self.p.doc.diagnostics.iter().flat_map(|d| {
            self.rows.iter().filter_map(move |row| {
                let span = &row.cells.content;
                let (s, e) = (d.range.start, d.range.end);
                // A point diagnostic belongs to the row holding its offset (a
                // line's end is its last row's); a range to every row it
                // overlaps, an empty row counting as one cell wide.
                let crosses = if s == e {
                    (span.start <= s && s < span.end) || (s == span.end && row.last)
                } else {
                    s < span.end.max(span.start + 1) && e > span.start
                };
                crosses.then_some((d, row))
            })
        });
        for (d, row) in crossings {
            let span = &row.cells.content;
            let x1 = self.x1(row);
            let a = self.x(row, row.cells.cell_of(d.range.start.max(span.start), x1));
            let end = d.range.end.min(span.end);
            let b = self.x(row, row.cells.cell_of(end, x1)).max(a + cw);
            let (a, b) = (a.max(t.left() - cw), b.min(t.right() + cw));
            if a >= b {
                continue;
            }
            let base = row.y + self.g.metrics.row_h - 3.0;
            let (colour, dotted) = match d.severity {
                Severity::Error => (pal.error, false),
                Severity::Warning => (pal.warning, false),
                Severity::Note => (pal.note, true),
            };
            let stroke = Stroke::new(1.25, colour);
            let mut x = a;
            let mut up = false;
            while x < b {
                let w = (b - x).min(2.0);
                if dotted {
                    if !up {
                        painter.rect_filled(
                            Rect::from_min_size(pos2(x, base + 1.0), vec2(w.min(1.5), 1.5)),
                            0.0,
                            colour,
                        );
                    }
                } else {
                    let (y0, y1) = if up {
                        (base + 1.5, base)
                    } else {
                        (base, base + 1.5)
                    };
                    painter.line_segment([pos2(x, y0), pos2(x + w, y1)], stroke);
                }
                up = !up;
                x += 2.0;
            }
        }
    }

    fn strip_x(&self) -> f32 {
        let g = self.g;
        g.gutter_rect().left()
            + if g.digits > 0 {
                (g.digits as f32 + 1.0) * g.metrics.cell_w
            } else {
                0.0
            }
    }

    fn gutter(&self, painter: &Painter) {
        let g = self.g;
        let pal = self.p.palette;
        let doc = self.p.doc;
        // The gutter shares the editor background; faint numbers set it apart.
        let gr = g.gutter_rect();
        let cw = g.metrics.cell_w;
        let caret_line = line_of(doc.text, doc.model.sel.head);
        let dy = ((g.metrics.row_h - self.row_font_h) / 2.0).round();
        let gp = painter.with_clip_rect(gr);
        if g.digits > 0 {
            for row in self.rows.iter().filter(|r| r.pos.row == 0) {
                let n = row.pos.line.to_string();
                let x = gr.left() + (g.digits - n.len().min(g.digits)) as f32 * cw + cw * 0.5;
                let colour = if row.pos.line == caret_line {
                    pal.text
                } else {
                    pal.gutter_text
                };
                let galley = gp.layout_no_wrap(n, self.font.clone(), colour);
                gp.galley(pos2(x, row.y + dy), galley, colour);
            }
        }
        let (Some(top), Some(bottom)) = (self.rows.first(), self.rows.last()) else {
            return;
        };
        // Origin strip: lines other origins changed.
        let strip_x = self.strip_x();
        for (range, origin, _) in &doc.model.markers.changed {
            if let Some((y0, y1)) = self.rows_span(range, top, bottom) {
                gp.rect_filled(
                    Rect::from_min_max(pos2(strip_x, y0), pos2(strip_x + STRIP_W, y1)),
                    0.0,
                    self.origin_colour(origin),
                );
            }
        }
        // Lint column: one dot per line, worst severity wins.
        let lint_x = strip_x + STRIP_W + cw * 0.25;
        let mut worst: Vec<(usize, Severity)> = Vec::new();
        for d in doc.diagnostics {
            let line = line_of(doc.text, d.range.start);
            if line < top.pos.line || line > bottom.pos.line {
                continue;
            }
            match worst.iter_mut().find(|(l, _)| *l == line) {
                Some((_, s)) if rank(d.severity) > rank(*s) => *s = d.severity,
                Some(_) => {}
                None => worst.push((line, d.severity)),
            }
        }
        for (line, sev) in worst {
            let Some(row) = self
                .rows
                .iter()
                .find(|r| r.pos.line == line && r.pos.row == 0)
            else {
                continue;
            };
            let colour = match sev {
                Severity::Error => pal.error,
                Severity::Warning => pal.warning,
                Severity::Note => pal.note,
            };
            let r = (cw * 0.5).min(g.metrics.row_h * 0.4) / 2.0;
            gp.circle_filled(pos2(lint_x + r, row.y + g.metrics.row_h / 2.0), r, colour);
        }
    }

    /// The vertical extent of the rows `range` touches, within the view.
    fn rows_span(&self, range: &Range<usize>, top: &Row, bottom: &Row) -> Option<(f32, f32)> {
        let text = self.p.doc.text;
        // The end is exclusive: a span ending at a line start leaves that
        // line alone. An empty span (a deletion) marks its own line.
        let last = if range.is_empty() {
            range.start
        } else {
            range.end - 1
        };
        let l0 = line_of(text, range.start).max(top.pos.line);
        let l1 = line_of(text, last).min(bottom.pos.line);
        if l0 > l1 {
            return None;
        }
        let y0 = self.rows.iter().find(|r| r.pos.line >= l0)?.y;
        let y1 = self.rows.iter().rev().find(|r| r.pos.line <= l1)?.y + self.g.metrics.row_h;
        Some((y0, y1))
    }

    fn preedit(&self, painter: &Painter, caret: Rect) {
        let st = &self.p.st;
        if !st.ime.active() || !self.g.bounds.contains(caret.center()) {
            return;
        }
        let pal = self.p.palette;
        let galley = painter.layout_no_wrap(st.ime.preedit.clone(), self.font.clone(), pal.text);
        let w = galley.size().x;
        let dy = ((self.g.metrics.row_h - self.row_font_h) / 2.0).round();
        painter.rect_filled(
            Rect::from_min_size(caret.min, vec2(w, caret.height())),
            0.0,
            pal.background,
        );
        painter.galley(pos2(caret.left(), caret.top() + dy), galley, pal.text);
        painter.rect_filled(
            Rect::from_min_size(pos2(caret.left(), caret.bottom() - 2.0), vec2(w, 1.0)),
            0.0,
            pal.caret,
        );
    }

    fn remote_carets(&self, painter: &Painter) {
        if !self.p.view.remote_carets {
            return;
        }
        for (origin, sels) in &self.p.doc.model.remote {
            let colour = self.origin_colour(origin);
            for s in sels {
                let Some(row) = self.row_of(s.head) else {
                    continue;
                };
                let x = self.x(row, row.cells.cell_of(s.head, self.x1(row)));
                painter.rect_filled(
                    Rect::from_min_size(pos2(x, row.y), vec2(2.0, self.g.metrics.row_h)),
                    0.0,
                    colour,
                );
                painter.rect_filled(
                    Rect::from_min_size(pos2(x - 2.0, row.y), vec2(6.0, 3.0)),
                    0.0,
                    colour,
                );
            }
        }
    }

    fn own_caret(&self, painter: &Painter, caret: Rect) {
        let st = &self.p.st;
        if st.ime.active() || !self.g.text_rect().expand(1.0).contains(caret.left_top()) {
            return;
        }
        let pal = self.p.palette;
        if !self.p.response.has_focus() {
            painter.rect_filled(
                Rect::from_min_size(caret.min, vec2(1.0, caret.height())),
                0.0,
                pal.caret.gamma_multiply(0.5),
            );
        } else if self.p.doc.model.overwrite {
            painter.rect_filled(
                Rect::from_min_size(caret.min, vec2(self.g.metrics.cell_w, caret.height())),
                0.0,
                pal.caret.gamma_multiply(0.45),
            );
        } else {
            painter.rect_filled(caret, 0.0, pal.caret);
        }
    }

    fn scrollbars(&self, painter: &Painter) {
        let g = self.g;
        let p = self.p;
        let colour = p.palette.gutter_text.gamma_multiply(0.55);
        let rows = g.full_rows();
        let v = g.vbar_track();
        let (total, first) = self.vbar;
        if let Some((off, len)) = geo::thumb(v.height(), total, rows, first) {
            let r = Rect::from_min_size(
                pos2(v.left() + 2.0, v.top() + off),
                vec2(v.width() - 4.0, len),
            );
            painter.rect_filled(r, (v.width() - 4.0) / 2.0, colour);
        }
        if p.hbar_shown(g) {
            let h = g.hbar_track();
            if let Some((off, len)) = geo::thumb(h.width(), p.h_extent(g), g.cols(), p.st.x_cells) {
                let r = Rect::from_min_size(
                    pos2(h.left() + off, h.top() + 2.0),
                    vec2(len, h.height() - 4.0),
                );
                painter.rect_filled(r, (h.height() - 4.0) / 2.0, colour);
            }
        }
    }

    /// Labels on hover: who owns a remote caret, and who changed lines
    /// marked in the origin strip.
    fn hover_labels(&self, painter: &Painter) {
        let Some(p) = self.p.response.hover_pos() else {
            return;
        };
        let g = self.g;
        let doc = self.p.doc;
        if self.p.view.remote_carets && g.text_rect().contains(p) {
            for (origin, sels) in &doc.model.remote {
                for s in sels {
                    let Some(row) = self.row_of(s.head) else {
                        continue;
                    };
                    let x = self.x(row, row.cells.cell_of(s.head, self.x1(row)));
                    if p.y >= row.y
                        && p.y < row.y + g.metrics.row_h
                        && (p.x - x).abs() <= g.metrics.cell_w
                    {
                        let above = if row.y - g.metrics.row_h >= g.bounds.top() {
                            row.y - g.metrics.row_h
                        } else {
                            row.y + g.metrics.row_h
                        };
                        self.chip(
                            painter,
                            &origin.to_string(),
                            pos2(x, above),
                            self.origin_colour(origin),
                        );
                        return;
                    }
                }
            }
        }
        let strip_x = self.strip_x();
        if p.x < strip_x - 2.0 || p.x > strip_x + STRIP_W + g.metrics.cell_w {
            return;
        }
        let (Some(top), Some(bottom)) = (self.rows.first(), self.rows.last()) else {
            return;
        };
        for (range, origin, rev) in &doc.model.markers.changed {
            if let Some((y0, y1)) = self.rows_span(range, top, bottom)
                && p.y >= y0
                && p.y < y1
            {
                let when = self.p.st.tint_seen.get(rev).map_or(String::new(), |t| {
                    format!(" · {}", ago((self.p.now - t).max(0.0) as u64))
                });
                let label = format!("{origin} · rev {rev}{when}");
                self.chip(
                    painter,
                    &label,
                    pos2(strip_x + STRIP_W + 4.0, p.y + 12.0),
                    self.origin_colour(origin),
                );
                return;
            }
        }
    }

    /// A small label: background, accent edge, text.
    fn chip(&self, painter: &Painter, label: &str, at: Pos2, accent: Color32) {
        let pal = self.p.palette;
        let galley = painter.layout_no_wrap(label.to_owned(), self.font.clone(), pal.text);
        let pad = self.g.metrics.cell_w * 0.5;
        let size = vec2(galley.size().x + 2.0 * pad, self.g.metrics.row_h);
        let b = self.g.bounds;
        let x = at.x.min(b.right() - size.x - 2.0).max(b.left());
        let rect = Rect::from_min_size(pos2(x, at.y), size);
        painter.rect(
            rect,
            4.0,
            pal.background,
            Stroke::new(1.0, accent),
            egui::StrokeKind::Inside,
        );
        let dy = ((self.g.metrics.row_h - self.row_font_h) / 2.0).round();
        painter.galley(pos2(x + pad, at.y + dy), galley, pal.text);
    }
}
