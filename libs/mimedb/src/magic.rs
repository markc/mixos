// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The `magic` file: content rules, one section per type, highest priority
//! first. Format (shared-mime-info spec, "The magic files"):
//!
//! ```text
//! MIME-Magic\0\n
//! [priority:type]\n
//! [indent]>offset=<u16 big-endian length><value>[&<mask>][~word-size][+range]\n
//! ```
//!
//! A rule at indent `n + 1` belongs to the nearest rule above it at indent
//! `n`: a rule matches when its own test does and, if it has children, any
//! child does. A section matches when any top-level rule does.

#[derive(Clone, Debug)]
pub(crate) struct Rule {
    indent: usize,
    offset: usize,
    value: Vec<u8>,
    mask: Option<Vec<u8>>,
    range: usize,
}

#[derive(Clone, Debug)]
pub(crate) struct Section {
    pub priority: u32,
    pub mime: String,
    /// The section held a `__NOMAGIC__` rule: the type's rules from lower
    /// directories are cleared, and only this section's other rules count.
    pub cleared: bool,
    rules: Vec<Rule>,
}

impl Section {
    pub fn has_rules(&self) -> bool {
        !self.rules.is_empty()
    }

    /// The furthest byte any rule can look at.
    pub fn extent(&self) -> usize {
        self.rules
            .iter()
            .map(|r| {
                r.offset
                    .saturating_add(r.range)
                    .saturating_add(r.value.len())
            })
            .max()
            .unwrap_or(0)
    }

    pub fn matches(&self, data: &[u8]) -> bool {
        let mut i = 0;
        while i < self.rules.len() {
            if self.rules[i].indent == 0 && rule_matches(&self.rules, i, data) {
                return true;
            }
            i += 1;
        }
        false
    }
}

/// A rule nested deeper than this never matches: no real rule nests anywhere
/// near it, and a hostile file must not exhaust the stack.
const MAX_DEPTH: usize = 64;

/// Rule `i` and, when it has children, any one of them.
fn rule_matches(rules: &[Rule], i: usize, data: &[u8]) -> bool {
    if rules[i].indent > MAX_DEPTH || !test(&rules[i], data) {
        return false;
    }
    let depth = rules[i].indent;
    let mut j = i + 1;
    let mut has_children = false;
    while j < rules.len() && rules[j].indent > depth {
        if rules[j].indent == depth + 1 {
            has_children = true;
            if rule_matches(rules, j, data) {
                return true;
            }
        }
        j += 1;
    }
    !has_children
}

fn test(rule: &Rule, data: &[u8]) -> bool {
    let len = rule.value.len();
    for start in rule.offset..rule.offset.saturating_add(rule.range) {
        let Some(window) = data.get(start..start.saturating_add(len)) else {
            break;
        };
        let hit = match &rule.mask {
            None => window == rule.value.as_slice(),
            Some(mask) => window
                .iter()
                .zip(&rule.value)
                .zip(mask)
                .all(|((d, v), m)| d & m == v & m),
        };
        if hit {
            return true;
        }
    }
    false
}

/// Parse a whole `magic` file. A line that cannot be read is skipped, as
/// the spec asks, so one unknown extension does not hide the rest; a file
/// cut short ends at the last whole rule.
pub(crate) fn parse(bytes: &[u8]) -> Vec<Section> {
    const HEADER: &[u8] = b"MIME-Magic\0\n";
    let mut sections = Vec::new();
    let Some(mut rest) = bytes.strip_prefix(HEADER) else {
        return sections;
    };
    let mut current: Option<Section> = None;
    while !rest.is_empty() {
        if rest[0] == b'[' {
            let Some(end) = rest.iter().position(|&b| b == b'\n') else {
                break;
            };
            let header = &rest[1..end];
            rest = &rest[end + 1..];
            if let Some(done) = current.take() {
                sections.push(finish(done));
            }
            // A header that is not `[priority:type]` starts a section that
            // collects (and loses) its rules.
            let header = header.strip_suffix(b"]").unwrap_or_default();
            let header = String::from_utf8_lossy(header);
            let (priority, mime) = header.split_once(':').unwrap_or(("", ""));
            let (priority, mime) = match priority.parse() {
                Ok(p) => (p, mime),
                Err(_) => (0, ""),
            };
            current = Some(Section {
                priority,
                mime: mime.to_owned(),
                cleared: false,
                rules: Vec::new(),
            });
            continue;
        }
        match parse_rule(rest) {
            Parsed::Rule(rule, next) => {
                if let Some(section) = current.as_mut()
                    && !section.mime.is_empty()
                {
                    section.rules.push(rule);
                }
                rest = next;
            }
            Parsed::Skip(next) => rest = next,
            Parsed::End => break,
        }
    }
    if let Some(done) = current.take() {
        sections.push(finish(done));
    }
    sections
}

/// A rule whose value is `__NOMAGIC__` marks the section as clearing the
/// type's rules from lower directories; it is not itself a test.
fn finish(mut section: Section) -> Section {
    let before = section.rules.len();
    section.rules.retain(|r| r.value != b"__NOMAGIC__");
    section.cleared = section.rules.len() != before;
    section
}

enum Parsed<'a> {
    Rule(Rule, &'a [u8]),
    /// The line could not be read: carry on after its newline.
    Skip(&'a [u8]),
    /// The file ends inside a rule.
    End,
}

/// Everything after the next newline in `bytes`.
fn skip_line(bytes: &[u8]) -> Parsed<'_> {
    match bytes.iter().position(|&b| b == b'\n') {
        Some(at) => Parsed::Skip(&bytes[at + 1..]),
        None => Parsed::End,
    }
}

/// A decimal number at the start of `bytes`: `Some(None)` when there are
/// no digits, `None` when the digits overflow, and the bytes after it.
fn number(bytes: &[u8]) -> (Option<Option<usize>>, &[u8]) {
    let digits = bytes.iter().take_while(|b| b.is_ascii_digit()).count();
    let value = if digits == 0 {
        Some(None)
    } else {
        std::str::from_utf8(&bytes[..digits])
            .ok()
            .and_then(|s| s.parse().ok())
            .map(Some)
    };
    (value, &bytes[digits..])
}

/// A number that must be there and must fit.
fn required(bytes: &[u8]) -> Option<(usize, &[u8])> {
    match number(bytes) {
        (Some(Some(n)), rest) => Some((n, rest)),
        _ => None,
    }
}

fn parse_rule(bytes: &[u8]) -> Parsed<'_> {
    // The indent may be left out; the offset may not; neither may overflow.
    let (Some(indent), rest) = number(bytes) else {
        return skip_line(bytes);
    };
    let indent = indent.unwrap_or(0);
    let Some(rest) = rest.strip_prefix(b">") else {
        return skip_line(bytes);
    };
    let Some((offset, rest)) = required(rest) else {
        return skip_line(bytes);
    };
    let Some(rest) = rest.strip_prefix(b"=") else {
        return skip_line(bytes);
    };
    if rest.len() < 2 {
        return Parsed::End;
    }
    let len = u16::from_be_bytes([rest[0], rest[1]]) as usize;
    let rest = &rest[2..];
    let Some(value) = rest.get(..len) else {
        return Parsed::End;
    };
    let mut value = value.to_vec();
    let mut rest = &rest[len..];
    let mut mask = None;
    if let Some(after) = rest.strip_prefix(b"&") {
        let Some(m) = after.get(..len) else {
            return Parsed::End;
        };
        mask = Some(m.to_vec());
        rest = &after[len..];
    }
    let mut word_size = 1;
    if let Some(after) = rest.strip_prefix(b"~") {
        let Some((w, after)) = required(after) else {
            return skip_line(after);
        };
        // GLib accepts 0, 1, 2 and 4 (0 and 1 mean no swap); anything else
        // drops the rule.
        if !matches!(w, 0 | 1 | 2 | 4) {
            return skip_line(after);
        }
        word_size = w.max(1);
        rest = after;
    }
    let mut range = 1;
    if let Some(after) = rest.strip_prefix(b"+") {
        let Some((r, after)) = required(after) else {
            return skip_line(after);
        };
        // An explicit range of zero compares nothing, as in GLib.
        range = r;
        rest = after;
    }
    let Some(rest) = rest.strip_prefix(b"\n") else {
        // An extension this reader does not know: drop the rule, keep going.
        return skip_line(rest);
    };
    // A value that is not whole words cannot be swapped: drop the rule.
    if !value.len().is_multiple_of(word_size) {
        return Parsed::Skip(rest);
    }
    // Values wider than a byte are written big-endian; compare them in the
    // host's order.
    if word_size > 1 && cfg!(target_endian = "little") {
        swap(&mut value, word_size);
        if let Some(mask) = mask.as_mut() {
            swap(mask, word_size);
        }
    }
    Parsed::Rule(
        Rule {
            indent,
            offset,
            value,
            mask,
            range,
        },
        rest,
    )
}

fn swap(bytes: &mut [u8], word: usize) {
    for chunk in bytes.chunks_mut(word) {
        if chunk.len() == word {
            chunk.reverse();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(indent: usize, offset: usize, value: &[u8], extra: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        if indent > 0 {
            out.extend(indent.to_string().bytes());
        }
        out.extend(format!(">{offset}=").bytes());
        out.extend((value.len() as u16).to_be_bytes());
        out.extend(value);
        out.extend(extra);
        out.push(b'\n');
        out
    }

    fn file(body: &[u8]) -> Vec<u8> {
        let mut out = b"MIME-Magic\0\n".to_vec();
        out.extend(body);
        out
    }

    #[test]
    fn plain_masked_ranged_and_nested() {
        let mut body = b"[60:image/x-a]\n".to_vec();
        body.extend(rule(0, 0, b"AB", b""));
        body.extend(b"[50:image/x-b]\n");
        body.extend(rule(0, 2, b"\x0f", b"&\x0f"));
        body.extend(b"[40:image/x-c]\n");
        body.extend(rule(0, 0, b"ZZ", b"+4"));
        body.extend(b"[30:image/x-d]\n");
        body.extend(rule(0, 0, b"P", b""));
        body.extend(rule(1, 1, b"Q", b""));
        body.extend(rule(1, 1, b"R", b""));
        let sections = parse(&file(&body));
        assert_eq!(sections.len(), 4);
        assert!(sections[0].matches(b"ABC"));
        assert!(!sections[0].matches(b"A"));
        assert!(sections[1].matches(b"xx\xffy"));
        assert!(!sections[1].matches(b"xx\xf0y"));
        assert!(sections[2].matches(b"...ZZ"));
        assert!(!sections[2].matches(b".....ZZ"));
        assert!(sections[3].matches(b"PR"));
        assert!(!sections[3].matches(b"PX"));
        assert_eq!(sections[2].extent(), 6);
    }

    #[test]
    fn a_value_that_is_not_whole_words_is_dropped() {
        let mut body = b"[50:image/x-w]\n".to_vec();
        body.extend(rule(0, 0, b"\x12\x34\x56", b"~2"));
        body.extend(rule(0, 0, b"Z", b""));
        let sections = parse(&file(&body));
        assert!(!sections[0].matches(b"\x34\x12\x56"));
        assert!(sections[0].matches(b"Z"));
    }

    #[test]
    fn word_size_compares_in_host_order() {
        let mut body = b"[50:image/x-w]\n".to_vec();
        body.extend(rule(0, 0, b"\x12\x34", b"~2"));
        let sections = parse(&file(&body));
        let host = 0x1234u16.to_ne_bytes();
        assert!(sections[0].matches(&host));
    }

    #[test]
    fn nomagic_recovery_and_truncation() {
        // __NOMAGIC__ is a rule value; the section keeps its other rules.
        let mut body = b"[50:image/x-a]\n".to_vec();
        body.extend(rule(0, 0, b"__NOMAGIC__", b""));
        body.extend(rule(0, 0, b"A", b""));
        // An unknown extension drops its rule and nothing after it.
        body.extend(b"[50:image/x-b]\n");
        body.extend(rule(0, 0, b"X", b"!future"));
        body.extend(rule(0, 0, b"B", b""));
        body.extend(b"[50:image/x-c]\n");
        body.extend(rule(0, 0, b"C", b""));
        body.extend(b">0=");
        let sections = parse(&file(&body));
        assert_eq!(sections.len(), 3);
        assert!(sections[0].cleared);
        assert!(sections[0].matches(b"A"));
        assert!(!sections[1].matches(b"X"));
        assert!(sections[1].matches(b"B"));
        assert!(sections[2].matches(b"C"));
        assert!(parse(b"not magic").is_empty());
    }

    #[test]
    fn unsupported_word_sizes_drop_the_rule_and_a_zero_range_compares_nothing() {
        let mut body = b"[50:image/x-w]\n".to_vec();
        body.extend(rule(0, 0, b"ABC", b"~3"));
        body.extend(rule(0, 0, b"Z", b"+0"));
        body.extend(rule(0, 0, b"Y", b""));
        body.extend(rule(0, 0, b"W", b"~0"));
        let sections = parse(&file(&body));
        assert!(sections[0].matches(b"W"));
        assert!(!sections[0].matches(b"ABC"));
        assert!(!sections[0].matches(b"CBA"));
        assert!(!sections[0].matches(b"Z"));
        assert!(sections[0].matches(b"Y"));
    }

    #[test]
    fn missing_or_overflowing_numbers_drop_the_rule() {
        let mut body = b"[50:image/x-n]\n".to_vec();
        body.extend(b">18446744073709551616=\0\x01A\n");
        body.extend(b">=\0\x01A\n");
        body.extend(b"99999999999999999999>0=\0\x01A\n");
        body.extend(rule(0, 0, b"B", b"~"));
        body.extend(rule(0, 0, b"B", b"+"));
        body.extend(rule(0, 0, b"C", b""));
        let sections = parse(&file(&body));
        assert!(!sections[0].matches(b"A"));
        assert!(!sections[0].matches(b"B"));
        assert!(sections[0].matches(b"C"));
    }

    #[test]
    fn hostile_offsets_and_nesting_are_bounded() {
        // The largest offset and range neither overflow nor match.
        let mut body = b"[50:image/x-far]\n".to_vec();
        body.extend(rule(0, usize::MAX, b"X", b"+99999999999"));
        let sections = parse(&file(&body));
        assert!(!sections[0].matches(b"XXXX"));
        assert_eq!(sections[0].extent(), usize::MAX);

        // A hundred thousand nested rules end at the depth cap, not the stack.
        let mut body = b"[50:image/x-deep]\n".to_vec();
        for depth in 0..100_000 {
            body.extend(rule(depth, 0, b"D", b""));
        }
        let sections = parse(&file(&body));
        assert!(!sections[0].matches(b"D"));
    }
}
