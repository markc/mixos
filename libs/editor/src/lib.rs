// SPDX-License-Identifier: MIT OR Apache-2.0
//! editor: the shared plain-text editor for MixOS egui applications
//! (AGENTS.md §4.1).
//!
//! A monospace, cell-grid view over the headless editing model
//! (`editor-model`, `edit`). Only visible rows are measured and drawn, so a
//! multi-megabyte file or a multi-megabyte line costs no more per frame than
//! a short one. Soft wrap is a view option ([`Wrap`]).
//!
//! The widget never edits text. It turns keys, pointer, clipboard and input
//! method events into [`Event`]s, mostly [`EditCommand`]s, which the owner of
//! the document applies. Two owners exist:
//!
//! - [`local::Document`]: an in-process document over [`edit::buffer::Buffer`]
//!   with open and save, undo and redo, and highlighting. An application that
//!   needs a text editor (a Markdown editor, a notes or config pane) holds one
//!   and calls [`local::Document::show`].
//! - ced, which applies the same events to a buffer shared through the edit
//!   service (the mirror in `editor-model`), and draws other origins' carets
//!   and changes through the same [`Doc`].
//!
//! Offsets are UTF-8 byte offsets into the view text, lines are 1-based and
//! cells 0-based.

pub mod layout;
pub mod lines;
pub mod local;
pub mod palette;
pub mod rows;

mod ime;
mod input;
mod pane;

use std::ops::Range;

use edit::text::Text;
use edit::view::MeasureCfg;
use editor_model::diag::Diagnostic;
use editor_model::highlight::Highlight;
use editor_model::model::EditorModel;
use serde::{Deserialize, Serialize};

pub use edit::anchor::Selection;
pub use editor_model::highlight::HlClass;
pub use editor_model::model::{EditCommand, Motion, Scroll};
pub use palette::Palette;
pub use pane::show;

/// One document as the widget sees it for one frame: borrowed, never owned.
#[derive(Clone, Copy)]
pub struct Doc<'a> {
    pub text: &'a Text,
    /// Distinguishes documents (tabs, reopened buffers). A change resets the
    /// view's scroll, drag and composition.
    pub identity: u64,
    /// Changes whenever the text changes, including undo and an
    /// equal-length replacement.
    pub revision: u64,
    /// Selection, scroll, overwrite mode, composition, other origins'
    /// selections and change markers.
    pub model: &'a EditorModel,
    pub highlight: Option<&'a Highlight>,
    pub diagnostics: &'a [Diagnostic],
}

/// How long lines are shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Wrap {
    /// One row per line, scrolled horizontally: code.
    #[default]
    None,
    /// Rows break after whitespace where they can, inside a word where it is
    /// longer than a row: prose and Markdown.
    Words,
}

/// How the view is drawn beyond the text and colours. Part of an
/// application's serialisable UI state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct View {
    /// Text size in points; `None` takes the theme's Mono role.
    pub font_size: Option<f32>,
    /// Row height as a multiple of the text size.
    pub line_height: f32,
    pub tab_size: u8,
    pub ambiguous_wide: bool,
    pub wrap: Wrap,
    pub line_numbers: bool,
    /// Draw `·` for spaces and `→` for tabs.
    pub whitespace: bool,
    /// Draw other origins' carets and selections.
    pub remote_carets: bool,
    /// Accept no editing commands; selection, copy and scrolling still work.
    pub read_only: bool,
    /// Find highlight-all ranges, ascending and non-overlapping; drawn under
    /// the selection.
    #[serde(skip)]
    pub matches: Vec<Range<usize>>,
}

impl Default for View {
    fn default() -> Self {
        Self {
            font_size: None,
            line_height: 1.35,
            tab_size: 4,
            ambiguous_wide: false,
            wrap: Wrap::None,
            line_numbers: true,
            whitespace: false,
            remote_carets: true,
            read_only: false,
            matches: Vec::new(),
        }
    }
}

impl View {
    /// The measurement settings this view draws with.
    pub fn measure(&self) -> MeasureCfg {
        MeasureCfg {
            tab_size: self.tab_size,
            ambiguous_wide: self.ambiguous_wide,
        }
    }

    /// The settings for prose: soft wrap and no line numbers.
    pub fn prose() -> Self {
        Self {
            wrap: Wrap::Words,
            line_numbers: false,
            ..Self::default()
        }
    }
}

/// What the widget asks of the document's owner, in order.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// An editing or motion command for [`EditorModel::command`].
    Command(EditCommand),
    /// The view scrolled; store it in the model's `scroll`.
    Scrolled(Scroll),
    /// The input method's preedit changed (empty: it ended). The widget draws
    /// the preedit itself; the owner must set the model's composition
    /// ([`EditorModel::set_preedit`]) before the next frame. A preedit whose
    /// composition is gone by then counts as cancelled by another edit.
    Preedit(String),
    /// Undo or Redo was pressed and nothing else claimed the shortcut.
    Undo,
    Redo,
    /// The widget gained or lost keyboard focus.
    Focus(bool),
    /// The geometry changed (for agents driving the view, and tests).
    Layout(LayoutReport),
}

/// Where the editor drew things in the last frame, in points.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct LayoutReport {
    /// The whole widget: `[x, y, width, height]`.
    pub editor: [f32; 4],
    pub gutter_width: f32,
    pub row_height: f32,
    pub cell_width: f32,
    /// The first line at the top of the view.
    pub first_line: usize,
    /// Rows that fit entirely; Page Up and Page Down move by this.
    pub visible_rows: usize,
    /// The caret: `[x, y, width, height]`.
    pub caret: [f32; 4],
}

/// The result of [`show`].
pub struct Output {
    pub response: egui::Response,
    pub events: Vec<Event>,
}
