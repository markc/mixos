// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! pulldown-cmark's event stream folded into a [`Document`].
//!
//! Each container (quote, list item, table cell) is read by a recursive
//! call that returns at its own end event; nesting is well formed, so any
//! end a call meets that it did not open is its own. A tight list item's
//! text arrives with no paragraph round it and is read as a run that stops
//! at the first block event, without consuming it.

use std::iter::Peekable;

use pulldown_cmark::{Alignment, CodeBlockKind, Event, Options, Parser, Tag, TagEnd};

use super::{Align, Block, Document, Inline, Item, Link, Marks};

pub fn parse(source: &str) -> Document {
    let options =
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let mut reader = Reader {
        events: Parser::new_ext(source, options).peekable(),
        tasks: Vec::new(),
    };
    Document {
        blocks: reader.blocks(),
    }
}

struct Reader<'a> {
    events: Peekable<Parser<'a>>,
    /// The task box of each list item being read, innermost last: a task
    /// marker belongs to the innermost item.
    tasks: Vec<Option<bool>>,
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
            match event {
                Event::End(_) => break,
                Event::Start(tag) => match tag {
                    Tag::Paragraph => out.push(Block::Paragraph(self.inline(Until::End))),
                    Tag::Heading { level, .. } => out.push(Block::Heading {
                        level: level as u8,
                        text: self.inline(Until::End),
                    }),
                    Tag::BlockQuote(_) => out.push(Block::Quote(self.blocks())),
                    Tag::CodeBlock(kind) => {
                        let info = match kind {
                            CodeBlockKind::Fenced(info) => info.trim().to_owned(),
                            CodeBlockKind::Indented => String::new(),
                        };
                        let mut text = self.literal();
                        if text.ends_with('\n') {
                            text.pop();
                        }
                        out.push(Block::Code { info, text });
                    }
                    Tag::HtmlBlock => {
                        let mut text = self.literal();
                        if text.ends_with('\n') {
                            text.pop();
                        }
                        out.push(Block::Html(text));
                    }
                    Tag::List(start) => out.push(Block::List {
                        start,
                        items: self.items(),
                    }),
                    Tag::Table(align) => out.push(self.table(&align)),
                    // Footnotes, definition lists and metadata are not
                    // enabled; anything else keeps its children.
                    _ => out.extend(self.blocks()),
                },
                Event::Rule => out.push(Block::Rule),
                Event::Html(text) | Event::DisplayMath(text) => {
                    out.push(Block::Html(text.trim_end().to_owned()));
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
        let marks = |open: &[Open], code: bool| Marks {
            strong: open.iter().any(|o| matches!(o, Open::Strong)),
            emphasis: open.iter().any(|o| matches!(o, Open::Emphasis)),
            strike: open.iter().any(|o| matches!(o, Open::Strike)),
            code,
        };
        let link = |open: &[Open]| -> Option<Link> {
            open.iter().rev().find_map(|o| match o {
                Open::Link(link) => Some(link.clone()),
                _ => None,
            })
        };
        while let Some(event) = self.events.peek() {
            if until == Until::Block && !is_inline(event, !open.is_empty()) {
                break;
            }
            let Some(event) = self.events.next() else {
                break;
            };
            match event {
                Event::Text(text) => out.push(&text, marks(&open, false), link(&open).as_ref()),
                Event::Code(text) | Event::InlineMath(text) => {
                    out.push(&text, marks(&open, true), link(&open).as_ref());
                }
                // Never interpreted: the source, as text.
                Event::InlineHtml(text) | Event::Html(text) => {
                    out.push(&text, marks(&open, false), link(&open).as_ref());
                }
                Event::FootnoteReference(name) => {
                    out.push(&format!("[^{name}]"), marks(&open, false), None);
                }
                Event::SoftBreak => out.push(" ", marks(&open, false), link(&open).as_ref()),
                Event::HardBreak => out.push("\n", marks(&open, false), None),
                Event::TaskListMarker(done) => {
                    if let Some(task) = self.tasks.last_mut() {
                        *task = Some(done);
                    }
                }
                Event::Start(Tag::Emphasis) => open.push(Open::Emphasis),
                Event::Start(Tag::Strong) => open.push(Open::Strong),
                Event::Start(Tag::Strikethrough) => open.push(Open::Strike),
                Event::Start(Tag::Link { dest_url, .. }) => open.push(Open::Link(Link {
                    url: dest_url.into_string(),
                    image: false,
                })),
                Event::Start(Tag::Image { dest_url, .. }) => open.push(Open::Link(Link {
                    url: dest_url.into_string(),
                    image: true,
                })),
                Event::End(tag) if inline_end(&tag) && !open.is_empty() => {
                    open.pop();
                }
                // The container's own end.
                Event::End(_) => break,
                _ => {}
            }
        }
        out
    }

    /// A list's items, up to the list's end.
    fn items(&mut self) -> Vec<Item> {
        let mut items = Vec::new();
        while let Some(event) = self.events.next() {
            match event {
                Event::Start(Tag::Item) => {
                    self.tasks.push(None);
                    let blocks = self.blocks();
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

    #[test]
    fn empty_and_odd_input_parses() {
        assert_eq!(parse("").blocks, vec![]);
        // Unclosed marks and fences still give blocks.
        assert!(!parse("**open\n```\nno end").blocks.is_empty());
    }
}
