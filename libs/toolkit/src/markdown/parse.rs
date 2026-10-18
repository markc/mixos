// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! pulldown-cmark's event stream folded into a [`Document`].
//!
//! Each container (quote, list item, table cell) is read by a recursive
//! call that returns at its own end event; nesting is well formed, so any
//! end a call meets that it did not open is its own. A tight list item's
//! text arrives with no paragraph round it and is read as a run that stops
//! at the first block event, without consuming it.
//!
//! Three limits keep any input's document small and its recursion shallow:
//! quotes and list items nest at most [`MAX_DEPTH`] deep (deeper text is
//! kept as plain text), and the document holds at most [`MAX_NODES`] blocks
//! and spans and [`MAX_BYTES`] bytes; past either budget the rest of the
//! source is one plain-text paragraph. A link's address is one shared
//! `Arc<str>` for all its spans.

use std::iter::Peekable;
use std::sync::Arc;

use pulldown_cmark::{Alignment, CodeBlockKind, Event, Options, Parser, Tag, TagEnd};

use super::{Align, Block, Document, Inline, Item, Link, MAX_BYTES, MAX_DEPTH, MAX_NODES, Marks};

pub fn parse(source: &str) -> Document {
    let options =
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let mut reader = Reader {
        events: Parser::new_ext(source, options).peekable(),
        tasks: Vec::new(),
        depth: 0,
        nodes: 0,
        bytes: 0,
        rest: None,
    };
    let mut blocks = reader.blocks();
    if let Some(rest) = reader.rest.take()
        && !rest.trim().is_empty()
    {
        blocks.push(Block::Paragraph(plain(rest.trim())));
    }
    Document { blocks }
}

/// `text` as one unmarked span.
fn plain(text: &str) -> Inline {
    let mut inline = Inline::default();
    inline.push(text, Marks::default(), None);
    inline
}

/// End a line of flattened text, once.
fn newline(text: &mut String) {
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
}

struct Reader<'a> {
    events: Peekable<Parser<'a>>,
    /// The task box of each list item being read, innermost last: a task
    /// marker belongs to the innermost item.
    tasks: Vec<Option<bool>>,
    /// How many quotes and list items enclose the blocks being read.
    depth: usize,
    /// Blocks and spans made so far.
    nodes: usize,
    /// Bytes of text and link addresses held so far.
    bytes: usize,
    /// Once a budget is spent: the rest of the source, as plain text. Every
    /// event has been read by then, so every loop ends.
    rest: Option<String>,
}

/// An inline element still open.
enum Open {
    Emphasis,
    Strong,
    Strike,
    Link(Link),
}

/// How an inline run ends.
#[derive(Clone, Copy, PartialEq)]
enum Until {
    /// At its container's end event, which it consumes.
    End,
    /// Before the first block event, which it leaves (a tight list item).
    Block,
}

fn inline_start(tag: &Tag<'_>) -> bool {
    matches!(
        tag,
        Tag::Emphasis | Tag::Strong | Tag::Strikethrough | Tag::Link { .. } | Tag::Image { .. }
    )
}

fn inline_end(tag: &TagEnd) -> bool {
    matches!(
        tag,
        TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough | TagEnd::Link | TagEnd::Image
    )
}

/// Whether `event` continues an inline run (`open` inline elements are
/// still waiting for their ends).
fn is_inline(event: &Event<'_>, open: bool) -> bool {
    match event {
        Event::Text(_)
        | Event::Code(_)
        | Event::InlineMath(_)
        | Event::InlineHtml(_)
        | Event::FootnoteReference(_)
        | Event::SoftBreak
        | Event::HardBreak
        | Event::TaskListMarker(_) => true,
        Event::Start(tag) => inline_start(tag),
        Event::End(tag) => open && inline_end(tag),
        _ => false,
    }
}

impl Reader<'_> {
    /// Take `nodes` blocks or spans and `bytes` bytes from the budgets;
    /// false (and nothing taken) once either would be spent.
    fn admit(&mut self, nodes: usize, bytes: usize) -> bool {
        if self.rest.is_some() {
            return false;
        }
        if self.nodes + nodes > MAX_NODES || self.bytes + bytes > MAX_BYTES {
            return false;
        }
        self.nodes += nodes;
        self.bytes += bytes;
        true
    }

    /// A budget is spent: `first` and every event left become the plain
    /// text shown after the document's last block.
    fn overflow(&mut self, first: &str) {
        let mut text = first.to_owned();
        self.flatten(&mut text, false);
        match self.rest.as_mut() {
            Some(rest) => rest.push_str(&text),
            None => self.rest = Some(text),
        }
    }

    /// Append the text of the events that follow to `text`, without marks:
    /// to the end of the container just opened (`container`), or of the
    /// input. A loop with a counter, never recursion, however deep the
    /// input nests.
    fn flatten(&mut self, text: &mut String, container: bool) {
        let mut open = 1usize;
        for event in self.events.by_ref() {
            match event {
                Event::Start(tag) => {
                    if !inline_start(&tag) {
                        newline(text);
                    }
                    open += 1;
                }
                Event::End(tag) => {
                    if !inline_end(&tag) {
                        newline(text);
                    }
                    open = open.saturating_sub(1);
                    if container && open == 0 {
                        break;
                    }
                }
                Event::Text(t)
                | Event::Code(t)
                | Event::InlineMath(t)
                | Event::DisplayMath(t)
                | Event::Html(t)
                | Event::InlineHtml(t) => text.push_str(&t),
                Event::SoftBreak | Event::HardBreak | Event::Rule => text.push('\n'),
                Event::TaskListMarker(done) => text.push_str(if done { "[x] " } else { "[ ] " }),
                Event::FootnoteReference(name) => {
                    text.push_str("[^");
                    text.push_str(&name);
                    text.push(']');
                }
            }
        }
    }

    /// The container just opened, past [`MAX_DEPTH`]: its text as one
    /// plain paragraph.
    fn too_deep(&mut self) -> Option<Block> {
        let mut text = String::new();
        self.flatten(&mut text, true);
        let text = text.trim();
        if text.is_empty() {
            return None;
        }
        if !self.admit(1, text.len()) {
            self.overflow(text);
            return None;
        }
        Some(Block::Paragraph(plain(text)))
    }

    /// Blocks up to the end of the container being read (or the input's).
    fn blocks(&mut self) -> Vec<Block> {
        let mut out = Vec::new();
        while let Some(event) = self.events.peek() {
            if is_inline(event, false) {
                let text = self.inline(Until::Block);
                if !text.is_empty() {
                    out.push(Block::Paragraph(text));
                }
                continue;
            }
            let Some(event) = self.events.next() else {
                break;
            };
            if matches!(
                event,
                Event::Start(_) | Event::Rule | Event::Html(_) | Event::DisplayMath(_)
            ) && !self.admit(1, 0)
            {
                // The block that would have been made is in the plain text.
                self.overflow("");
                break;
            }
            match event {
                Event::End(_) => break,
                Event::Start(tag) => match tag {
                    Tag::Paragraph => out.push(Block::Paragraph(self.inline(Until::End))),
                    Tag::Heading { level, .. } => out.push(Block::Heading {
                        level: level as u8,
                        text: self.inline(Until::End),
                    }),
                    Tag::BlockQuote(_) if self.depth >= MAX_DEPTH => out.extend(self.too_deep()),
                    Tag::BlockQuote(_) => {
                        self.depth += 1;
                        let quote = self.blocks();
                        self.depth -= 1;
                        out.push(Block::Quote(quote));
                    }
                    Tag::CodeBlock(kind) => {
                        let info = match kind {
                            CodeBlockKind::Fenced(info) => info.trim().to_owned(),
                            CodeBlockKind::Indented => String::new(),
                        };
                        let mut text = self.literal();
                        if text.ends_with('\n') {
                            text.pop();
                        }
                        if !self.admit(0, text.len()) {
                            self.overflow(&text);
                            break;
                        }
                        out.push(Block::Code { info, text });
                    }
                    Tag::HtmlBlock => {
                        let mut text = self.literal();
                        if text.ends_with('\n') {
                            text.pop();
                        }
                        if !self.admit(0, text.len()) {
                            self.overflow(&text);
                            break;
                        }
                        out.push(Block::Html(text));
                    }
                    Tag::List(_) if self.depth >= MAX_DEPTH => out.extend(self.too_deep()),
                    Tag::List(start) => out.push(Block::List {
                        start,
                        items: self.items(),
                    }),
                    Tag::Table(align) => out.push(self.table(&align)),
                    // Footnotes, definition lists and metadata are not
                    // enabled; anything else keeps its children.
                    _ if self.depth >= MAX_DEPTH => out.extend(self.too_deep()),
                    _ => {
                        self.depth += 1;
                        let inner = self.blocks();
                        self.depth -= 1;
                        out.extend(inner);
                    }
                },
                Event::Rule => out.push(Block::Rule),
                Event::Html(text) | Event::DisplayMath(text) => {
                    let text = text.trim_end();
                    if !self.admit(0, text.len()) {
                        self.overflow(text);
                        break;
                    }
                    out.push(Block::Html(text.to_owned()));
                }
                _ => {}
            }
        }
        out
    }

    /// The text up to the current block's end, verbatim.
    fn literal(&mut self) -> String {
        let mut text = String::new();
        for event in self.events.by_ref() {
            match event {
                Event::End(_) => break,
                Event::Text(t) | Event::Html(t) | Event::InlineHtml(t) | Event::Code(t) => {
                    text.push_str(&t);
                }
                Event::SoftBreak | Event::HardBreak => text.push('\n'),
                _ => {}
            }
        }
        text
    }

    /// One inline run.
    fn inline(&mut self, until: Until) -> Inline {
        let mut out = Inline::default();
        let mut open: Vec<Open> = Vec::new();
        // The marks and link in force, kept as `open` changes so each
        // piece of text costs no scan of `open`.
        let mut on = Marks::default();
        let (mut strong, mut emphasis, mut strike) = (0usize, 0usize, 0usize);
        let mut link: Option<Link> = None;
        while let Some(event) = self.events.peek() {
            if until == Until::Block && !is_inline(event, !open.is_empty()) {
                break;
            }
            let Some(event) = self.events.next() else {
                break;
            };
            let (text, code, linked): (&str, bool, bool) = match &event {
                Event::Text(text) => (&**text, false, true),
                Event::Code(text) | Event::InlineMath(text) => (&**text, true, true),
                // Never interpreted: the source, as text.
                Event::InlineHtml(text) | Event::Html(text) => (&**text, false, true),
                Event::SoftBreak => (" ", false, true),
                Event::HardBreak => ("\n", false, false),
                _ => ("", false, false),
            };
            if !text.is_empty() {
                let marks = Marks { code, ..on };
                let link = if linked { link.as_ref() } else { None };
                if !self.add(&mut out, text, marks, link) {
                    break;
                }
                continue;
            }
            match event {
                Event::FootnoteReference(name) => {
                    let text = format!("[^{name}]");
                    if !self.add(&mut out, &text, on, None) {
                        break;
                    }
                }
                Event::TaskListMarker(done) => {
                    if let Some(task) = self.tasks.last_mut() {
                        *task = Some(done);
                    }
                }
                Event::Start(Tag::Emphasis) => {
                    emphasis += 1;
                    open.push(Open::Emphasis);
                }
                Event::Start(Tag::Strong) => {
                    strong += 1;
                    open.push(Open::Strong);
                }
                Event::Start(Tag::Strikethrough) => {
                    strike += 1;
                    open.push(Open::Strike);
                }
                Event::Start(Tag::Link { dest_url, .. } | Tag::Image { dest_url, .. })
                    if !self.admit(0, dest_url.len()) =>
                {
                    self.overflow("");
                    break;
                }
                Event::Start(Tag::Link { dest_url, .. }) => {
                    let opened = Link {
                        url: Arc::from(&*dest_url),
                        image: false,
                    };
                    link = Some(opened.clone());
                    open.push(Open::Link(opened));
                }
                Event::Start(Tag::Image { dest_url, .. }) => {
                    let opened = Link {
                        url: Arc::from(&*dest_url),
                        image: true,
                    };
                    link = Some(opened.clone());
                    open.push(Open::Link(opened));
                }
                Event::End(tag) if inline_end(&tag) && !open.is_empty() => match open.pop() {
                    Some(Open::Emphasis) => emphasis -= 1,
                    Some(Open::Strong) => strong -= 1,
                    Some(Open::Strike) => strike -= 1,
                    Some(Open::Link(_)) => {
                        link = open.iter().rev().find_map(|o| match o {
                            Open::Link(link) => Some(link.clone()),
                            _ => None,
                        });
                    }
                    None => {}
                },
                // The container's own end.
                Event::End(_) => break,
                _ => {}
            }
            on = Marks {
                strong: strong > 0,
                emphasis: emphasis > 0,
                strike: strike > 0,
                code: false,
            };
        }
        out
    }

    /// Append `text` to `out` within the budgets; false once they are
    /// spent (`text` and the rest of the source are then the plain text).
    fn add(&mut self, out: &mut Inline, text: &str, marks: Marks, link: Option<&Link>) -> bool {
        let joins = out
            .spans
            .last()
            .is_some_and(|last| last.marks == marks && last.link.as_ref() == link);
        if !self.admit(usize::from(!joins), text.len()) {
            self.overflow(text);
            return false;
        }
        out.push(text, marks, link);
        true
    }

    /// A list's items, up to the list's end. Each item is one level of
    /// [`MAX_DEPTH`].
    fn items(&mut self) -> Vec<Item> {
        let mut items = Vec::new();
        while let Some(event) = self.events.next() {
            match event {
                Event::Start(Tag::Item) => {
                    if !self.admit(1, 0) {
                        self.overflow("");
                        break;
                    }
                    self.tasks.push(None);
                    self.depth += 1;
                    let blocks = self.blocks();
                    self.depth -= 1;
                    let task = self.tasks.pop().flatten();
                    items.push(Item { task, blocks });
                }
                Event::End(_) => break,
                _ => {}
            }
        }
        items
    }

    fn table(&mut self, align: &[Alignment]) -> Block {
        let align = align
            .iter()
            .map(|a| match a {
                Alignment::None => Align::None,
                Alignment::Left => Align::Left,
                Alignment::Center => Align::Center,
                Alignment::Right => Align::Right,
            })
            .collect();
        let mut head = Vec::new();
        let mut rows = Vec::new();
        while let Some(event) = self.events.next() {
            match event {
                Event::Start(Tag::TableHead) => head = self.cells(),
                Event::Start(Tag::TableRow) => rows.push(self.cells()),
                Event::End(_) => break,
                _ => {}
            }
        }
        Block::Table { align, head, rows }
    }

    /// One row's cells, up to the row's end.
    fn cells(&mut self) -> Vec<Inline> {
        let mut cells = Vec::new();
        while let Some(event) = self.events.next() {
            match event {
                Event::Start(Tag::TableCell) if !self.admit(1, 0) => {
                    self.overflow("");
                    break;
                }
                Event::Start(Tag::TableCell) => cells.push(self.inline(Until::End)),
                Event::End(_) => break,
                _ => {}
            }
        }
        cells
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markdown::Span;

    fn text(s: &str) -> Inline {
        Inline {
            spans: vec![Span {
                text: s.into(),
                marks: Marks::default(),
                link: None,
            }],
        }
    }

    #[test]
    fn headings_paragraphs_and_breaks() {
        let doc = parse("# Title\n\nOne\ntwo  \nthree\n\n###### Small");
        assert_eq!(
            doc.blocks,
            vec![
                Block::Heading {
                    level: 1,
                    text: text("Title")
                },
                Block::Paragraph(text("One two\nthree")),
                Block::Heading {
                    level: 6,
                    text: text("Small")
                },
            ]
        );
    }

    #[test]
    fn marks_nest_and_links_carry_their_address() {
        let doc = parse("a **b _c_** ~~d~~ `e` [f](https://x.invalid/)");
        let Block::Paragraph(inline) = &doc.blocks[0] else {
            panic!("{doc:?}");
        };
        let find = |t: &str| inline.spans.iter().find(|s| s.text == t).unwrap();
        assert!(find("b ").marks.strong && !find("b ").marks.emphasis);
        assert!(find("c").marks.strong && find("c").marks.emphasis);
        assert!(find("d").marks.strike);
        assert!(find("e").marks.code);
        assert_eq!(
            find("f").link,
            Some(Link {
                url: "https://x.invalid/".into(),
                image: false
            })
        );
        assert_eq!(inline.text(), "a b c d e f");
    }

    #[test]
    fn code_blocks_keep_their_text_and_info() {
        let doc = parse("```rust\nfn main() {}\n    let x;\n```\n\n    indented\n");
        assert_eq!(
            doc.blocks,
            vec![
                Block::Code {
                    info: "rust".into(),
                    text: "fn main() {}\n    let x;".into()
                },
                Block::Code {
                    info: String::new(),
                    text: "indented".into()
                },
            ]
        );
    }

    #[test]
    fn nested_lists_tasks_and_quotes() {
        let doc = parse("- [x] done\n- [ ] todo\n  1. one\n  2. two\n\n> outer\n>> inner\n");
        let Block::List { start, items } = &doc.blocks[0] else {
            panic!("{doc:?}");
        };
        assert_eq!(*start, None);
        assert_eq!(items[0].task, Some(true));
        assert_eq!(items[0].blocks, vec![Block::Paragraph(text("done"))]);
        assert_eq!(items[1].task, Some(false));
        let Block::List {
            start,
            items: inner,
        } = &items[1].blocks[1]
        else {
            panic!("{:?}", items[1]);
        };
        assert_eq!(*start, Some(1));
        assert_eq!(inner.len(), 2);
        assert_eq!(inner[0].task, None, "the outer task stays the outer's");
        let Block::Quote(quote) = &doc.blocks[1] else {
            panic!("{doc:?}");
        };
        assert_eq!(quote[0], Block::Paragraph(text("outer")));
        assert!(
            matches!(&quote[1], Block::Quote(inner) if inner[0] == Block::Paragraph(text("inner")))
        );
    }

    #[test]
    fn a_loose_task_item_keeps_its_box() {
        let doc = parse("- [x] first\n\n- [ ] second\n");
        let Block::List { items, .. } = &doc.blocks[0] else {
            panic!("{doc:?}");
        };
        assert_eq!(
            items.iter().map(|i| i.task).collect::<Vec<_>>(),
            [Some(true), Some(false)]
        );
    }

    #[test]
    fn tables_rules_and_raw_html() {
        let doc = parse(
            "| a | b |\n|:--|--:|\n| 1 | 2 |\n\n---\n\n<div onclick=\"x\">hi</div>\n\nan <b>inline</b> tag",
        );
        assert_eq!(
            doc.blocks[0],
            Block::Table {
                align: vec![Align::Left, Align::Right],
                head: vec![text("a"), text("b")],
                rows: vec![vec![text("1"), text("2")]],
            }
        );
        assert_eq!(doc.blocks[1], Block::Rule);
        assert_eq!(
            doc.blocks[2],
            Block::Html("<div onclick=\"x\">hi</div>".into())
        );
        assert_eq!(
            doc.blocks[3],
            Block::Paragraph(text("an <b>inline</b> tag"))
        );
    }

    #[test]
    fn images_are_links_with_their_alt_text() {
        let doc = parse("![a cat](https://track.invalid/p.gif)");
        let Block::Paragraph(inline) = &doc.blocks[0] else {
            panic!("{doc:?}");
        };
        assert_eq!(inline.text(), "a cat");
        assert_eq!(
            inline.spans[0].link,
            Some(Link {
                url: "https://track.invalid/p.gif".into(),
                image: true
            })
        );
    }

    /// How deep quotes and list items nest in `blocks`.
    fn depth(blocks: &[Block]) -> usize {
        blocks
            .iter()
            .map(|block| match block {
                Block::Quote(inner) => 1 + depth(inner),
                Block::List { items, .. } => {
                    1 + items.iter().map(|i| depth(&i.blocks)).max().unwrap_or(0)
                }
                _ => 0,
            })
            .max()
            .unwrap_or(0)
    }

    /// Every piece of text in `blocks`, in order.
    fn all_text(blocks: &[Block], out: &mut String) {
        for block in blocks {
            match block {
                Block::Paragraph(text) | Block::Heading { text, .. } => out.push_str(&text.text()),
                Block::Quote(inner) => all_text(inner, out),
                Block::List { items, .. } => items.iter().for_each(|i| all_text(&i.blocks, out)),
                Block::Code { text, .. } | Block::Html(text) => out.push_str(text),
                Block::Table { head, rows, .. } => {
                    head.iter()
                        .chain(rows.iter().flatten())
                        .for_each(|c| out.push_str(&c.text()));
                }
                Block::Rule => {}
            }
            out.push('\n');
        }
    }

    /// The nodes (blocks and spans) and bytes (text, and each distinct link
    /// address once) a document holds.
    fn weight(blocks: &[Block], seen: &mut Vec<Arc<str>>) -> (usize, usize) {
        let inline = |text: &Inline, seen: &mut Vec<Arc<str>>| {
            let mut bytes = 0;
            for span in &text.spans {
                bytes += span.text.len();
                if let Some(link) = &span.link
                    && !seen.iter().any(|url| Arc::ptr_eq(url, &link.url))
                {
                    bytes += link.url.len();
                    seen.push(link.url.clone());
                }
            }
            (text.spans.len(), bytes)
        };
        let mut total = (0, 0);
        let mut add = |(n, b): (usize, usize)| {
            total.0 += n;
            total.1 += b;
        };
        for block in blocks {
            add((1, 0));
            match block {
                Block::Paragraph(text) | Block::Heading { text, .. } => add(inline(text, seen)),
                Block::Quote(inner) => add(weight(inner, seen)),
                Block::List { items, .. } => {
                    for item in items {
                        add((1, 0));
                        add(weight(&item.blocks, seen));
                    }
                }
                Block::Code { text, .. } | Block::Html(text) => add((0, text.len())),
                Block::Table { head, rows, .. } => {
                    for cell in head.iter().chain(rows.iter().flatten()) {
                        add((1, 0));
                        add(inline(cell, seen));
                    }
                }
                Block::Rule => {}
            }
        }
        total
    }

    #[test]
    fn ten_thousand_nested_quotes_and_list_markers_keep_a_bounded_depth() {
        let sources = [
            "> ".repeat(10_000) + "deep",
            ">".repeat(10_000) + " deep",
            "- ".repeat(10_000) + "deep",
            "1. ".repeat(10_000) + "deep",
            "> - ".repeat(5_000) + "deep",
            "> ".repeat(10_000) + "deep\n" + &"> ".repeat(9_999) + "- also",
        ];
        for source in &sources {
            let doc = parse(source);
            assert!(depth(&doc.blocks) <= MAX_DEPTH, "{}", depth(&doc.blocks));
            let mut text = String::new();
            all_text(&doc.blocks, &mut text);
            assert!(text.contains("deep"), "the deep text is kept as plain text");
        }
        // Up to the limit, quotes stay quotes.
        let doc = parse(&("> ".repeat(MAX_DEPTH) + "x"));
        assert_eq!(depth(&doc.blocks), MAX_DEPTH);
    }

    #[test]
    fn a_huge_link_with_many_spans_shares_one_address_within_the_budget() {
        let url = format!("https://x.invalid/{}", "a".repeat(4 * 1024 * 1024));
        let words: String = (0..10_000).map(|_| "**b** _i_ ").collect();
        let source = format!("[{words}]({url})");
        let started = std::time::Instant::now();
        let doc = parse(&source);
        let elapsed = started.elapsed();
        let Block::Paragraph(inline) = &doc.blocks[0] else {
            panic!("{:?}", doc.blocks.first().map(|_| ()));
        };
        assert!(inline.spans.len() >= 30_000, "{}", inline.spans.len());
        let first = inline.spans[0].link.clone().expect("linked");
        assert!(
            inline.spans.iter().all(|s| s
                .link
                .as_ref()
                .is_some_and(|l| Arc::ptr_eq(&l.url, &first.url))),
            "one shared address"
        );
        let (nodes, bytes) = weight(&doc.blocks, &mut Vec::new());
        assert!(
            nodes <= MAX_NODES && bytes <= MAX_BYTES,
            "{nodes} nodes, {bytes} bytes"
        );
        assert!(bytes < url.len() + words.len(), "the address is held once");
        eprintln!(
            "4 MiB link, {} spans: parsed in {elapsed:?}",
            inline.spans.len()
        );
    }

    #[test]
    fn past_the_node_budget_the_rest_is_plain_text() {
        let source: String = (0..MAX_NODES + 5_000)
            .map(|i| format!("w{i}\n\n"))
            .collect();
        let doc = parse(&source);
        let (nodes, _) = weight(&doc.blocks, &mut Vec::new());
        assert!(nodes <= MAX_NODES + 2, "{nodes}");
        let Some(Block::Paragraph(rest)) = doc.blocks.last() else {
            panic!("no fallback");
        };
        let rest = rest.text();
        assert!(
            rest.ends_with(&format!("w{}", MAX_NODES + 4_999)),
            "nothing is lost"
        );
        assert!(rest.contains('\n'), "one line per paragraph");
    }

    #[test]
    fn empty_and_odd_input_parses() {
        assert_eq!(parse("").blocks, vec![]);
        // Unclosed marks and fences still give blocks.
        assert!(!parse("**open\n```\nno end").blocks.is_empty());
    }
}
