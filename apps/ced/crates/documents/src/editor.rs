// SPDX-License-Identifier: MIT OR Apache-2.0
//! What the editor view reports to the controller: editing commands, scroll,
//! clipboard, input-method and focus changes, and its geometry. The window
//! translates its editor widget's events into these; the controller never
//! sees a UI toolkit.

use editor_model::model::{EditCommand, Scroll};

/// One thing the editor view asks of the controller.
#[derive(Debug, Clone, PartialEq)]
pub enum EditorMsg {
    /// An editing or motion command.
    Command(EditCommand),
    /// The view scrolled.
    Scrolled(Scroll),
    Copy,
    Cut,
    /// Selected text for the host's primary selection.
    PrimarySelection(String),
    /// Paste from the clipboard (`primary`: the middle-click selection).
    Paste {
        primary: bool,
    },
    /// The input-method preedit changed (empty: it ended).
    Preedit(String),
    /// The input method committed text.
    ImeCommit(String),
    /// The view gained or lost keyboard focus.
    Focus(bool),
    /// The geometry of the frame just drawn (feeds `ced.layout`).
    Layout(LayoutReport),
}

/// Where the editor drew things in its last frame, in points.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct LayoutReport {
    /// The editor: `[x, y, width, height]`.
    pub editor: [f32; 4],
    pub gutter_w: f32,
    pub line_height: f32,
    pub cell_w: f32,
    pub first_line: usize,
    pub visible_rows: usize,
    /// The caret: `[x, y, width, height]`.
    pub caret: [f32; 4],
}
