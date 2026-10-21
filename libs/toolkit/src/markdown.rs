// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Markdown, parsed once and drawn natively.
//!
//! [`Document::parse`] turns CommonMark (with GFM tables, strikethrough and
//! task lists) into plain data, with no egui in it: an app parses a text
//! once, keeps the [`Document`] in its state (or uses [`Preview`] to cache
//! it), and calls [`show`] every frame. [`looks_like_markdown`] is the
//! conservative test for plain text that was written as Markdown (an
//! LLM's reply, a README pasted into mail).
//!
//! What [`show`] draws, all from the theme:
//!
//! | element | drawn as |
//! |---|---|
//! | headings 1–6 | the heading face, at sizes stepped from the `Heading` and `Body` text styles |
//! | paragraphs | the `Body` style; a soft break is a space, a hard break a new line |
//! | emphasis, strong, strikethrough | slanted, the heading face's weight, struck |
//! | inline code | the `Monospace` style on `code_bg_color` |
//! | code blocks | the `Monospace` style on the `field` surface, scrolled sideways, never wrapped |
//! | block quotes | indented after a `text_faint` rule, in `text_dim`; nested quotes nest |
//! | lists | bullets or numbers in a gutter; a task's read-only box as [`crate::toggle`] draws it, filled in `hyperlink_color` |
//! | tables | a grid with a strong header row and `separator` lines |
//! | rules | a `separator` hairline |
//! | links | `hyperlink_color`, underlined; the address on hover |
//! | images | never fetched: the alt text as a link to the image |
//! | raw HTML | its source as literal text, never interpreted |
//!
//! Every run of text is one selectable label, so a person selects and
//! copies across a paragraph (and, as far as egui allows, across labels).
//! A link opens with [`egui::Context::open_url`] only when [`openable`]
//! (http, https or mailto); the app's shell calls [`open_links`] to hand
//! those requests to the desktop. [`show`] also returns the link activated
//! this frame.

mod detect;
mod parse;
mod preview;
mod view;

pub use detect::looks_like_markdown;
pub use preview::Preview;
pub use view::show;

/// A parsed Markdown text: its blocks in order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Document {
    pub blocks: Vec<Block>,
}

impl Document {
    /// Parse `source` as CommonMark with GFM tables, strikethrough and task
    /// lists. Never fails: any text is some Markdown.
    pub fn parse(source: &str) -> Self {
        parse::parse(source)
    }
}

/// One block.
#[derive(Clone, Debug, PartialEq)]
pub enum Block {
    /// `level` 1 to 6.
    Heading {
        level: u8,
        text: Inline,
    },
    Paragraph(Inline),
    /// A fenced or indented code block; `info` is the fence's info string.
    Code {
        info: String,
        text: String,
    },
    Quote(Vec<Block>),
    /// `start` is the first number of an ordered list, `None` for bullets.
    List {
        start: Option<u64>,
        items: Vec<Item>,
    },
    Table {
        align: Vec<Align>,
        head: Vec<Inline>,
        rows: Vec<Vec<Inline>>,
    },
    Rule,
    /// A raw HTML block, kept as its source text.
    Html(String),
}

/// One list item: its task box, if it is a task, and its blocks.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Item {
    pub task: Option<bool>,
    pub blocks: Vec<Block>,
}

/// A table column's alignment.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Align {
    #[default]
    None,
    Left,
    Center,
    Right,
}

/// A run of inline text.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Inline {
    pub spans: Vec<Span>,
}

impl Inline {
    /// The text with every mark dropped.
    pub fn text(&self) -> String {
        self.spans.iter().map(|s| s.text.as_str()).collect()
    }

    pub fn is_empty(&self) -> bool {
        self.spans.iter().all(|s| s.text.is_empty())
    }

    /// Append `text` with `marks` and `link`, joining the last span when it
    /// has the same ones.
    pub(crate) fn push(&mut self, text: &str, marks: Marks, link: Option<&Link>) {
        if text.is_empty() {
            return;
        }
        if let Some(last) = self.spans.last_mut()
            && last.marks == marks
            && last.link.as_ref() == link
        {
            last.text.push_str(text);
            return;
        }
        self.spans.push(Span {
            text: text.to_owned(),
            marks,
            link: link.cloned(),
        });
    }
}

/// Text with the same marks and link throughout.
#[derive(Clone, Debug, PartialEq)]
pub struct Span {
    pub text: String,
    pub marks: Marks,
    pub link: Option<Link>,
}

/// The inline marks on a span.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Marks {
    pub strong: bool,
    pub emphasis: bool,
    pub strike: bool,
    pub code: bool,
}

/// Where a span points. An image is a link to the image, never fetched
/// (remote images in mail are a tracking risk); its span is the alt text.
///
/// The address is shared: every span of one link holds the same `Arc`, so
/// a long address with many styled spans is stored once. Comparing two
/// spans of one link is a pointer test (`Arc<str>`'s `PartialEq` checks
/// identity before content).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    pub url: std::sync::Arc<str>,
    pub image: bool,
}

/// The most nested quotes and list items read as structure; deeper text
/// is kept as plain text in the innermost container. Every layer of the
/// document (parse, draw, drop) recurses once per level.
pub const MAX_DEPTH: usize = 32;

/// The most blocks and spans one document holds. Past it, the rest of the
/// source is shown as one plain-text paragraph.
pub const MAX_NODES: usize = 50_000;

/// The most bytes of text and link addresses one document holds (each
/// address counted once, however many spans share it). Past it, the rest of
/// the source is shown as one plain-text paragraph.
pub const MAX_BYTES: usize = 16 * 1024 * 1024;

/// Whether a link may be opened in the desktop's handler: http, https and
/// mailto only (any case), never a file, a script or an app's own scheme.
pub fn openable(url: &str) -> bool {
    let url = url.trim_start();
    ["http://", "https://", "mailto:"].iter().any(|scheme| {
        url.get(..scheme.len())
            .is_some_and(|head| head.eq_ignore_ascii_case(scheme))
    })
}

/// Hand the frame's link requests to the desktop's handler, detached.
/// eframe without its browser launcher leaves these in egui's output;
/// take every OpenUrl request, but open only http, https and mailto, as
/// one argument and never through a shell. `app_name` labels errors and
/// the thread that reaps the child (`<app_name>-xdg-open-reap`).
pub fn open_links(ctx: &egui::Context, app_name: &str) {
    let urls = ctx.output_mut(|output| take_open_links(&mut output.commands));
    for url in urls {
        match std::process::Command::new("xdg-open")
            .arg(url.trim())
            .spawn()
        {
            Ok(mut child) => {
                // Reaped off the UI thread, so it never lingers as a zombie.
                let reaped = std::thread::Builder::new()
                    .name(format!("{app_name}-xdg-open-reap"))
                    .spawn(move || drop(child.wait()));
                if reaped.is_err() {
                    eprintln!("{app_name}: the link opener's reaper did not start");
                }
            }
            Err(error) => eprintln!("{app_name}: opening a link: {error}"),
        }
    }
}

fn take_open_links(commands: &mut Vec<egui::OutputCommand>) -> Vec<String> {
    let mut urls = Vec::new();
    commands.retain(|command| match command {
        egui::OutputCommand::OpenUrl(open) => {
            if openable(&open.url) {
                urls.push(open.url.clone());
            }
            false
        }
        _ => true,
    });
    urls
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_requests_are_taken_and_filtered_without_opening_them() {
        use egui::{OpenUrl, OutputCommand};

        let mut commands = vec![OutputCommand::CopyText("before".into())];
        let allowed = [
            " https://example.invalid/a?one=1&two=2 ",
            "HTTP://example.invalid",
            "mailto:ann@example.invalid",
        ];
        for url in allowed.into_iter().chain([
            "file:///home/user/document.md",
            "javascript:alert(1)",
            "data:text/html,x",
            "dev.mixos.email:x",
            "/relative",
            "",
        ]) {
            commands.push(OutputCommand::OpenUrl(OpenUrl::new_tab(url)));
        }
        commands.push(OutputCommand::CopyText("after".into()));

        assert_eq!(take_open_links(&mut commands), allowed);
        assert_eq!(commands.len(), 2);
        assert!(matches!(&commands[0], OutputCommand::CopyText(text) if text == "before"));
        assert!(matches!(&commands[1], OutputCommand::CopyText(text) if text == "after"));
        assert!(take_open_links(&mut commands).is_empty());
    }

    #[test]
    fn only_web_and_mail_links_open() {
        assert!(openable("https://mixos.dev/"));
        assert!(openable("HTTP://example.invalid"));
        assert!(openable("mailto:ann@example.invalid"));
        for url in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "data:text/html,x",
            "dev.mixos.email:x",
            "/relative",
            "",
        ] {
            assert!(!openable(url), "{url}");
        }
    }
}
