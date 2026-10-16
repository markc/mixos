// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Whether plain text was written as Markdown.
//!
//! Rendering plain mail as Markdown is worse than not rendering Markdown:
//! hard-wrapped lines reflow, and ASCII art, signatures and quoted replies
//! fall apart. So the test is conservative. It scans the text outside
//! quoted lines (`>`) and outside the signature (after a `-- ` line) and
//! records which signals it holds:
//!
//! | signal | counts when |
//! |---|---|
//! | fenced code | a ```` ``` ```` or `~~~` line with its closing fence later |
//! | table | a `\|---\|---\|` separator row under a header with the same cell count |
//! | heading | an ATX `#`–`######` line whose text has a letter or digit |
//! | list | two or more lines that start with `-`, `*`, `+`, `1.` or `1)` |
//! | strong or emphasis | `**text**`, `__text__` or `_text_` (not inside a word) |
//! | inline code | `` `text` `` |
//! | link | `[text](http…)`, `[text](mailto:…)` or a `[label]: http…` definition |
//! | block quote | `>` lines that are not a reply's quote (no "wrote:" or "Original Message" line before them) |
//!
//! It answers yes for one very strong signal (a fence, a table, or two
//! headings) or for two distinct strong ones. A bare URL, a single `- `
//! line, a quoted reply or forwarded headers never count.

/// The signals found.
#[derive(Debug, Default)]
struct Signals {
    fence: bool,
    table: bool,
    headings: usize,
    list_lines: usize,
    emphasis: bool,
    code: bool,
    link: bool,
    quote: bool,
}

impl Signals {
    fn markdown(&self) -> bool {
        if self.fence || self.table || self.headings >= 2 {
            return true;
        }
        let strong = [
            self.headings == 1,
            self.list_lines >= 2,
            self.emphasis,
            self.code,
            self.link,
            self.quote,
        ];
        strong.iter().filter(|s| **s).count() >= 2
    }
}

/// Whether `text` looks like Markdown, conservatively (see the module).
pub fn looks_like_markdown(text: &str) -> bool {
    signals(text).markdown()
}

fn signals(text: &str) -> Signals {
    let mut found = Signals::default();
    // The signature separator ends the scan (RFC 3676 "-- ", and the
    // common "--" without its space).
    let lines: Vec<&str> = text
        .lines()
        .map(|l| l.trim_end_matches('\r'))
        .take_while(|l| *l != "-- " && *l != "--")
        .collect();
    let mut fence: Option<(char, usize)> = None;
    let mut previous = "";
    // The last non-blank, non-quoted line: the attribution a reply's quote
    // follows.
    let mut lead = "";
    for &line in &lines {
        if let Some((mark, width)) = fence {
            if fence_marker(line)
                .is_some_and(|(m, w, info)| m == mark && w >= width && info.is_empty())
            {
                found.fence = true;
                fence = None;
            }
            continue;
        }
        let trimmed = line.trim_start();
        if trimmed.starts_with('>') {
            if !attribution(lead) && !trimmed.trim_start_matches('>').trim().is_empty() {
                found.quote = true;
            }
            previous = line;
            continue;
        }
        if let Some((mark, width, _)) = fence_marker(line) {
            // Only a fence that closes later counts.
            fence = Some((mark, width));
            previous = line;
            continue;
        }
        if heading(line) {
            found.headings += 1;
        }
        if list_item(line) {
            found.list_lines += 1;
        }
        if separator(line).is_some_and(|cells| cells >= 2 && cells == row_cells(previous)) {
            found.table = true;
        }
        found.emphasis |= emphasis(line);
        found.code |= inline_code(line);
        found.link |= link(line);
        if !line.trim().is_empty() {
            lead = line;
        }
        previous = line;
    }
    found
}

/// A reply's attribution line: "On … wrote:", "… a écrit :", Outlook's
/// "-----Original Message-----".
fn attribution(line: &str) -> bool {
    let line = line.trim().to_ascii_lowercase();
    line.ends_with("wrote:")
        || line.ends_with("writes:")
        || line.ends_with("écrit :")
        || line.ends_with("schrieb:")
        || line.contains("original message")
}

/// The leading spaces of `line` when there are at most three (a deeper
/// indent is an indented code line, not a marker).
fn indent(line: &str) -> Option<&str> {
    let rest = line.trim_start_matches(' ');
    (line.len() - rest.len() <= 3).then_some(rest)
}

/// A fence line: its character, its width and its info string.
fn fence_marker(line: &str) -> Option<(char, usize, &str)> {
    let rest = indent(line)?;
    let mark = rest.chars().next().filter(|c| *c == '`' || *c == '~')?;
    let width = rest.chars().take_while(|c| *c == mark).count();
    if width < 3 {
        return None;
    }
    let info = rest[width..].trim();
    // A backtick fence's info string has no backtick (CommonMark 4.5).
    if mark == '`' && info.contains('`') {
        return None;
    }
    Some((mark, width, info))
}

/// An ATX heading with a letter or digit in it (`# # #` is ASCII art).
fn heading(line: &str) -> bool {
    let Some(rest) = indent(line) else {
        return false;
    };
    let hashes = rest.chars().take_while(|c| *c == '#').count();
    if !(1..=6).contains(&hashes) {
        return false;
    }
    let text = &rest[hashes..];
    if !text.starts_with(' ') && !text.starts_with('\t') {
        return false;
    }
    let text = text.trim().trim_end_matches('#').trim();
    !text.contains('#') && text.chars().any(char::is_alphanumeric)
}

/// A bullet or ordered list item with text after its marker.
fn list_item(line: &str) -> bool {
    let rest = line.trim_start();
    let after = if let Some(after) = rest
        .strip_prefix("- ")
        .or_else(|| rest.strip_prefix("* "))
        .or_else(|| rest.strip_prefix("+ "))
    {
        after
    } else {
        let digits = rest.chars().take_while(char::is_ascii_digit).count();
        if !(1..=9).contains(&digits) {
            return false;
        }
        let Some(after) = rest[digits..]
            .strip_prefix(". ")
            .or_else(|| rest[digits..].strip_prefix(") "))
        else {
            return false;
        };
        after
    };
    after.chars().any(char::is_alphanumeric)
}

/// A table separator row's cell count: cells of `-` (at least three) with
/// optional `:` ends, between pipes.
fn separator(line: &str) -> Option<usize> {
    let line = line.trim();
    if !line.contains('|') || !line.contains("---") {
        return None;
    }
    let inner = line.strip_prefix('|').unwrap_or(line);
    let inner = inner.strip_suffix('|').unwrap_or(inner);
    let mut cells = 0;
    for cell in inner.split('|') {
        let cell = cell.trim();
        let dashes = cell.trim_start_matches(':').trim_end_matches(':');
        if dashes.len() < 3 || !dashes.chars().all(|c| c == '-') {
            return None;
        }
        cells += 1;
    }
    Some(cells)
}

/// How many cells a header row has (0 when it is not a pipe row).
fn row_cells(line: &str) -> usize {
    let line = line.trim();
    if !line.contains('|') {
        return 0;
    }
    let inner = line.strip_prefix('|').unwrap_or(line);
    let inner = inner.strip_suffix('|').unwrap_or(inner);
    inner.split('|').count()
}

/// Whether `delimiter` wraps some text on `line`: it opens before a
/// non-space, closes after one, and (for `_`) neither end touches a word.
fn wrapped(line: &str, delimiter: &str, intraword: bool) -> bool {
    let mut from = 0;
    while let Some(start) = line[from..].find(delimiter).map(|i| i + from) {
        let body_start = start + delimiter.len();
        let before = line[..start].chars().next_back();
        let open_ok = line[body_start..]
            .chars()
            .next()
            .is_some_and(|c| !c.is_whitespace() && !delimiter.starts_with(c))
            && (intraword || before.is_none_or(|c| !c.is_alphanumeric()));
        if open_ok && let Some(len) = line[body_start..].find(delimiter) {
            let end = body_start + len;
            let body = &line[body_start..end];
            let after = line[end + delimiter.len()..].chars().next();
            let close_ok = body.chars().next_back().is_some_and(|c| !c.is_whitespace())
                && (intraword || after.is_none_or(|c| !c.is_alphanumeric()));
            if close_ok && body.chars().any(char::is_alphanumeric) {
                return true;
            }
        }
        from = body_start;
    }
    false
}

fn emphasis(line: &str) -> bool {
    wrapped(line, "**", true) || wrapped(line, "__", false) || wrapped(line, "_", false)
}

fn inline_code(line: &str) -> bool {
    wrapped(line, "`", true)
}

/// `[text](http…)` or `(mailto:…)`, or a `[label]: http…` definition.
fn link(line: &str) -> bool {
    let target = |rest: &str| {
        let rest = rest.trim_start().to_ascii_lowercase();
        rest.starts_with("http://") || rest.starts_with("https://") || rest.starts_with("mailto:")
    };
    if let Some(label) = line.trim_start().strip_prefix('[')
        && let Some(close) = label.find("]:")
        && close > 0
        && target(&label[close + 2..])
    {
        return true;
    }
    let mut from = 0;
    while let Some(at) = line[from..].find("](").map(|i| i + from) {
        if line[..at].contains('[') && target(&line[at + 2..]) {
            return true;
        }
        from = at + 2;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    const README: &str = "\
# mixtool

A small tool for the Mix shell.

## Install

```sh
mix install mixtool
```

## Usage

- `mixtool run` starts it
- `mixtool stop` stops it

See [the manual](https://mixos.dev/mix/) for more.
";

    const LLM_REPLY: &str = "\
Hi Ann,

Here is what I found about the build failure.

## What went wrong

1. The lockfile pins **serde 1.0.229**, but the vendored crate needs 1.0.230.
2. The CI job runs `cargo build --locked`, so it refuses to update.

## The fix

Update the pin and commit the lock:

```
cargo update -p serde
git add Cargo.lock
```

Let me know if that works.
";

    const GITHUB: &str = "\
@octocat commented on this pull request.

> The parser drops the last cell.

Good catch: the loop stops at `len - 1`. I pushed a fix in **a1b2c3d**.

—
Reply to this email directly, view it on GitHub:
https://github.com/markc/mixos/pull/42#discussion_r1
You are receiving this because you were mentioned.
";

    const TABLE: &str = "\
Here are the numbers:

| host | load |
|------|------|
| cbc2 | 0.4  |
| cbc3 | 1.2  |
";

    const PLAIN_REPLY: &str = "\
Sounds good, Thursday works for me.

- Bob

On Tue, 14 Oct 2026 at 09:12, Ann <ann@example.invalid> wrote:
> Can we move the meeting to Thursday?
> - the room is booked on Wednesday
> - **Cy** is away
> # not a heading, a quote
";

    const NEWSLETTER: &str = "\
GOLD COAST CYCLING NEWS
=======================

October rides
-------------

* Saturday social ride, 7am from the Spit
* Sunday hills, 6am from Currumbin

Full calendar: https://example.invalid/rides?utm_source=mail

To unsubscribe visit https://example.invalid/u/123
";

    const SIGNATURE: &str = "\
Thanks, see you then.

--
Ann Example | Head of Things
**Example Pty Ltd** | `+61 7 5555 0000`
https://example.invalid
";

    const FORWARDED: &str = "\
FYI, see below.

---------- Forwarded message ---------
From: Bob <bob@example.invalid>
Date: Mon, 13 Oct 2026 at 10:00
Subject: Re: the_new_plan
To: Ann <ann@example.invalid>

Looks fine to me. The file is my_notes_final.txt in the share.
";

    const ASCII_ART: &str = "\
  +--------+      +--------+
  | client | ---> | server |
  +--------+      +--------+
# # # # # # # # # # # #
# # # # # # # # # # # #
";

    #[test]
    fn markdown_written_text_is_detected() {
        for (name, text) in [
            ("readme", README),
            ("llm reply", LLM_REPLY),
            ("github", GITHUB),
            ("table", TABLE),
        ] {
            assert!(looks_like_markdown(text), "{name}: {:?}", signals(text));
        }
    }

    #[test]
    fn plain_mail_is_not() {
        for (name, text) in [
            ("plain reply", PLAIN_REPLY),
            ("newsletter", NEWSLETTER),
            ("signature", SIGNATURE),
            ("forwarded", FORWARDED),
            ("ascii art", ASCII_ART),
            ("one line", "Sandwiches?"),
            ("a url", "Have a look: https://example.invalid/a_b_c"),
            (
                "one dash",
                "Two things:\n- the car is ready\nand that is all.",
            ),
            ("empty", ""),
        ] {
            assert!(!looks_like_markdown(text), "{name}: {:?}", signals(text));
        }
    }

    #[test]
    fn the_signals_are_read_as_the_table_says() {
        assert!(heading("## Install"));
        assert!(!heading("#hashtag"));
        assert!(!heading("    # indented code"));
        assert!(!heading("# # #"));
        assert!(list_item("  1. first") && list_item("* item") && !list_item("-- sig"));
        assert_eq!(separator("|:---|---:|"), Some(2));
        assert_eq!(separator("+-----+"), None);
        assert!(wrapped("a **b** c", "**", true));
        assert!(!wrapped("snake_case_name", "_", false));
        assert!(wrapped("an _aside_ here", "_", false));
        assert!(!wrapped("2 * 3 * 4", "**", true));
        assert!(inline_code("run `ls` now"));
        assert!(link("see [docs](https://x.invalid)"));
        assert!(link("[1]: https://x.invalid"));
        assert!(!link("[sic](not a url)"));
        // A fence counts only once it closes.
        assert!(!looks_like_markdown("```\nnever closed"));
        assert!(looks_like_markdown("```\nclosed\n```"));
    }
}
