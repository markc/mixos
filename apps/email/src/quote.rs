// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! A reply's quoted original, as plain text: pure functions, no state.
//!
//! The quote is the original text body (a Markdown message's source; an
//! HTML-only message's text, tags dropped) with its own signature cut off
//! at the `-- ` line, and every line marked: `> ` before plain text, a bare
//! `>` before a line that is already quoted (so `> a` becomes `>> a`) and
//! before an empty one. Lines are never re-wrapped. An attribution line
//! ("On …, Ann wrote:") goes before it, and the reply's own text and the
//! signature go above or below it.

/// Where the person's text goes relative to the quote.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Position {
    /// Their text and signature first, the quote under them.
    #[default]
    Above,
    /// The quote first, their text and signature under it.
    Below,
}

/// `text` without its signature: everything from the first line that is
/// exactly `-- ` (RFC 3676) on. Line ends are normalised to `\n`.
pub fn unsigned(text: &str) -> String {
    let text = text.replace("\r\n", "\n");
    let mut out = Vec::new();
    for line in text.split('\n') {
        if line == "-- " {
            break;
        }
        out.push(line);
    }
    while out.last().is_some_and(|l| l.trim().is_empty()) {
        out.pop();
    }
    out.join("\n")
}

/// `text` (signature already cut) with every line marked as quoted.
pub fn quoted(text: &str) -> String {
    text.replace("\r\n", "\n")
        .split('\n')
        .map(|line| {
            if line.starts_with('>') || line.is_empty() {
                format!(">{line}")
            } else {
                format!("> {line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The draft body of a reply: the person's (empty) text, `signature` (the
/// `-- ` block, or empty), and `attribution` with the quoted `original`, in
/// `position`'s order. The cursor's line is the body's first in `Above`
/// and the first after the quote in `Below`.
pub fn reply_body(original: &str, attribution: &str, signature: &str, position: Position) -> String {
    let quote = quoted(&unsigned(original));
    let quote = if quote.is_empty() || original.trim().is_empty() {
        attribution.to_owned()
    } else {
        format!("{attribution}\n{quote}")
    };
    let signature = signature.trim_matches('\n');
    match position {
        Position::Above if signature.is_empty() => format!("\n\n{quote}\n"),
        Position::Above => format!("\n\n{signature}\n\n{quote}\n"),
        Position::Below if signature.is_empty() => format!("{quote}\n\n\n"),
        Position::Below => format!("{quote}\n\n\n\n{signature}\n"),
    }
}

/// The text of an HTML body, for quoting a message that has no plain
/// part: tags dropped, block ends and `<br>` as new lines, the common
/// entities decoded, and runs of blank lines folded to one.
pub fn html_text(html: &str) -> String {
    let mut out = String::new();
    let mut rest = html;
    // Script and style contents are never text.
    let mut skipping: Option<&str> = None;
    while let Some(open) = rest.find('<') {
        if skipping.is_none() {
            out.push_str(&rest[..open]);
        }
        let Some(close) = rest[open..].find('>') else {
            rest = "";
            break;
        };
        let tag = rest[open + 1..open + close].trim().to_ascii_lowercase();
        rest = &rest[open + close + 1..];
        let name: String = tag
            .trim_start_matches('/')
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect();
        if let Some(until) = skipping {
            if tag.starts_with('/') && name == until {
                skipping = None;
            }
            continue;
        }
        match name.as_str() {
            "script" | "style" | "head" if !tag.starts_with('/') => {
                skipping = Some(match name.as_str() {
                    "script" => "script",
                    "style" => "style",
                    _ => "head",
                });
            }
            "br" => out.push('\n'),
            "p" | "div" | "tr" | "li" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "blockquote"
            | "pre" | "table" | "ul" | "ol" => out.push('\n'),
            _ => {}
        }
    }
    if skipping.is_none() {
        out.push_str(rest);
    }
    let decoded = out
        .replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&amp;", "&");
    let mut lines: Vec<&str> = Vec::new();
    for line in decoded.lines().map(str::trim_end) {
        if line.trim().is_empty() && lines.last().is_none_or(|l| l.trim().is_empty()) {
            continue;
        }
        lines.push(line);
    }
    while lines.last().is_some_and(|l| l.trim().is_empty()) {
        lines.pop();
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    const ATTRIBUTION: &str = "On Fri 17 Oct 2026, 09:14, Ann wrote:";
    const SIG: &str = "-- \nBob\n";

    #[test]
    fn lines_are_marked_and_nested_quotes_take_no_space() {
        assert_eq!(quoted("hi\n> earlier\n>> older\n\nend"), "> hi\n>> earlier\n>>> older\n>\n> end");
    }

    #[test]
    fn crlf_is_read_as_lf() {
        assert_eq!(quoted("a\r\nb"), "> a\n> b");
        assert_eq!(unsigned("a\r\n-- \r\nsig"), "a");
    }

    #[test]
    fn the_original_signature_is_cut() {
        assert_eq!(unsigned("Thanks!\n\n-- \nAnn\nExample Pty"), "Thanks!");
        // "--" without its space, or dashes in text, are not the separator.
        assert_eq!(unsigned("a\n--\nb"), "a\n--\nb");
    }

    #[test]
    fn long_lines_are_never_rewrapped() {
        let long = "word ".repeat(80);
        let q = quoted(long.trim_end());
        assert_eq!(q.lines().count(), 1);
        assert_eq!(q, format!("> {}", long.trim_end()));
    }

    #[test]
    fn above_puts_the_person_and_signature_first() {
        let body = reply_body("Lunch?\n-- \nAnn", ATTRIBUTION, SIG, Position::Above);
        assert_eq!(body, format!("\n\n-- \nBob\n\n{ATTRIBUTION}\n> Lunch?\n"));
        assert!(body.starts_with('\n'), "the cursor line is empty and first");
    }

    #[test]
    fn below_puts_the_quote_first() {
        let body = reply_body("Lunch?", ATTRIBUTION, SIG, Position::Below);
        assert_eq!(body, format!("{ATTRIBUTION}\n> Lunch?\n\n\n\n-- \nBob\n"));
        let unsigned_body = reply_body("Lunch?", ATTRIBUTION, "", Position::Below);
        assert_eq!(unsigned_body, format!("{ATTRIBUTION}\n> Lunch?\n\n\n"));
    }

    #[test]
    fn an_empty_original_keeps_only_the_attribution() {
        assert_eq!(
            reply_body("", ATTRIBUTION, "", Position::Above),
            format!("\n\n{ATTRIBUTION}\n")
        );
        assert_eq!(
            reply_body("\r\n\r\n", ATTRIBUTION, "", Position::Above),
            format!("\n\n{ATTRIBUTION}\n")
        );
    }

    #[test]
    fn html_reads_as_text() {
        let html = "<html><head><style>p{x}</style></head><body><p>Hi &amp; bye</p>\
                    <p>line<br>two</p><script>alert(1)</script><div>&lt;tag&gt;</div></body></html>";
        assert_eq!(html_text(html), "Hi & bye\n\nline\ntwo\n\n<tag>");
    }
}
