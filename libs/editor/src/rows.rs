// SPDX-License-Identifier: MIT OR Apache-2.0
//! Visual rows: where each line is cut into the rows the view draws.
//!
//! Without wrap a line is one row. With [`Wrap::Words`] a row breaks after
//! whitespace where it can and inside a word only when the word is longer
//! than a row; whitespace may hang past the right edge. Lines longer than
//! [`WORD_WRAP_MAX`] bytes break every `cols` cells instead, through the
//! long-line checkpoints, so a row deep inside a huge line is found without
//! storing every row start. Counting such a line's rows needs its total
//! width, measured once per text revision.
//!
//! A position on the screen is a [`Pos`]: a 1-based line and the 0-based row
//! within it.

use std::collections::HashMap;

use edit::text::Text;
use edit::view::{MeasureCfg, clusters};
use editor_model::model::{content_end, line_of};

use crate::Wrap;
use crate::lines::{Checkpoints, Version, boundary_at, cells_of};

/// Lines up to this many bytes break at words; longer ones every `cols`.
pub const WORD_WRAP_MAX: usize = 64 * 1024;

/// Lines whose rows are kept at once before the cache is dropped.
const MAX_CACHED_LINES: usize = 1024;

/// Documents up to this many bytes are wrapped whole (once per revision) so
/// the scrollbar counts visual rows; longer ones count lines.
pub const EXACT_TOTAL_MAX: usize = 256 * 1024;

/// A 1-based line and the 0-based row within it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct Pos {
    pub line: usize,
    pub row: usize,
}

/// Where a row begins: a byte offset and its absolute cell on the line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RowStart {
    pub offset: usize,
    pub cells: usize,
}

enum LineRows {
    Starts(Vec<RowStart>),
    /// A long line cut every `cols` cells; its total width in cells.
    Hard {
        start: usize,
        cells: usize,
    },
}

/// Row layout per line, valid for one text version, measure, wrap mode and
/// row width.
#[derive(Default)]
pub struct Rows {
    key: Option<(Version, MeasureCfg, Wrap, usize)>,
    wrap: Wrap,
    cols: usize,
    lines: HashMap<usize, LineRows>,
    prefix: Option<Vec<usize>>,
}

impl Rows {
    pub fn sync(&mut self, version: Version, cfg: &MeasureCfg, wrap: Wrap, cols: usize) {
        // Two cells hold the widest cluster but a tab, so every row start
        // advances by at least one cluster.
        let cols = cols.max(2);
        let key = Some((version, *cfg, wrap, cols));
        if self.key != key {
            self.key = key;
            self.wrap = wrap;
            self.cols = cols;
            self.lines.clear();
            self.prefix = None;
        }
    }

    pub fn wraps(&self) -> bool {
        self.wrap == Wrap::Words
    }

    fn line(
        &mut self,
        text: &Text,
        cfg: &MeasureCfg,
        ck: &mut Checkpoints,
        line: usize,
    ) -> &LineRows {
        if self.lines.len() > MAX_CACHED_LINES {
            self.lines.clear();
        }
        let cols = self.cols;
        let wrap = self.wrap;
        self.lines.entry(line).or_insert_with(|| {
            let Some(r) = text.line_range(line) else {
                return LineRows::Starts(vec![RowStart {
                    offset: text.len(),
                    cells: 0,
                }]);
            };
            let first = RowStart {
                offset: r.start,
                cells: 0,
            };
            if wrap == Wrap::None {
                return LineRows::Starts(vec![first]);
            }
            let end = content_end(text, line);
            if end - r.start > WORD_WRAP_MAX {
                let (_, cells) = cells_of(text, cfg, ck, end);
                return LineRows::Hard {
                    start: r.start,
                    cells,
                };
            }
            LineRows::Starts(word_rows(text, cfg, r.start..end, cols))
        })
    }

    /// How many rows `line` takes (at least one).
    pub fn count(
        &mut self,
        text: &Text,
        cfg: &MeasureCfg,
        ck: &mut Checkpoints,
        line: usize,
    ) -> usize {
        let cols = self.cols;
        match self.line(text, cfg, ck, line) {
            LineRows::Starts(starts) => starts.len(),
            LineRows::Hard { cells, .. } => cells.div_ceil(cols).max(1),
        }
    }

    /// Where row `row` of `line` begins (the last row when `row` is past it).
    pub fn start(
        &mut self,
        text: &Text,
        cfg: &MeasureCfg,
        ck: &mut Checkpoints,
        line: usize,
        row: usize,
    ) -> RowStart {
        let cols = self.cols;
        let count = self.count(text, cfg, ck, line);
        let row = row.min(count - 1);
        match self.line(text, cfg, ck, line) {
            LineRows::Starts(starts) => starts[row],
            &LineRows::Hard { start, .. } => {
                if row == 0 {
                    return RowStart {
                        offset: start,
                        cells: 0,
                    };
                }
                let (offset, cells) = boundary_at(text, cfg, ck, line, row * cols);
                RowStart { offset, cells }
            }
        }
    }

    /// The offset where row `row` of `line` ends: the next row's start, or
    /// the line's content end for its last row.
    pub fn end(
        &mut self,
        text: &Text,
        cfg: &MeasureCfg,
        ck: &mut Checkpoints,
        line: usize,
        row: usize,
    ) -> usize {
        if row + 1 < self.count(text, cfg, ck, line) {
            self.start(text, cfg, ck, line, row + 1).offset
        } else {
            content_end(text, line)
        }
    }

    /// The row holding `offset`. An offset where a row starts belongs to that
    /// row, so a caret at a soft break shows at the start of the next row.
    pub fn pos_of(
        &mut self,
        text: &Text,
        cfg: &MeasureCfg,
        ck: &mut Checkpoints,
        offset: usize,
    ) -> Pos {
        let line = line_of(text, offset);
        let cols = self.cols;
        let row = match self.line(text, cfg, ck, line) {
            LineRows::Starts(starts) => starts
                .partition_point(|s| s.offset <= offset)
                .saturating_sub(1),
            LineRows::Hard { .. } => {
                // The cell gives a guess; the real row starts settle it, so a
                // wide cluster pushed to the next row is found there.
                let count = self.count(text, cfg, ck, line);
                let (_, cells) = cells_of(text, cfg, ck, offset);
                let mut row = (cells / cols).min(count - 1);
                while row > 0 && self.start(text, cfg, ck, line, row).offset > offset {
                    row -= 1;
                }
                while row + 1 < count && self.start(text, cfg, ck, line, row + 1).offset <= offset {
                    row += 1;
                }
                row
            }
        };
        Pos { line, row }
    }

    /// `pos` moved by `delta` rows, clamped to the text.
    pub fn step(
        &mut self,
        text: &Text,
        cfg: &MeasureCfg,
        ck: &mut Checkpoints,
        pos: Pos,
        delta: isize,
    ) -> Pos {
        let line_count = text.line_count().max(1);
        let mut pos = Pos {
            line: pos.line.clamp(1, line_count),
            row: pos.row,
        };
        pos.row = pos.row.min(self.count(text, cfg, ck, pos.line) - 1);
        let mut left = delta.unsigned_abs();
        if delta >= 0 {
            while left > 0 {
                let rest = self.count(text, cfg, ck, pos.line) - 1 - pos.row;
                if left <= rest {
                    pos.row += left;
                    break;
                }
                if pos.line == line_count {
                    pos.row += rest;
                    break;
                }
                left -= rest + 1;
                pos = Pos {
                    line: pos.line + 1,
                    row: 0,
                };
            }
        } else {
            while left > 0 {
                if left <= pos.row {
                    pos.row -= left;
                    break;
                }
                if pos.line == 1 {
                    pos.row = 0;
                    break;
                }
                left -= pos.row + 1;
                pos.line -= 1;
                pos.row = self.count(text, cfg, ck, pos.line) - 1;
            }
        }
        pos
    }

    /// Rows from `a` down to `b` (0 when `b` is not below `a`). Positions
    /// past their line's last row, left over from an older layout, count as
    /// that last row.
    pub fn distance(
        &mut self,
        text: &Text,
        cfg: &MeasureCfg,
        ck: &mut Checkpoints,
        a: Pos,
        b: Pos,
    ) -> usize {
        let a = self.step(text, cfg, ck, a, 0);
        let b = self.step(text, cfg, ck, b, 0);
        if b <= a {
            return 0;
        }
        if a.line == b.line {
            return b.row - a.row;
        }
        let mut n = self.count(text, cfg, ck, a.line) - a.row;
        for line in a.line + 1..b.line {
            n += self.count(text, cfg, ck, line);
        }
        n + b.row
    }

    /// Rows before each line, `[0, rows(1), rows(1) + rows(2), …]`, when the
    /// document is small enough to wrap whole ([`EXACT_TOTAL_MAX`]); `None`
    /// otherwise, and the scrollbar then counts lines.
    fn prefix(&mut self, text: &Text, cfg: &MeasureCfg, ck: &mut Checkpoints) -> Option<&[usize]> {
        if self.wrap == Wrap::None || text.len() > EXACT_TOTAL_MAX {
            return None;
        }
        if self.prefix.is_none() {
            let lines = text.line_count().max(1);
            let mut sums = Vec::with_capacity(lines + 1);
            let mut n = 0;
            sums.push(0);
            for line in 1..=lines {
                n += self.count(text, cfg, ck, line);
                sums.push(n);
            }
            self.prefix = Some(sums);
        }
        self.prefix.as_deref()
    }

    /// The scrollbar's view of the document: total units and the index of
    /// `top`, in visual rows where [`Self::prefix`] is known, else in lines.
    pub fn extent(
        &mut self,
        text: &Text,
        cfg: &MeasureCfg,
        ck: &mut Checkpoints,
        top: Pos,
    ) -> (usize, usize) {
        let top = self.step(text, cfg, ck, top, 0);
        match self.prefix(text, cfg, ck) {
            Some(p) => (p[p.len() - 1], p[top.line - 1] + top.row),
            None => (text.line_count().max(1), top.line - 1),
        }
    }

    /// The position at scrollbar index `index` (the inverse of
    /// [`Self::extent`]).
    pub fn at_index(
        &mut self,
        text: &Text,
        cfg: &MeasureCfg,
        ck: &mut Checkpoints,
        index: usize,
    ) -> Pos {
        match self.prefix(text, cfg, ck) {
            Some(p) => {
                let line = p
                    .partition_point(|&rows| rows <= index)
                    .clamp(1, p.len() - 1);
                let pos = Pos {
                    line,
                    row: index - p[line - 1],
                };
                self.step(text, cfg, ck, pos, 0)
            }
            None => self.step(
                text,
                cfg,
                ck,
                Pos {
                    line: index.saturating_add(1),
                    row: 0,
                },
                0,
            ),
        }
    }
}

/// Greedy word wrap of one line's content into rows of `cols` cells.
fn word_rows(
    text: &Text,
    cfg: &MeasureCfg,
    content: std::ops::Range<usize>,
    cols: usize,
) -> Vec<RowStart> {
    let mut rows = vec![RowStart {
        offset: content.start,
        cells: 0,
    }];
    let mut row_cells = 0;
    // The last place a row may break: just after whitespace.
    let mut last_break: Option<RowStart> = None;
    let mut cell = 0;
    for c in clusters(text, cfg, content.clone(), 0) {
        if c.range.start >= content.end {
            break;
        }
        let next = cell + usize::from(c.cells);
        let space = c.is_tab || (c.ascii && is_space(text, c.range.start));
        if !space && next - row_cells > cols && cell > row_cells {
            let at = match last_break {
                Some(b) if b.cells > row_cells => b,
                _ => RowStart {
                    offset: c.range.start,
                    cells: cell,
                },
            };
            rows.push(at);
            row_cells = at.cells;
            last_break = None;
            // A word still longer than a row breaks again at this cluster.
            if next - row_cells > cols && cell > row_cells {
                let at = RowStart {
                    offset: c.range.start,
                    cells: cell,
                };
                rows.push(at);
                row_cells = cell;
            }
        }
        if space {
            last_break = Some(RowStart {
                offset: c.range.end,
                cells: next,
            });
        }
        cell = next;
    }
    rows
}

fn is_space(text: &Text, offset: usize) -> bool {
    text.chunk_at(offset).first() == Some(&b' ')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> MeasureCfg {
        MeasureCfg::default()
    }

    fn rows_of(s: &str, cols: usize) -> Vec<String> {
        let text = Text::from_text(s).unwrap();
        let mut rows = Rows::default();
        let mut ck = Checkpoints::default();
        rows.sync((1, 0, text.len()), &cfg(), Wrap::Words, cols);
        let mut out = Vec::new();
        for line in 1..=text.line_count() {
            for row in 0..rows.count(&text, &cfg(), &mut ck, line) {
                let a = rows.start(&text, &cfg(), &mut ck, line, row).offset;
                let b = rows.end(&text, &cfg(), &mut ck, line, row);
                let mut s = String::new();
                text.read(a..b, &mut s);
                out.push(s);
            }
        }
        out
    }

    #[test]
    fn words_break_after_whitespace_and_spaces_hang() {
        assert_eq!(
            rows_of("hello world again", 8),
            ["hello ", "world ", "again"]
        );
        assert_eq!(rows_of("one two", 20), ["one two"]);
        assert_eq!(
            rows_of("a b\nlonger line here", 6),
            ["a b", "longer ", "line ", "here"]
        );
    }

    #[test]
    fn a_word_longer_than_a_row_breaks_inside() {
        assert_eq!(rows_of("abcdefghij", 4), ["abcd", "efgh", "ij"]);
        assert_eq!(rows_of("go abcdefghij", 4), ["go ", "abcd", "efgh", "ij"]);
    }

    #[test]
    fn wide_clusters_move_whole_to_the_next_row() {
        assert_eq!(rows_of("中文中文中", 4), ["中文", "中文", "中"]);
        assert_eq!(rows_of("abc中", 4), ["abc", "中"]);
    }

    #[test]
    fn without_wrap_a_line_is_one_row() {
        let text = Text::from_text(&"word ".repeat(100)).unwrap();
        let mut rows = Rows::default();
        let mut ck = Checkpoints::default();
        rows.sync((1, 0, text.len()), &cfg(), Wrap::None, 10);
        assert_eq!(rows.count(&text, &cfg(), &mut ck, 1), 1);
        assert_eq!(
            rows.pos_of(&text, &cfg(), &mut ck, 400),
            Pos { line: 1, row: 0 }
        );
    }

    #[test]
    fn huge_lines_cut_every_row_width() {
        let len = WORD_WRAP_MAX * 4;
        let text = Text::from_text(&"x".repeat(len)).unwrap();
        let mut rows = Rows::default();
        let mut ck = Checkpoints::default();
        rows.sync((1, 0, text.len()), &cfg(), Wrap::Words, 100);
        ck.sync((1, 0, text.len()), &cfg());
        assert_eq!(rows.count(&text, &cfg(), &mut ck, 1), len / 100 + 1);
        let s = rows.start(&text, &cfg(), &mut ck, 1, 1234);
        assert_eq!((s.offset, s.cells), (123_400, 123_400));
        assert_eq!(
            rows.pos_of(&text, &cfg(), &mut ck, 123_456),
            Pos { line: 1, row: 1234 }
        );
        assert_eq!(
            rows.pos_of(&text, &cfg(), &mut ck, 123_400),
            Pos { line: 1, row: 1234 }
        );
    }

    #[test]
    fn huge_line_affinity_follows_the_real_row_starts() {
        let mut rows = Rows::default();
        let mut ck = Checkpoints::default();
        let s = format!("abc中{}", "x".repeat(WORD_WRAP_MAX));
        let text = Text::from_text(&s).unwrap();
        rows.sync((1, 0, text.len()), &cfg(), Wrap::Words, 4);
        assert_eq!(
            rows.start(&text, &cfg(), &mut ck, 1, 1).offset,
            3,
            "中 starts row 1"
        );
        assert_eq!(
            rows.pos_of(&text, &cfg(), &mut ck, 3),
            Pos { line: 1, row: 1 }
        );
        let text = Text::from_text(&"x".repeat(WORD_WRAP_MAX + 4)).unwrap();
        let mut rows = Rows::default();
        rows.sync((2, 0, text.len()), &cfg(), Wrap::Words, 4);
        let count = rows.count(&text, &cfg(), &mut ck, 1);
        assert_eq!(count, (WORD_WRAP_MAX + 4) / 4);
        let end = rows.pos_of(&text, &cfg(), &mut ck, text.len());
        assert_eq!(
            end,
            Pos {
                line: 1,
                row: count - 1
            },
            "the end is on the last row"
        );
    }

    #[test]
    fn stale_positions_measure_as_their_last_row() {
        let text = Text::from_text("short\nline two").unwrap();
        let mut rows = Rows::default();
        let mut ck = Checkpoints::default();
        rows.sync((1, 0, text.len()), &cfg(), Wrap::Words, 40);
        let stale = Pos { line: 1, row: 10 };
        assert_eq!(
            rows.distance(&text, &cfg(), &mut ck, stale, Pos { line: 2, row: 0 }),
            1
        );
        assert_eq!(
            rows.distance(&text, &cfg(), &mut ck, Pos { line: 1, row: 0 }, stale),
            0
        );
    }

    #[test]
    fn the_scroll_extent_counts_visual_rows_in_small_documents() {
        let text = Text::from_text(&format!("{}\nend", "word ".repeat(40))).unwrap();
        let mut rows = Rows::default();
        let mut ck = Checkpoints::default();
        rows.sync((1, 0, text.len()), &cfg(), Wrap::Words, 20);
        let first = rows.count(&text, &cfg(), &mut ck, 1);
        assert_eq!(first, 10);
        let (total, at) = rows.extent(&text, &cfg(), &mut ck, Pos { line: 1, row: 3 });
        assert_eq!((total, at), (11, 3));
        assert_eq!(
            rows.at_index(&text, &cfg(), &mut ck, 3),
            Pos { line: 1, row: 3 }
        );
        assert_eq!(
            rows.at_index(&text, &cfg(), &mut ck, 10),
            Pos { line: 2, row: 0 }
        );
        assert_eq!(
            rows.at_index(&text, &cfg(), &mut ck, 99),
            Pos { line: 2, row: 0 }
        );
        rows.sync((1, 0, text.len()), &cfg(), Wrap::None, 20);
        assert_eq!(
            rows.extent(&text, &cfg(), &mut ck, Pos { line: 2, row: 0 }),
            (2, 1),
            "lines without wrap"
        );
    }

    #[test]
    fn caret_positions_and_stepping() {
        let text = Text::from_text("hello world again\nx\nmore words").unwrap();
        let mut rows = Rows::default();
        let mut ck = Checkpoints::default();
        rows.sync((1, 0, text.len()), &cfg(), Wrap::Words, 8);
        let p = |line, row| Pos { line, row };
        assert_eq!(rows.pos_of(&text, &cfg(), &mut ck, 3), p(1, 0));
        assert_eq!(
            rows.pos_of(&text, &cfg(), &mut ck, 6),
            p(1, 1),
            "a soft break starts the next row"
        );
        assert_eq!(rows.step(&text, &cfg(), &mut ck, p(1, 0), 3), p(2, 0));
        assert_eq!(rows.step(&text, &cfg(), &mut ck, p(2, 0), -1), p(1, 2));
        assert_eq!(rows.step(&text, &cfg(), &mut ck, p(1, 1), -5), p(1, 0));
        assert_eq!(rows.step(&text, &cfg(), &mut ck, p(3, 0), 9), p(3, 1));
        assert_eq!(rows.distance(&text, &cfg(), &mut ck, p(1, 1), p(3, 1)), 4);
    }
}
