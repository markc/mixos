// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The window's state around the core's tab set: what is on screen, how
//! each pane is sized, the keyboard, pointer, input method and clipboard
//! gestures, and the font.
//!
//! The tab set (`term_core::tabs`) is the model, shared with the Bus
//! thread: a `term.pane.split` from another node and a Ctrl+Shift+E at the
//! keyboard produce the same tree. Everything here is the window's view of
//! it, refreshed by [`App::sync`] whenever the core wakes the window.
//!
//! Work that leaves the window (the clipboard, quitting) is queued as an
//! [`Effect`] for the shell, so every path here runs in a test without a
//! display.

use crate::ime::Composition;
use crate::input::{self, Action, Wheel};
use crate::layout::{self, Shape};
use egui::{Key, Modifiers, Pos2};
use settings::terminal::{CursorShape, FocusRing, Terminal as TerminalSettings};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use term_core::config::Cursor;
use term_core::font::FontSize;
use term_core::panes::Geometry;
use term_core::raster::Raster;
use term_core::tabs::{self, CompletionNote, Removed, TabSet};
use term_core::terminal::Key as TerminalKey;

/// Work for the shell: it owns the platform.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    /// Write the clipboard, or the primary selection.
    Write { text: String, primary: bool },
    /// Read the clipboard, or the primary selection, and paste it into
    /// `pane` ([`App::paste`]).
    Read { pane: u64, primary: bool },
    /// Close the window: the tab set is shut.
    Quit,
}

/// What a frame's sync found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Synced {
    Running,
    /// The last tab closed: the window goes.
    Exit,
}

pub struct App {
    pub tabs: Arc<Mutex<TabSet>>,
    pub cleanup: tabs::Cleanup,
    notify: Option<tokio::sync::mpsc::UnboundedSender<CompletionNote>>,
    /// Set by the core's wake on any thread; [`App::take_wake`] consumes it.
    woken: Arc<AtomicBool>,
    /// Tabs and the active pane tree as of the last sync: what is drawn.
    pub shape: Shape,
    /// The pane area as last laid out, in points.
    area: Geometry,
    /// Columns and rows each visible pane's PTY has been told about. A pane
    /// missing here has not been sized yet, which forces its first resize.
    pub grids: HashMap<u64, (u16, u16)>,
    font: FontSize,
    text_scale: f32,
    /// The font raster every pane draws with (shared with the GPU grid).
    pub raster: Arc<Mutex<Raster>>,
    scale: f32,
    /// Physical cell size.
    cell: (u32, u32),
    /// The GPU grid of each visible pane, and how many snapshots it was
    /// handed (a pane leaving the screen drops its grid).
    pub panes: HashMap<u64, term_render::Pane>,
    pub painted: HashMap<u64, u64>,
    /// Panes handed a snapshot whose prepare has not been read back yet.
    prepared: HashSet<u64>,
    /// Each visible pane's caret at its last snapshot, for the input method.
    cursors: HashMap<u64, Option<(usize, usize)>>,
    /// The physical size of each visible pane's last snapshot: the size
    /// its grid is drawn at.
    pub targets: HashMap<u64, (u32, u32)>,
    pub modifiers: Modifiers,
    /// Fractional Ctrl+wheel travel not yet worth a font step.
    pub wheel: f32,
    /// History and zoom gestures never share fractional travel.
    pub scroll_wheel: f32,
    pub scroll_pane: Option<u64>,
    /// Window coordinates of the pointer, surviving a tab change beneath a
    /// stationary pointer.
    pub pointer: Option<Pos2>,
    pub(crate) mouse: crate::mouse::MouseState,
    pub paste_notice: Option<String>,
    pub ime: Composition,
    pub ime_preedit: Option<String>,
    /// The letter of a clipboard chord still held: its autorepeat is
    /// swallowed, as every non-repeating chord's is.
    clipboard_chord: Option<Key>,
    pub keyboard_focus: bool,
    force_paint: bool,
    pub effects: Vec<Effect>,
    /// The window's own theme choice (View › Theme).
    pub theme: toolkit::theme_menu::Choice,
    /// The Keyboard Shortcuts dialog is open.
    pub keys_dialog: bool,
    pub macros_open: bool,
    /// The About dialog is open.
    pub about_dialog: bool,
    /// Text the platform read for a Ctrl+Shift+V, for the paste command.
    pasted: Option<String>,
    /// The tab or pane a targeted command (tab.select, tab.close,
    /// pane.focus) acts on, set by the strip or `term.execute`; taken by
    /// the command.
    pub target_tab: Option<u64>,
    pub target_pane: Option<u64>,
    /// Commands the window ran itself (a click focusing a pane), for the
    /// shell to publish.
    pub noted: Vec<&'static str>,
    /// Each pane's border and padding.
    edges: layout::Edges,
    /// The face the raster draws: family (None is the bundled font) and
    /// CSS weight.
    face: (Option<String>, u32),
    /// A face whose raster could not be built, reported once; the next
    /// look retries it.
    face_failed: Option<(Option<String>, u32)>,
    /// Points of touchpad travel, or wheel lines × this / 40, per history
    /// line ([`input::scroll_steps`]).
    scroll_speed: f32,
    /// A finished mouse selection is copied to the clipboard as well.
    pub(crate) copy_on_select: bool,
    /// Answer OSC 52 clipboard queries (`terminal.clipboard.osc52_read`).
    pub(crate) osc52_read: bool,
    /// The focused pane's ring, and how much of the accent it takes.
    pub focus_ring: (FocusRing, f32),
    /// The cursor blinks while the window has the keyboard.
    blink: bool,
    /// When the blink cycle (shown first) began: typing restarts it.
    blink_epoch: Instant,
    /// The pane last drawn with its cursor blinked off.
    blink_hidden: Option<u64>,
    /// The instant the last paint chose the blink phase at.
    blink_painted: Instant,
    /// A size the person zoomed to, for the shell to write to the
    /// settings (`terminal.font.size`).
    pub font_write: Option<f32>,
    /// The person reset the size to the settings' own: the shell drops a
    /// zoom not yet written rather than writing anything new.
    pub font_reset: bool,
}

/// Half a blink cycle: the cursor is shown, then hidden, this long each.
pub const BLINK: Duration = Duration::from_millis(530);

impl App {
    pub fn new(
        tabs: Arc<Mutex<TabSet>>,
        cleanup: tabs::Cleanup,
        notify: Option<tokio::sync::mpsc::UnboundedSender<CompletionNote>>,
        woken: Arc<AtomicBool>,
        raster: Raster,
        font: FontSize,
    ) -> Self {
        let (scale, cell) = (raster.scale, (raster.width, raster.height));
        Self {
            tabs,
            cleanup,
            notify,
            woken,
            shape: Shape::default(),
            area: Geometry {
                x: 0.0,
                y: 0.0,
                w: 0.0,
                h: 0.0,
            },
            grids: HashMap::new(),
            font,
            text_scale: 1.0,
            raster: Arc::new(Mutex::new(raster)),
            scale,
            cell,
            panes: HashMap::new(),
            painted: HashMap::new(),
            prepared: HashSet::new(),
            cursors: HashMap::new(),
            targets: HashMap::new(),
            modifiers: Modifiers::NONE,
            wheel: 0.0,
            scroll_wheel: 0.0,
            scroll_pane: None,
            pointer: None,
            mouse: Default::default(),
            paste_notice: None,
            ime: Composition::default(),
            ime_preedit: None,
            clipboard_chord: None,
            keyboard_focus: true,
            force_paint: false,
            effects: Vec::new(),
            theme: Default::default(),
            keys_dialog: false,
            macros_open: false,
            about_dialog: false,
            pasted: None,
            target_tab: None,
            target_pane: None,
            noted: Vec::new(),
            edges: layout::Edges::default(),
            face: (None, u32::from(term_core::raster::DEFAULT_WEIGHT)),
            face_failed: None,
            scroll_speed: input::DEFAULT_SCROLL_SPEED,
            copy_on_select: false,
            osc52_read: false,
            focus_ring: {
                let focus = TerminalSettings::default().focus;
                (focus.ring, focus.strength as f32)
            },
            blink: false,
            blink_epoch: Instant::now(),
            blink_hidden: None,
            blink_painted: Instant::now(),
            font_write: None,
            font_reset: false,
        }
    }

    /// The core woke the window since the last call.
    pub fn take_wake(&self) -> bool {
        self.woken.swap(false, Ordering::AcqRel)
    }

    pub fn scale(&self) -> f32 {
        self.scale
    }

    /// Physical cell size.
    pub fn cell(&self) -> (u32, u32) {
        self.cell
    }

    /// Cell size in points.
    pub fn logical_cell(&self) -> (f32, f32) {
        (
            self.cell.0 as f32 / self.scale,
            self.cell.1 as f32 / self.scale,
        )
    }

    pub fn font(&self) -> FontSize {
        self.font
    }

    /// Larger text multiplies the raster size; zoom and saved font px stay
    /// in the terminal's unscaled units.
    pub fn set_text_scale(&mut self, scale: f32) {
        let scale = if scale.is_nan() {
            1.0
        } else {
            scale.clamp(0.5, 3.0)
        };
        if self.text_scale == scale {
            return;
        }
        let raster = self
            .raster
            .lock()
            .expect("raster")
            .resized(self.scale, self.font.current() * scale);
        match raster {
            Ok(raster) => {
                self.text_scale = scale;
                self.replace_raster(raster);
            }
            Err(error) => eprintln!("term: larger text: {error}"),
        }
    }

    pub fn area(&self) -> Geometry {
        self.area
    }

    pub fn edges(&self) -> layout::Edges {
        self.edges
    }

    /// From a pane's outer edge to its grid, in points.
    pub fn inset(&self) -> f32 {
        layout::inset(self.scale, self.edges)
    }

    /// Apply the profile's `terminal.*` settings. `family` is the face they
    /// name (the terminal's own, else the session's monospace family).
    /// With `keep_size` the window's size stands: a zoom not yet written
    /// back. Shells already running keep their history capacity and
    /// `TERM`; the next tab or pane starts with the new ones.
    pub fn apply_terminal(
        &mut self,
        terminal: &TerminalSettings,
        family: Option<&str>,
        keep_size: bool,
    ) {
        // The font: one raster rebuild for a new face, size or both. The
        // settings' size is what reset returns to, even while a zoom stands.
        let mut font = self.font;
        let resized = if keep_size {
            font.configure(terminal.font.size as f32);
            false
        } else {
            font.set(terminal.font.size as f32)
        };
        let face = (family.map(str::to_owned), terminal.font.weight);
        let mut rebuilt = None;
        if face != self.face {
            let weight = u16::try_from(terminal.font.weight).unwrap_or(400);
            let cursor = self.raster.lock().expect("raster").cursor();
            match Raster::with_face(
                self.scale,
                font.current() * self.text_scale,
                cursor,
                family,
                weight,
            ) {
                Ok(raster) => {
                    self.face = face;
                    self.face_failed = None;
                    rebuilt = Some(Ok(raster));
                }
                Err(error) => {
                    // Not recorded as current, so the next look retries it;
                    // reported once per face. The size still applies.
                    if self.face_failed.as_ref() != Some(&face) {
                        eprintln!("term: font face: {error}");
                        self.face_failed = Some(face);
                    }
                }
            }
        }
        if rebuilt.is_none() && resized {
            rebuilt = Some(
                self.raster
                    .lock()
                    .expect("raster")
                    .resized(self.scale, font.current() * self.text_scale),
            );
        }
        match rebuilt {
            Some(Ok(raster)) => {
                self.font = font;
                self.replace_raster(raster);
            }
            Some(Err(error)) => {
                // The drawn size stands; the reset target is the settings'.
                eprintln!("term: font: {error}");
                self.font.configure(font.configured());
            }
            None => self.font = font,
        }
        let cursor = match terminal.cursor.shape {
            CursorShape::Block => Cursor::Block,
            CursorShape::Underline => Cursor::Underline,
            CursorShape::Bar => Cursor::Bar,
        };
        {
            let mut raster = self.raster.lock().expect("raster");
            if raster.cursor() != cursor {
                raster.set_cursor(cursor);
                self.force_paint = true;
            }
        }
        // A new border or padding resizes every PTY once.
        let edges = layout::Edges {
            border: terminal.border.width,
            padding: terminal.padding,
        };
        if edges != self.edges {
            self.edges = edges;
            self.grids.clear();
            self.relayout();
            self.force_paint = true;
        }
        self.blink = terminal.cursor.blink;
        self.focus_ring = (terminal.focus.ring, terminal.focus.strength as f32);
        self.copy_on_select = terminal.mouse.copy_on_select;
        self.osc52_read = terminal.clipboard.osc52_read;
        self.scroll_speed = terminal.scroll.speed as f32;
        let mut tabs = self.tabs.lock().expect("tabs");
        let mut next = tabs.settings().clone();
        next.config.scrollback = terminal.scrollback.lines as usize;
        next.config.cursor = cursor;
        next.config.font_px = self.font.current();
        if settings::terminal::valid_term(&terminal.env.term) {
            next.term.clone_from(&terminal.env.term);
        } else {
            eprintln!(
                "term: TERM {:?} refused; new shells keep {}",
                terminal.env.term, next.term
            );
        }
        if *tabs.settings() != next {
            tabs.set_settings(next);
        }
    }

    /// Whether the cursor blinks: blink is on and the window has the
    /// keyboard. Otherwise it is steady.
    fn blinking(&self) -> bool {
        self.blink && self.keyboard_focus
    }

    /// Whether the focused pane's cursor is in the shown half of its blink
    /// at `now` (always, while it does not blink).
    pub fn cursor_shown(&self, now: Instant) -> bool {
        let halves =
            now.saturating_duration_since(self.blink_epoch).as_millis() / BLINK.as_millis();
        !self.blinking() || halves.is_multiple_of(2)
    }

    /// How long from `now` until the focused pane's cursor next blinks, for
    /// a scheduled repaint; None while nothing blinks (blink off, no
    /// keyboard, or the program hid its cursor), so nothing repaints.
    ///
    /// Counted from the phase the last paint drew, not the phase at `now`:
    /// a paint just before an edge and this call just after it must still
    /// wake for that edge (at once), not a half cycle later.
    pub fn next_blink(&self, now: Instant) -> Option<Duration> {
        let visible = matches!(self.cursors.get(&self.shape.active_pane), Some(Some(_)));
        if !self.blinking() || !visible {
            return None;
        }
        let drawn = self.blink_painted.min(now);
        let half = BLINK.as_nanos();
        let halves = drawn.saturating_duration_since(self.blink_epoch).as_nanos() / half;
        let edge = self.blink_epoch + Duration::from_nanos(((halves + 1) * half) as u64);
        Some(edge.saturating_duration_since(now))
    }

    /// Every visible pane's outer rectangle, in tree order.
    pub fn placed(&self) -> Vec<(u64, Geometry)> {
        self.shape
            .tree
            .as_ref()
            .map(|tree| layout::panes(tree, self.area, self.scale))
            .unwrap_or_default()
    }

    /// Whether the tab set holds no terminal yet, or any more.
    pub fn is_empty(&self) -> bool {
        self.tabs.lock().expect("tabs").is_empty()
    }

    /// Everything a wake can mean, in one place: reap exited shells, follow
    /// the tab set's shape (a Bus command may have changed it), and size any
    /// pane that needs it. Painting is [`App::paint`]'s.
    pub fn sync(&mut self) -> Synced {
        let (removed, notes) = self.tabs.lock().expect("tabs").reap_exited();
        self.sync_ime();
        self.cancel_hidden_gesture();
        self.cleanup.submit(removed);
        if let Some(notify) = &self.notify {
            for note in notes {
                let _ = notify.send(note);
            }
        }
        let shape = {
            let mut tabs = self.tabs.lock().expect("tabs");
            if tabs.is_empty() {
                if tabs.is_starting() {
                    return Synced::Running;
                }
                return Synced::Exit;
            }
            // A program's OSC title becomes its tab's.
            tabs.refresh_titles();
            Shape::of(&tabs)
        };
        let visible = shape.visible();
        self.panes.retain(|id, _| visible.contains(id));
        self.painted.retain(|id, _| visible.contains(id));
        self.prepared.retain(|id| visible.contains(id));
        self.cursors.retain(|id, _| visible.contains(id));
        self.targets.retain(|id, _| visible.contains(id));
        self.grids.retain(|id, _| visible.contains(id));
        self.shape = shape;
        self.relayout();
        Synced::Running
    }

    /// Lay the panes out in `area` (points), resizing any PTY whose grid
    /// changed.
    pub fn layout(&mut self, area: Geometry) {
        // An unsized grid forces a layout even when the area is unchanged:
        // otherwise a first frame that matches the default size never
        // sizes a pane at all.
        if self.area == area
            && self
                .shape
                .visible()
                .iter()
                .all(|id| self.grids.contains_key(id))
        {
            return;
        }
        self.area = area;
        self.relayout();
    }

    /// Size every visible pane from the area and tell each PTY that moved.
    fn relayout(&mut self) {
        let Some(tree) = &self.shape.tree else {
            return; // Nothing synced yet; the first wake lays out.
        };
        let (scale, cell) = (self.scale, self.cell);
        if cell.0 == 0 || cell.1 == 0 || self.area.w <= 0.0 || self.area.h <= 0.0 {
            return;
        }
        let placed = layout::panes(tree, self.area, scale);
        let mut resize = Vec::new();
        {
            let mut tabs = self.tabs.lock().expect("tabs");
            if tabs.is_empty() {
                return;
            }
            for (id, geometry) in &placed {
                // Points relative to the pane area: what `term.panes`
                // reports and `focus_dir` reads.
                tabs.geometry(
                    *id,
                    Geometry {
                        x: geometry.x - self.area.x,
                        y: geometry.y - self.area.y,
                        ..*geometry
                    },
                );
                let grid = layout::grid(*geometry, cell, scale, self.edges);
                if self.grids.get(id) != Some(&grid)
                    && let Some(terminal) = tabs.pane_by_id(*id)
                {
                    resize.push((*id, grid, terminal));
                }
            }
        }
        // Record a grid only once it has reached its PTY, so a failed lookup
        // never suppresses the retry. Terminal locks are taken without the
        // set lock, as everywhere else in this frontend.
        for (id, (cols, rows), terminal) in resize {
            // Physical pixels to the PTY: TIOCSWINSZ's ws_xpixel is what a
            // full-screen program asks for when it wants real geometry.
            terminal.lock().expect("terminal").resize(
                cols,
                rows,
                cols.saturating_mul(cell.0 as u16),
                rows.saturating_mul(cell.1 as u16),
            );
            self.tabs.lock().expect("tabs").resized(id, cols, rows);
            self.grids.insert(id, (cols, rows));
        }
    }

    /// Hand every visible pane whose terminal changed its new snapshot (all
    /// of them after a zoom or a new raster). A clean pane costs one damage
    /// check.
    pub fn paint(&mut self) {
        // The focused pane's cursor in the hidden half of its blink; a pane
        // whose blink phase changed repaints with no new output.
        self.blink_painted = Instant::now();
        let hide = (!self.cursor_shown(self.blink_painted)).then_some(self.shape.active_pane);
        let blinked = std::mem::replace(&mut self.blink_hidden, hide);
        let terminals: Vec<_> = {
            let tabs = self.tabs.lock().expect("tabs");
            self.shape
                .visible()
                .into_iter()
                .filter_map(|id| tabs.pane_by_id(id).map(|terminal| (id, terminal)))
                .collect()
        };
        for (id, terminal) in terminals {
            // The renderer has drawn the snapshot handed over last time:
            // its prepare cost is known now.
            let prepare = self
                .prepared
                .remove(&id)
                .then(|| self.panes.get(&id))
                .flatten()
                .map(|pane| Duration::from_nanos(pane.stats().prepare_ns));
            let terminal = terminal.lock().expect("terminal");
            // Consume before capture: a write racing capture is either in
            // this snapshot, or wakes the window again.
            let dirty = terminal.take_damage();
            let blink = (hide == Some(id)) != (blinked == Some(id));
            if !dirty && !self.force_paint && !blink && self.panes.contains_key(&id) {
                if let Some(prepare) = prepare {
                    let mut stats = terminal.stats.lock().expect("metrics");
                    stats.gpu = true;
                    stats.uploads += 1;
                    stats.raster_paint.add(prepare);
                }
                continue;
            }
            let snapshot = terminal.grid_snapshot();
            // term.snapshot's DIAGNOSTIC lines: VT notification to this
            // hand-over, the interval between hand-overs, the prepare.
            terminal.stats.lock().expect("metrics").gpu_frame(prepare);
            drop(terminal);
            self.prepared.insert(id);
            let mut screen = snapshot.screen;
            let target = self.raster.lock().expect("raster").target_size(&screen);
            self.targets.insert(id, target);
            let cursor = (screen.cursor_visible
                && screen.cursor.0 < screen.cols
                && screen.cursor.1 < screen.rows)
                .then_some(screen.cursor);
            self.cursors.insert(id, cursor);
            if hide == Some(id) {
                screen.cursor_visible = false;
            }
            let pane = self.panes.entry(id).or_default();
            // A new grid has no rows yet: every row is owed.
            let mut dirty_rows = if self.force_paint || *self.painted.entry(id).or_default() == 0 {
                vec![true; screen.rows]
            } else {
                snapshot.dirty_rows
            };
            if blink
                && let Some((_, row)) = cursor
                && let Some(dirty) = dirty_rows.get_mut(row)
            {
                *dirty = true;
            }
            pane.update(screen, dirty_rows);
            *self.painted.entry(id).or_default() += 1;
        }
        self.force_paint = false;
    }

    /// Where the active pane's caret is, in points: the input method's
    /// cursor area.
    pub fn caret(&self) -> Option<egui::Rect> {
        let id = self.shape.active_pane;
        let pane = self.placed().into_iter().find(|(p, _)| *p == id)?.1;
        let (col, row) = (*self.cursors.get(&id)?)?;
        let (cw, ch) = self.logical_cell();
        let inset = self.inset();
        Some(egui::Rect::from_min_size(
            egui::pos2(
                pane.x + inset + col as f32 * cw,
                pane.y + inset + row as f32 * ch,
            ),
            egui::vec2(cw, ch),
        ))
    }

    /// Run an action: on the tab set, the font, or the clipboard.
    pub fn act(&mut self, action: Action) {
        match action {
            Action::Copy | Action::Paste => self.clipboard_action(action),
            Action::FontIncrease => self.zoom(FontSize::increase),
            Action::FontDecrease => self.zoom(FontSize::decrease),
            Action::FontReset => self.reset_font(),
            _ => {
                if !matches!(action, Action::Scroll(_)) {
                    self.cancel_mouse_gesture();
                }
                let removed = apply(&mut self.tabs.lock().expect("tabs"), action);
                self.cleanup.submit(removed);
                if action == Action::Quit {
                    self.effects.push(Effect::Quit);
                }
                // Every other mutation wakes the window, and the next sync
                // picks up the new shape.
            }
        }
    }

    /// Close the window as Quit does: shut every tab.
    pub fn quit(&mut self) {
        self.cancel_mouse_gesture();
        let removed = self.tabs.lock().expect("tabs").shutdown();
        self.cleanup.submit(removed);
        self.effects.push(Effect::Quit);
    }

    /// foot's behaviour: the window keeps its size and the grid reflows to
    /// the new cell. Every visible pane is redrawn and every PTY is told
    /// its new size.
    pub fn zoom(&mut self, change: impl FnOnce(&mut FontSize) -> bool) {
        let mut font = self.font;
        if !change(&mut font) {
            return;
        }
        // The new size is committed only once a raster for it exists: a
        // failed rebuild keeps the old one, legible, rather than none.
        let rebuilt = self
            .raster
            .lock()
            .expect("raster")
            .resized(self.scale, font.current() * self.text_scale);
        match rebuilt {
            Ok(raster) => {
                self.font = font;
                self.replace_raster(raster);
                // The person's size: the shell writes it to the settings.
                self.font_write = Some(font.current());
            }
            Err(error) => eprintln!("term: font resize: {error}"),
        }
    }

    /// Back to the settings' size (`terminal.font.size`, or the startup
    /// size before settingsd has answered). Nothing new is written: the
    /// settings already hold that size.
    pub fn reset_font(&mut self) {
        self.zoom(FontSize::reset);
        self.font_write = None;
        self.font_reset = true;
    }

    /// Rebuild the raster for the window's device scale, at the CURRENT
    /// font size, so moving the window to another output keeps the zoom.
    pub fn rescale(&mut self, scale: f32) {
        if !scale.is_finite() || scale <= 0.0 || (scale - self.scale).abs() < 0.01 {
            return;
        }
        let rebuilt = self
            .raster
            .lock()
            .expect("raster")
            .resized(scale, self.font.current() * self.text_scale);
        match rebuilt {
            Ok(raster) => self.replace_raster(raster),
            Err(error) => eprintln!("term: raster rebuild at scale {scale}: {error}"),
        }
    }

    fn replace_raster(&mut self, raster: Raster) {
        self.scale = raster.scale;
        self.cell = (raster.width, raster.height);
        *self.raster.lock().expect("raster") = raster;
        // Every PTY resizes to the new cell, and every pane redraws whole.
        self.grids.clear();
        self.relayout();
        self.force_paint = true;
    }

    fn sync_ime(&mut self) {
        if !self.ime.has_owner() {
            return;
        }
        let active = self.active_pane();
        if self.ime.focus(active) {
            self.ime_preedit = None;
        }
    }

    /// The pane the keyboard goes to, if any.
    pub fn active_pane(&self) -> Option<u64> {
        let tabs = self.tabs.lock().expect("tabs");
        (!tabs.is_empty()).then(|| tabs.active_tab().active_pane)
    }

    pub fn send_keys(&mut self, keys: Vec<TerminalKey>) {
        if keys.is_empty() {
            return;
        }
        let tabs = self.tabs.lock().expect("tabs");
        if tabs.is_empty() {
            return;
        }
        tabs.user_activity();
        // The focused pane of the active tab.
        let terminal = tabs.active_terminal();
        drop(tabs);
        // Typing shows a blinking cursor at once.
        self.blink_epoch = Instant::now();
        let terminal = terminal.lock().expect("terminal");
        if let Err(error) = terminal.keys(&keys, Instant::now()) {
            eprintln!("term input: {error}");
        }
    }

    /// Tab: with the active tab split it moves between panes (Shift+Tab
    /// backwards, autorepeat ignored); with one pane it is the shell's Tab.
    /// The panes are read live, not from the last shape: a tab switch
    /// earlier in the same frame has not been synced yet.
    pub fn tab_key(&mut self, forward: bool, repeat: bool) -> Option<Action> {
        let split = {
            let tabs = self.tabs.lock().expect("tabs");
            if tabs.is_empty() {
                return None;
            }
            tabs.leaves().len() > 1
        };
        if split {
            return (!repeat).then_some(Action::CyclePane { forward });
        }
        self.send_keys(vec![TerminalKey::Tab]);
        None
    }

    /// A key press: a terminal chord (returned, for the command registry to
    /// run), or keys for the shell. A chord wins over the shell encoder:
    /// without that order Ctrl+Shift+T would reach the PTY as Ctrl-T.
    pub fn key(&mut self, key: Key, modifiers: Modifiers, repeat: bool) -> Option<Action> {
        if key == Key::Tab && !modifiers.ctrl && !modifiers.alt {
            return self.tab_key(!modifiers.shift, repeat);
        }
        let mut action = input::action_for(key, modifiers);
        // Ordinary typing takes no terminal lock just to decide who owns a
        // scrollback chord.
        if matches!(action, Some(Action::Scroll(_))) && self.alternate_screen() {
            action = input::action_on_screen(action, true);
        }
        if let Some(action) = action {
            // A repeat of a non-repeating chord is swallowed, not passed
            // through: it must not turn into a control code either.
            return (!repeat || action.repeats()).then_some(action);
        }
        self.send_keys(input::keys_for(key, modifiers));
        None
    }

    fn alternate_screen(&self) -> bool {
        let tabs = self.tabs.lock().expect("tabs");
        !tabs.is_empty()
            && tabs
                .active_terminal()
                .lock()
                .expect("terminal")
                .alternate_screen()
    }

    /// Typed text: the shell's, ESC-prefixed under Alt (Meta), and
    /// nothing under Ctrl (the key press carries the chord).
    pub fn text(&mut self, text: &str) {
        self.send_keys(input::typed(text, self.modifiers));
    }

    /// A copy, cut or paste event from the platform (see
    /// [`input::clipboard_event`]). Returns a chord for the registry.
    pub fn clipboard_event(&mut self, event: &egui::Event) -> Option<Action> {
        let letter = match event {
            egui::Event::Copy => Key::C,
            egui::Event::Cut => Key::X,
            _ => Key::V,
        };
        match input::clipboard_event(event, self.modifiers)? {
            input::Clipboard::Keys(keys) => {
                self.send_keys(keys);
                None
            }
            // The platform repeats these with the key held; only the first
            // press is a chord.
            _ if self.clipboard_chord == Some(letter) => None,
            input::Clipboard::Action(action) => {
                self.clipboard_chord = Some(letter);
                Some(action)
            }
            input::Clipboard::Paste(text) => {
                // The platform has read the clipboard already: the paste
                // command pastes this text instead of reading it again.
                self.clipboard_chord = Some(letter);
                self.pasted = Some(text);
                Some(Action::Paste)
            }
        }
    }

    /// A key went up: a held clipboard chord ends with its letter.
    pub fn key_released(&mut self, key: Key) {
        if self.clipboard_chord == Some(key) {
            self.clipboard_chord = None;
        }
    }

    pub fn modifiers_changed(&mut self, modifiers: Modifiers) {
        if modifiers != self.modifiers {
            self.scroll_wheel = 0.0;
        }
        // Letting go of Ctrl ends a Ctrl+wheel gesture: travel short of a
        // step must not carry into the next one and zoom early.
        if !modifiers.ctrl {
            self.wheel = 0.0;
        }
        self.modifiers = modifiers;
    }

    /// A wheel event over pane `id`: Ctrl zooms, otherwise it scrolls the
    /// pane under the pointer (or reports to the program in it).
    /// Returns the font steps a Ctrl+wheel gesture made (positive grows),
    /// for the shell to run as zoom commands.
    pub fn wheel(&mut self, id: u64, wheel: Wheel) -> i32 {
        if self.modifiers.ctrl {
            return input::wheel_steps(&mut self.wheel, wheel);
        }
        self.scroll(id, wheel);
        0
    }

    fn scroll(&mut self, id: u64, wheel: Wheel) {
        if self.scroll_pane != Some(id) {
            self.scroll_wheel = 0.0;
            self.scroll_pane = Some(id);
        }
        let Some(position) = self.pointer else {
            return;
        };
        let Some((_, pane)) = self.placed().into_iter().find(|(pane, _)| *pane == id) else {
            return;
        };
        let Some(&grid) = self.grids.get(&id) else {
            return;
        };
        let cell_height = self.logical_cell().1;
        let lines = input::scroll_steps(
            &mut self.scroll_wheel,
            wheel,
            cell_height,
            self.scroll_speed,
        );
        if lines == 0 {
            return;
        }
        let (col, row) = input::pointer_cell(
            egui::pos2(position.x - pane.x, position.y - pane.y),
            self.inset(),
            self.logical_cell(),
            grid,
        );
        let tabs = self.tabs.lock().expect("tabs");
        let Some(terminal) = tabs.pane_by_id(id) else {
            return;
        };
        tabs.user_activity();
        drop(tabs);
        let terminal = terminal.lock().expect("terminal");
        let mods = self.mouse_modifiers();
        if !terminal.mouse_scroll(col, row, lines, mods) {
            terminal.scroll_wheel(lines, mods);
        }
    }

    pub(crate) fn mouse_modifiers(&self) -> term_core::terminal::MouseModifiers {
        term_core::terminal::MouseModifiers {
            shift: self.modifiers.shift,
            alt: self.modifiers.alt,
            ctrl: self.modifiers.ctrl,
        }
    }

    /// An input method event, for the pane that has the keyboard.
    pub fn ime(&mut self, event: &egui::ImeEvent) {
        if !self.keyboard_focus {
            return;
        }
        match event {
            egui::ImeEvent::Preedit { text, .. } => {
                let active = self.active_pane();
                self.ime_preedit = self.ime.preedit(text, active).then(|| text.clone());
            }
            egui::ImeEvent::Commit(text) => {
                self.ime_preedit = None;
                // Resolve and check the target under one lock: a Bus focus
                // change cannot race the owner check.
                let tabs = self.tabs.lock().expect("tabs");
                let active = (!tabs.is_empty()).then(|| tabs.active_tab().active_pane);
                if self.ime.commit(active) {
                    tabs.user_activity();
                    let terminal = tabs.active_terminal();
                    drop(tabs);
                    if let Err(error) = terminal
                        .lock()
                        .expect("terminal")
                        .keys(&input::text_keys(text), Instant::now())
                    {
                        eprintln!("term input: {error}");
                    }
                }
            }
            _ => {}
        }
    }

    pub fn focused(&mut self) {
        self.tabs.lock().expect("tabs").user_activity();
        self.keyboard_focus = true;
        self.blink_epoch = Instant::now();
    }

    /// The window lost the keyboard. A release that happens while another
    /// window has it is never delivered: a latched Ctrl would turn every
    /// later wheel into a zoom, and a drag would never end.
    pub fn unfocused(&mut self) {
        self.keyboard_focus = false;
        self.ime.cancel();
        self.ime_preedit = None;
        self.cancel_mouse_gesture();
        self.modifiers = Modifiers::NONE;
        self.clipboard_chord = None;
        self.wheel = 0.0;
        self.scroll_wheel = 0.0;
    }

    /// `tab.select`: the targeted tab.
    pub fn select_target_tab(&mut self) {
        if let Some(id) = self.target_tab.take() {
            self.select_tab(id);
        }
    }

    /// `tab.close`: the targeted tab, else the active one.
    pub fn close_target_tab(&mut self) {
        match self.target_tab.take() {
            Some(id) => self.close_tab(id),
            None => self.act(Action::CloseTab),
        }
    }

    /// `pane.focus`: the targeted pane of the active tab.
    pub fn focus_target_pane(&mut self) {
        if let Some(id) = self.target_pane.take() {
            self.cancel_mouse_gesture();
            let mut tabs = self.tabs.lock().expect("tabs");
            tabs.user_activity();
            tabs.focus(id);
            drop(tabs);
            self.sync_ime();
        }
    }

    /// The person picked tab `id` in the strip.
    pub fn select_tab(&mut self, id: u64) {
        self.cancel_mouse_gesture();
        let mut tabs = self.tabs.lock().expect("tabs");
        tabs.user_activity();
        tabs.select(id);
        drop(tabs);
        self.sync_ime();
    }

    /// The person closed tab `id` from the strip.
    pub fn close_tab(&mut self, id: u64) {
        self.cancel_mouse_gesture();
        let removed = {
            let mut tabs = self.tabs.lock().expect("tabs");
            tabs.user_activity();
            tabs.close(id).1
        };
        self.cleanup.submit(removed.into_iter().collect());
    }

    pub fn clipboard_action(&mut self, action: Action) {
        let tabs = self.tabs.lock().expect("tabs");
        if tabs.is_empty() {
            return;
        }
        tabs.user_activity();
        let pane = tabs.active_tab().active_pane;
        if action == Action::Paste {
            drop(tabs);
            match self.pasted.take() {
                Some(text) => self.paste(pane, Some(text)),
                None => self.effects.push(Effect::Read {
                    pane,
                    primary: false,
                }),
            }
            return;
        }
        let text = tabs
            .active_terminal()
            .lock()
            .expect("terminal")
            .selection_text();
        if let Some(text) = text {
            self.effects.push(Effect::Write {
                text,
                primary: false,
            });
        }
    }

    /// Paste `text` into pane `pane`. Ids never recycle: a closed pane drops
    /// a late answer, and changing focus or tabs cannot redirect it.
    pub fn paste(&mut self, pane: u64, text: Option<String>) {
        let Some(text) = text else { return };
        let tabs = self.tabs.lock().expect("tabs");
        let Some(terminal) = tabs.pane_by_id(pane) else {
            return;
        };
        tabs.user_activity();
        drop(tabs);
        let pasted = terminal.lock().expect("terminal").paste(&text);
        match pasted {
            Ok(()) => self.paste_notice = None,
            Err(error) => {
                eprintln!("term: paste: {error}");
                self.paste_notice = Some(error);
            }
        }
    }

    /// The commands' enablement: whether there is a terminal to act on.
    pub fn running(&self) -> bool {
        !self.is_empty()
    }

    /// Whether the active tab is split.
    pub fn split(&self) -> bool {
        let tabs = self.tabs.lock().expect("tabs");
        !tabs.is_empty() && tabs.leaves().len() > 1
    }

    /// Whether there is more than one tab.
    pub fn several_tabs(&self) -> bool {
        self.tabs.lock().expect("tabs").list().len() > 1
    }

    /// The window's state, for `term.actions` callers and tests.
    pub fn state(&self) -> serde_json::Value {
        let (cw, ch) = self.logical_cell();
        serde_json::json!({
            "tabs": self.shape.tabs.iter().map(|t| serde_json::json!({
                "id": t.id, "title": t.title, "active": t.active,
            })).collect::<Vec<_>>(),
            "panes": self.placed().iter().map(|(id, g)| serde_json::json!({
                "id": id,
                "focused": *id == self.shape.active_pane,
                "grid": self.grids.get(id),
                "rect": [g.x, g.y, g.w, g.h],
            })).collect::<Vec<_>>(),
            "font_px": self.font.current(),
            "cell": [cw, ch],
            "scale": self.scale,
            "paste_notice": self.paste_notice,
        })
    }
}

/// Apply a tab or pane chord to the tab set, returning what to tear down.
///
/// Font and clipboard chords are not tab operations and never reach here.
pub fn apply(tabs: &mut TabSet, action: Action) -> Vec<Removed> {
    if tabs.is_empty() {
        return Vec::new();
    }
    tabs.user_activity();
    match action {
        Action::NewTab => {
            if let Err(error) = tabs.open() {
                eprintln!("new tab: {error}");
            }
            Vec::new()
        }
        Action::CloseTab => {
            let id = tabs.active_id();
            tabs.close(id).1.into_iter().collect()
        }
        Action::Quit => tabs.shutdown(),
        Action::Split(dir) => {
            if let Err(error) = tabs.split_active(dir) {
                eprintln!("split pane: {error}");
            }
            Vec::new()
        }
        Action::ClosePane => tabs.close_active().1.into_iter().collect(),
        Action::Focus(direction) => {
            tabs.focus_dir(direction);
            Vec::new()
        }
        Action::Cycle { forward } => {
            tabs.cycle(forward);
            Vec::new()
        }
        Action::CyclePane { forward } => {
            let ids: Vec<_> = tabs.leaves().iter().map(|pane| pane.id).collect();
            if let Some(id) = input::cycle_pane(&ids, tabs.active_tab().active_pane, forward) {
                tabs.focus(id);
            }
            Vec::new()
        }
        Action::Scroll(request) => {
            tabs.active_terminal()
                .lock()
                .expect("terminal")
                .scroll_view(request);
            Vec::new()
        }
        Action::FontIncrease
        | Action::FontDecrease
        | Action::FontReset
        | Action::Copy
        | Action::Paste => Vec::new(),
    }
}
