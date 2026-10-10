// SPDX-License-Identifier: MIT OR Apache-2.0
//! Line measurement for drawing and hit-testing: walks
//! [`edit::view::clusters`] over the part of one line that is needed. Long
//! lines are seeked through checkpoints that grow only to the requested
//! prefix and are kept per text revision, so a repeat seek walks at most one
//! checkpoint interval instead of the whole line.

use std::collections::HashMap;
use std::ops::Range;

use edit::text::Text;
use edit::view::{Cluster, MeasureCfg, clusters};
use editor_model::model::{content_end, line_of};

/// Lines longer than this (bytes) seek through checkpoints.
pub const LONG_LINE: usize = 4096;

/// Lines indexed at once before the index is dropped.
const MAX_INDEXED_LINES: usize = 64;

/// Which identity a set of checkpoints is valid for.
pub type Version = (u64, u64, usize);

/// Checkpoints per line start, valid for one text version and measure.
#[derive(Default)]
pub struct Checkpoints {
    key: Option<(Version, MeasureCfg)>,
    lines: HashMap<usize, LineIndex>,
}

#[derive(Clone, Copy)]
enum Seek {
    Offset(usize),
    Cells(usize),
}

impl Seek {
    fn beyond(self, &(offset, cells): &(usize, usize)) -> bool {
        match self {
            Self::Offset(target) => target > offset,
            Self::Cells(target) => target > cells,
        }
    }

    fn contains(self, &(offset, cells): &(usize, usize)) -> bool {
        match self {
            Self::Offset(target) => offset <= target,
            Self::Cells(target) => cells <= target,
        }
    }
}

struct LineIndex {
    points: Vec<(usize, usize)>,
    scanned: (usize, usize),
    complete: bool,
}

impl LineIndex {
    fn new(start: usize) -> Self {
        Self {
            points: vec![(start, 0)],
            scanned: (start, 0),
            complete: false,
        }
    }

    fn seek(&mut self, text: &Text, cfg: &MeasureCfg, end: usize, target: Seek) -> (usize, usize) {
        // Expand only to the requested prefix: viewing the start of a huge
        // line never indexes its unseen tail.
        if !self.complete && target.beyond(&self.scanned) {
            let mut cursor = self.scanned;
            for cluster in clusters(text, cfg, cursor.0..end, cursor.1) {
                cursor = (cluster.range.end, cursor.1 + usize::from(cluster.cells));
                let last = self.points.last().map_or(0, |p| p.0);
                if cursor.0.saturating_sub(last) >= LONG_LINE {
                    self.points.push(cursor);
                }
                if !target.beyond(&cursor) {
                    break;
                }
            }
            self.scanned = cursor;
            self.complete = cursor.0 >= end;
        }
        if target.contains(&self.scanned) {
            return self.scanned;
        }
        let index = self.points.partition_point(|point| target.contains(point));
        self.points[index.saturating_sub(1)]
    }
}

impl Checkpoints {
    /// Identity, revision, length and measurement each invalidate the
    /// checkpoints, including an equal-length replacement.
    pub fn sync(&mut self, version: Version, cfg: &MeasureCfg) {
        let key = Some((version, *cfg));
        if self.key != key {
            self.key = key;
            self.lines.clear();
        }
    }

    /// The best `(offset, cells)` start at or before the target on `line`
    /// (the line start for short lines).
    fn start(
        &mut self,
        text: &Text,
        cfg: &MeasureCfg,
        line: usize,
        target: Seek,
    ) -> (usize, usize) {
        let Some(r) = text.line_range(line) else {
            return (text.len(), 0);
        };
        if r.len() <= LONG_LINE {
            return (r.start, 0);
        }
        if self.lines.len() > MAX_INDEXED_LINES {
            self.lines.clear();
        }
        let index = self
            .lines
            .entry(r.start)
            .or_insert_with(|| LineIndex::new(r.start));
        index.seek(text, cfg, content_end(text, line), target)
    }
}

/// One cluster placed on the grid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placed {
    pub range: Range<usize>,
    /// Starting cell, absolute from the line start.
    pub cell: usize,
    pub cells: u8,
    pub is_tab: bool,
    pub ascii: bool,
}

/// The clusters of one line that intersect a cell window.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LineCells {
    /// Line content range (no `\r\n` or `\n`).
    pub content: Range<usize>,
    pub placed: Vec<Placed>,
    /// Cells at the content end, when the walk reached it.
    pub end_cells: Option<usize>,
}

impl LineCells {
    /// Absolute cell of `offset` on this line, clipped to the walked window:
    /// before the window gives its left edge, beyond it `x1`.
    pub fn cell_of(&self, offset: usize, x1: usize) -> usize {
        if offset >= self.content.end {
            return self.end_cells.unwrap_or(x1);
        }
        // The first placed cluster ending after `offset` contains it or, when
        // `offset` lies left of the window, starts the window.
        self.placed
            .get(self.placed.partition_point(|p| p.range.end <= offset))
            .map_or(self.end_cells.unwrap_or(x1), |p| p.cell)
    }
}

/// Walk `line`'s clusters covering cells `[x0, x1)`.
pub fn walk(
    text: &Text,
    cfg: &MeasureCfg,
    ck: &mut Checkpoints,
    line: usize,
    x0: usize,
    x1: usize,
) -> LineCells {
    let Some(r) = text.line_range(line) else {
        return LineCells::default();
    };
    let end = content_end(text, line);
    let (from, from_cells) = ck.start(text, cfg, line, Seek::Cells(x0));
    walk_from(text, cfg, r.start..end, (from, from_cells), x0, x1)
}

/// Walk clusters of the line content `content` from the known boundary
/// `from`, keeping those covering cells `[x0, x1)`.
pub(crate) fn walk_from(
    text: &Text,
    cfg: &MeasureCfg,
    content: Range<usize>,
    (from, from_cells): (usize, usize),
    x0: usize,
    x1: usize,
) -> LineCells {
    let end = content.end;
    let mut out = LineCells {
        content,
        placed: Vec::new(),
        end_cells: None,
    };
    if from >= end {
        out.end_cells = Some(from_cells);
        return out;
    }
    let mut cell = from_cells;
    let mut reached_end = true;
    for Cluster {
        range,
        cells,
        is_tab,
        ascii,
    } in clusters(text, cfg, from..end, from_cells)
    {
        if cell >= x1 {
            reached_end = false;
            break;
        }
        if range.start >= end {
            break;
        }
        let next = cell + usize::from(cells);
        if next > x0 || (cells == 0 && cell >= x0) {
            out.placed.push(Placed {
                range,
                cell,
                cells,
                is_tab,
                ascii,
            });
        }
        cell = next;
    }
    if reached_end {
        out.end_cells = Some(cell);
    }
    out
}

/// The `(line, absolute cell)` of `offset`, exact at any line length.
pub fn cells_of(
    text: &Text,
    cfg: &MeasureCfg,
    ck: &mut Checkpoints,
    offset: usize,
) -> (usize, usize) {
    let line = line_of(text, offset);
    let end = content_end(text, line);
    let offset = offset.min(end);
    let (from, from_cells) = ck.start(text, cfg, line, Seek::Offset(offset));
    if from == offset {
        return (line, from_cells);
    }
    let mut cell = from_cells;
    for c in clusters(text, cfg, from..end, from_cells) {
        if c.range.end > offset {
            break;
        }
        cell += usize::from(c.cells);
    }
    (line, cell)
}

/// The boundary at or before absolute cell `cells` on `line`, with its cell:
/// where a soft-wrapped row of a long line starts.
pub fn boundary_at(
    text: &Text,
    cfg: &MeasureCfg,
    ck: &mut Checkpoints,
    line: usize,
    cells: usize,
) -> (usize, usize) {
    let end = content_end(text, line);
    let (from, from_cells) = ck.start(text, cfg, line, Seek::Cells(cells));
    let mut at = (from, from_cells);
    for c in clusters(text, cfg, from..end, from_cells) {
        let next = at.1 + usize::from(c.cells);
        if next > cells {
            break;
        }
        at = (c.range.end, next);
    }
    at
}

/// The offset nearest fractional cell `target` on `line`, walking from the
/// known boundary `from` and stopping at `stop` (mouse hits): the boundary
/// before a cluster when the target is in its first half.
pub(crate) fn offset_from(
    text: &Text,
    cfg: &MeasureCfg,
    (from, from_cells): (usize, usize),
    stop: usize,
    target: f32,
) -> usize {
    let mut cell = from_cells as f32;
    for c in clusters(text, cfg, from..stop, from_cells) {
        if c.range.start >= stop {
            break;
        }
        let w = f32::from(c.cells);
        if target < cell + w / 2.0 {
            return c.range.start;
        }
        cell += w;
    }
    stop
}

/// The offset nearest fractional cell `target` on `line`.
pub fn offset_at(
    text: &Text,
    cfg: &MeasureCfg,
    ck: &mut Checkpoints,
    line: usize,
    target: f32,
) -> usize {
    let line = line.clamp(1, text.line_count().max(1));
    let end = content_end(text, line);
    let t = target.max(0.0) as usize;
    let start = ck.start(text, cfg, line, Seek::Cells(t));
    offset_from(text, cfg, start, end, target)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> MeasureCfg {
        MeasureCfg {
            tab_size: 4,
            ambiguous_wide: false,
        }
    }

    fn text(s: &str) -> Text {
        Text::from_text(s).unwrap()
    }

    #[test]
    fn tabs_and_wide_characters_on_the_grid() {
        let text = text("a\tb中c\r\nnext");
        let mut ck = Checkpoints::default();
        let lc = walk(&text, &cfg(), &mut ck, 1, 0, 80);
        let cells: Vec<(usize, u8)> = lc.placed.iter().map(|p| (p.cell, p.cells)).collect();
        assert_eq!(cells, [(0, 1), (1, 3), (4, 1), (5, 2), (7, 1)]);
        assert_eq!(lc.end_cells, Some(8), "the CR is not part of the content");
        assert_eq!(cells_of(&text, &cfg(), &mut ck, 3), (1, 5));
        assert_eq!(cells_of(&text, &cfg(), &mut ck, 6), (1, 7));
        assert_eq!(
            cells_of(&text, &cfg(), &mut ck, 8),
            (1, 8),
            "inside the CRLF clamps"
        );
        for (o, _) in "a\tb中c".char_indices() {
            let (_, c) = cells_of(&text, &cfg(), &mut ck, o);
            assert_eq!(
                offset_at(&text, &cfg(), &mut ck, 1, c as f32),
                o,
                "offset {o} at {c}"
            );
        }
        assert_eq!(
            offset_at(&text, &cfg(), &mut ck, 1, 6.2),
            6,
            "past the middle of 中"
        );
        assert_eq!(offset_at(&text, &cfg(), &mut ck, 1, 5.9), 3);
        assert_eq!(
            offset_at(&text, &cfg(), &mut ck, 1, 999.0),
            7,
            "clamped before the CRLF"
        );
    }

    #[test]
    fn a_window_clips_and_reports_offscreen_offsets_at_its_edges() {
        let text = text(&"0123456789".repeat(3));
        let mut ck = Checkpoints::default();
        let lc = walk(&text, &cfg(), &mut ck, 1, 10, 20);
        assert_eq!(lc.placed.first().map(|p| p.cell), Some(10));
        assert_eq!(lc.placed.len(), 10);
        assert_eq!(lc.end_cells, None);
        assert_eq!(lc.cell_of(15, 20), 15);
        assert_eq!(lc.cell_of(25, 20), 20);
        assert_eq!(
            lc.cell_of(2, 20),
            10,
            "left of the window gives its left edge"
        );
    }

    #[test]
    fn long_lines_seek_through_checkpoints() {
        let len = 5 * 1024 * 1024;
        let text = text(&format!("{}\nend", "x".repeat(len)));
        let mut ck = Checkpoints::default();
        ck.sync((1, 0, text.len()), &cfg());
        let far = len - 3;
        assert_eq!(cells_of(&text, &cfg(), &mut ck, far), (1, far));
        let lc = walk(&text, &cfg(), &mut ck, 1, far, far + 100);
        assert_eq!(lc.placed.first().map(|p| p.range.start), Some(far));
        assert_eq!(lc.end_cells, Some(len));
        assert_eq!(
            offset_at(&text, &cfg(), &mut ck, 1, (far + 1) as f32),
            far + 1
        );
        assert_eq!(boundary_at(&text, &cfg(), &mut ck, 1, far), (far, far));
    }

    #[test]
    fn boundaries_never_split_a_wide_cluster() {
        let text = text("ab中cd");
        let mut ck = Checkpoints::default();
        assert_eq!(
            boundary_at(&text, &cfg(), &mut ck, 1, 3),
            (2, 2),
            "中 spans cells 2-3"
        );
        assert_eq!(boundary_at(&text, &cfg(), &mut ck, 1, 4), (5, 4));
    }

    #[test]
    fn unicode_and_tabs_measure_like_a_full_walk() {
        let body = format!("{}\r\n", "a\t中e\u{301}👍🏽".repeat(600));
        let text = text(&body);
        let mut ck = Checkpoints::default();
        for offset in [0, 5, 1000, 7999, 4011, 9, 100, body.len() - 2] {
            let offset = editor_model::model::clamp_offset(&text, offset);
            let actual = cells_of(&text, &cfg(), &mut ck, offset);
            let expected: usize = clusters(&text, &cfg(), 0..content_end(&text, 1), 0)
                .take_while(|c| c.range.end <= offset)
                .map(|c| usize::from(c.cells))
                .sum();
            assert_eq!(actual, (1, expected));
        }
    }
}
