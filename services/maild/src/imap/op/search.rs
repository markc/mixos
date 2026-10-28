// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>

//! `SEARCH` / `UID SEARCH` — RFC 9051 §6.4.4 / §6.4.8.
//!
//! Scans the selected mailbox and emits `* SEARCH <msn|uid>...` for
//! every message that satisfies the criteria expression. `UID SEARCH`
//! returns UIDs; bare `SEARCH` returns message sequence numbers.
//!
//! ## scope
//!
//! Criteria implemented in this task:
//!
//! - `ALL`
//! - System flag predicates: `ANSWERED`, `UNANSWERED`, `DELETED`,
//!   `UNDELETED`, `DRAFT`, `UNDRAFT`, `FLAGGED`, `UNFLAGGED`, `SEEN`,
//!   `UNSEEN`
//! - `KEYWORD <name>`, `UNKEYWORD <name>` — user keywords, normalised to
//!   NFC + lowercase (the same fold STORE applies on write) and matched
//!   against per-membership tags. Per RFC 9051 §9 these take
//!   `flag-keyword`, a bare atom, so a `\`-prefixed argument is a syntax
//!   error rather than a system-flag search — unlike STORE, whose
//!   `flag-list` takes the wider `flag` production. Use the dedicated
//!   system keys (`SEEN`, `UNFLAGGED`, …) for the bits.
//! - `NEW` — always empty. RFC 9051 §6.4.4 defines `NEW = RECENT
//!   UNSEEN`; we don't track `\Recent` (no message carries it), so
//!   `RECENT` is always empty and `NEW` therefore must be too —
//!   collapsing `NEW` to plain `UNSEEN` would give clients false
//!   positives for any unread old mail.
//! - `OLD` — `NOT RECENT`. Since no message carries `\Recent`,
//!   every message is "old" → equivalent to `ALL`.
//! - `RECENT` — always empty (we don't track `\Recent`).
//! - `SMALLER <n>`, `LARGER <n>` — compares `size_bytes` to the literal.
//! - `UID <seq-set>` — restrict to UIDs in the given set.
//! - Bare `<seq-set>` — restrict to MSNs in the given set.
//! - `NOT <key>`, `OR <key1> <key2>`, `(<key1> <key2>...)` —
//!   the standard combinators.
//!
//! - `BEFORE`, `ON`, `SINCE <date>` — the internal date
//!   (`received_at`, UTC day) against a `d-Mon-yyyy` date.
//! - `SENTBEFORE`, `SENTON`, `SENTSINCE <date>` — the day of the
//!   `Date:` header as the sender wrote it (time and zone ignored, RFC
//!   9051 §6.4.4). A message with no parseable `Date:` falls back to
//!   the internal date, as Dovecot does.
//! - `FROM`, `TO`, `CC`, `BCC`, `SUBJECT <string>`, `HEADER <name>
//!   <string>` — case-insensitive substring of the named field. The
//!   needle is tried against both the raw field and its RFC 2047
//!   decoded form, so a non-ASCII search finds an encoded-word subject.
//!   `HEADER <name> ""` matches any message that has the field.
//! - `BODY <string>` — substring of the decoded text body parts (HTML
//!   parts are reduced to text first). `TEXT <string>` — header block
//!   or body.
//!
//! Multiple top-level keys are AND-combined (RFC 9051 §6.4.4
//! "combining multiple search-keys with implicit AND").
//!
//! String arguments arrive as atoms, quoted strings, or — for
//! non-ASCII text, which Thunderbird sends as `{N}` — a single
//! trailing literal that the session splices back into the argument
//! line as a quoted string (see `session::splice_search_literal`).
//!
//! ## Substrate primitive
//!
//! `list_emails_in_mailbox` returns the full snapshot — `EmailHandle`
//! carries `uid`, `keywords`, `size_bytes`, `received_at`. Each
//! message is first judged on that metadata alone with three-valued
//! logic ([`eval`] returns `None` when a header, sent-date or body key
//! is still undecided); only the undecided ones pay for a blob read
//! and a `mail_parser` parse. So `UNDELETED FROM x` on a mailbox of
//! mostly deleted mail never loads the deleted blobs.
//!
//! ## Response shape
//!
//! ```text
//! * SEARCH 1 3 5 7
//! tag OK SEARCH completed
//! ```
//!
//! Empty match emits `* SEARCH\r\n` (no trailing space) per RFC 9051
//! §7.3.4.

use std::sync::Arc;

use chrono::NaiveDate;
use mds::{ContainerId, Flags};

use super::flags::{self, FlagToken};
use crate::imap::response::{Status, tagged};
use crate::imap::seq::SeqSet;
use crate::mailstore::{EmailHandle, ListOpts, MailStore, SortKey};

type AccountId = i32;

/// Whether the response emits message sequence numbers (`SEARCH`) or
/// UIDs (`UID SEARCH`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchMode {
    Msn,
    Uid,
}

/// Criteria AST. `And` is the implicit combinator for a top-level
/// list of keys; `Or` and `Not` are the explicit RFC combinators.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Crit {
    /// Always true.
    All,
    /// Always false. Produced by `RECENT` (we never carry `\Recent`)
    /// and `NEW` (RFC 9051 §6.4.4 `NEW = RECENT UNSEEN`, which is
    /// empty whenever `RECENT` is empty).
    None,
    /// All named bits in `flags` must be set on the message.
    FlagsSet(Flags),
    /// None of the named bits in `flags` may be set on the message.
    FlagsClear(Flags),
    /// The normalised user keyword must be present in the message's tags.
    TagSet(String),
    /// `size_bytes < limit`.
    Smaller(u64),
    /// `size_bytes > limit`.
    Larger(u64),
    /// `msn` is a member of `set`.
    MsnInSet(SeqSet),
    /// `uid` is a member of `set`.
    UidInSet(SeqSet),
    /// Internal date (UTC day) is before / on / on-or-after the day.
    Before(NaiveDate),
    On(NaiveDate),
    Since(NaiveDate),
    /// `Date:` header day is before / on / on-or-after the day.
    SentBefore(NaiveDate),
    SentOn(NaiveDate),
    SentSince(NaiveDate),
    /// Lowercased field name, lowercased needle. An empty needle
    /// matches any message carrying the field.
    Header(String, String),
    /// Lowercased needle against the decoded text body.
    Body(String),
    /// Lowercased needle against the header block or the body.
    Text(String),
    /// All sub-keys must match.
    And(Vec<Crit>),
    /// Either sub-key matches.
    Or(Box<Crit>, Box<Crit>),
    /// The sub-key must not match.
    Not(Box<Crit>),
}

/// Parsed `SEARCH` / `UID SEARCH` request — the criteria expression
/// plus whether the response emits MSNs or UIDs.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SearchReq {
    crit: Crit,
    mode: SearchMode,
}

/// Parse the argument portion of `SEARCH` / `UID SEARCH`.
fn parse_args(args: &str, mode: SearchMode) -> Result<SearchReq, String> {
    let trimmed = args.trim();
    if trimmed.is_empty() {
        return Err("SEARCH requires at least one criterion".into());
    }
    let tokens = tokenize(trimmed)?;
    let mut idx = 0;
    if idx + 1 < tokens.len() && tokens[idx].eq_ignore_ascii_case("CHARSET") {
        idx += 2;
    }
    if idx < tokens.len() && tokens[idx].eq_ignore_ascii_case("RETURN") {
        return Err("SEARCH RETURN (ESEARCH) not implemented in this phase".into());
    }
    let mut keys: Vec<Crit> = Vec::new();
    while idx < tokens.len() {
        let (k, next) = parse_key(&tokens, idx)?;
        keys.push(k);
        idx = next;
    }
    if keys.is_empty() {
        return Err("SEARCH requires at least one criterion".into());
    }
    let crit = if keys.len() == 1 {
        keys.into_iter().next().unwrap()
    } else {
        Crit::And(keys)
    };
    Ok(SearchReq { crit, mode })
}

/// Pre-validate `args` so grammar failures can bump the abuse-cap
/// counter at the dispatch site.
pub fn parse_args_for_dispatch(args: &str, mode: SearchMode) -> Result<(), String> {
    parse_args(args, mode).map(|_| ())
}

fn tokenize(s: &str) -> Result<Vec<String>, String> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            ' ' | '\t' => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            '(' | ')' => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
                out.push(c.to_string());
            }
            '"' => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
                let mut q = String::new();
                let mut closed = false;
                while let Some(quoted) = chars.next() {
                    match quoted {
                        '"' => {
                            closed = true;
                            break;
                        }
                        '\\' => match chars.next() {
                            Some(escaped) => q.push(escaped),
                            None => break,
                        },
                        _ => q.push(quoted),
                    }
                }
                if !closed {
                    return Err("SEARCH unterminated quoted string".into());
                }
                out.push(q);
            }
            _ => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    Ok(out)
}

fn parse_key(tokens: &[String], idx: usize) -> Result<(Crit, usize), String> {
    if idx >= tokens.len() {
        return Err("SEARCH unexpected end of expression".into());
    }
    let tok = &tokens[idx];
    let upper = tok.to_ascii_uppercase();
    match upper.as_str() {
        "(" => {
            let mut keys: Vec<Crit> = Vec::new();
            let mut cur = idx + 1;
            loop {
                if cur >= tokens.len() {
                    return Err("SEARCH missing closing paren".into());
                }
                if tokens[cur] == ")" {
                    if keys.is_empty() {
                        return Err("SEARCH empty parenthesised list".into());
                    }
                    let combined = if keys.len() == 1 {
                        keys.into_iter().next().unwrap()
                    } else {
                        Crit::And(keys)
                    };
                    return Ok((combined, cur + 1));
                }
                let (k, next) = parse_key(tokens, cur)?;
                keys.push(k);
                cur = next;
            }
        }
        ")" => Err("SEARCH stray closing paren".into()),
        "ALL" => Ok((Crit::All, idx + 1)),
        "ANSWERED" => Ok((Crit::FlagsSet(Flags(Flags::ANSWERED)), idx + 1)),
        "UNANSWERED" => Ok((Crit::FlagsClear(Flags(Flags::ANSWERED)), idx + 1)),
        "DELETED" => Ok((Crit::FlagsSet(Flags(Flags::DELETED)), idx + 1)),
        "UNDELETED" => Ok((Crit::FlagsClear(Flags(Flags::DELETED)), idx + 1)),
        "DRAFT" => Ok((Crit::FlagsSet(Flags(Flags::DRAFT)), idx + 1)),
        "UNDRAFT" => Ok((Crit::FlagsClear(Flags(Flags::DRAFT)), idx + 1)),
        "FLAGGED" => Ok((Crit::FlagsSet(Flags(Flags::FLAGGED)), idx + 1)),
        "UNFLAGGED" => Ok((Crit::FlagsClear(Flags(Flags::FLAGGED)), idx + 1)),
        "SEEN" => Ok((Crit::FlagsSet(Flags(Flags::SEEN)), idx + 1)),
        "UNSEEN" => Ok((Crit::FlagsClear(Flags(Flags::SEEN)), idx + 1)),
        // `NEW = RECENT UNSEEN`; with no `\Recent` tracking, RECENT
        // is always empty so NEW must also be empty. Mapping NEW to
        // plain UNSEEN would emit false positives for unread old
        // mail, divergent from a client's expected semantics.
        "NEW" => Ok((Crit::None, idx + 1)),
        "OLD" => Ok((Crit::All, idx + 1)),
        "RECENT" => Ok((Crit::None, idx + 1)),
        "KEYWORD" => {
            let name = tokens
                .get(idx + 1)
                .ok_or_else(|| "SEARCH KEYWORD missing argument".to_string())?;
            Ok((parse_keyword(name, false)?, idx + 2))
        }
        "UNKEYWORD" => {
            let name = tokens
                .get(idx + 1)
                .ok_or_else(|| "SEARCH UNKEYWORD missing argument".to_string())?;
            Ok((parse_keyword(name, true)?, idx + 2))
        }
        "SMALLER" => {
            let n = parse_number(tokens, idx + 1, "SMALLER")?;
            Ok((Crit::Smaller(n), idx + 2))
        }
        "LARGER" => {
            let n = parse_number(tokens, idx + 1, "LARGER")?;
            Ok((Crit::Larger(n), idx + 2))
        }
        "UID" => {
            let set_tok = tokens
                .get(idx + 1)
                .ok_or_else(|| "SEARCH UID missing sequence-set".to_string())?;
            let set = SeqSet::parse(set_tok)
                .map_err(|e| format!("SEARCH UID invalid sequence-set: {e}"))?;
            Ok((Crit::UidInSet(set), idx + 2))
        }
        "NOT" => {
            let (k, next) = parse_key(tokens, idx + 1)?;
            Ok((Crit::Not(Box::new(k)), next))
        }
        "OR" => {
            let (k1, n1) = parse_key(tokens, idx + 1)?;
            let (k2, n2) = parse_key(tokens, n1)?;
            Ok((Crit::Or(Box::new(k1), Box::new(k2)), n2))
        }
        "BEFORE" | "ON" | "SINCE" | "SENTBEFORE" | "SENTON" | "SENTSINCE" => {
            let arg = tokens
                .get(idx + 1)
                .ok_or_else(|| format!("SEARCH {upper} missing date"))?;
            let day = parse_date(arg).ok_or_else(|| {
                format!("SEARCH {upper} expected a date like 1-Feb-2026, got {arg:?}")
            })?;
            let crit = match upper.as_str() {
                "BEFORE" => Crit::Before(day),
                "ON" => Crit::On(day),
                "SINCE" => Crit::Since(day),
                "SENTBEFORE" => Crit::SentBefore(day),
                "SENTON" => Crit::SentOn(day),
                _ => Crit::SentSince(day),
            };
            Ok((crit, idx + 2))
        }
        "FROM" | "TO" | "CC" | "BCC" | "SUBJECT" => {
            let needle = string_arg(tokens, idx + 1, &upper)?;
            Ok((Crit::Header(upper.to_ascii_lowercase(), needle), idx + 2))
        }
        "HEADER" => {
            let name = tokens
                .get(idx + 1)
                .ok_or_else(|| "SEARCH HEADER missing field name".to_string())?;
            if name.is_empty() {
                return Err("SEARCH HEADER empty field name".into());
            }
            let needle = string_arg(tokens, idx + 2, "HEADER")?;
            Ok((Crit::Header(name.to_ascii_lowercase(), needle), idx + 3))
        }
        "BODY" => Ok((Crit::Body(string_arg(tokens, idx + 1, "BODY")?), idx + 2)),
        "TEXT" => Ok((Crit::Text(string_arg(tokens, idx + 1, "TEXT")?), idx + 2)),
        _ => match SeqSet::parse(tok) {
            Ok(set) => Ok((Crit::MsnInSet(set), idx + 1)),
            Err(_) => Err(format!("SEARCH unknown criterion {tok:?}")),
        },
    }
}

fn parse_number(tokens: &[String], idx: usize, key: &str) -> Result<u64, String> {
    let tok = tokens
        .get(idx)
        .ok_or_else(|| format!("SEARCH {key} missing number"))?;
    tok.parse::<u64>()
        .map_err(|_| format!("SEARCH {key} expected number, got {tok:?}"))
}

/// A search string argument, lowercased for the case-insensitive
/// substring match RFC 9051 §6.4.4 asks for.
fn string_arg(tokens: &[String], idx: usize, key: &str) -> Result<String, String> {
    tokens
        .get(idx)
        .map(|s| s.to_lowercase())
        .ok_or_else(|| format!("SEARCH {key} missing string"))
}

/// Parse an RFC 9051 `date` (`date-day "-" date-month "-" date-year`,
/// e.g. `1-Feb-2026` or `01-feb-2026`; quotes already stripped).
fn parse_date(tok: &str) -> Option<NaiveDate> {
    let mut parts = tok.split('-');
    let day: u32 = parts.next()?.parse().ok()?;
    let month_tok = parts.next()?;
    let year: i32 = parts.next()?.parse().ok()?;
    if parts.next().is_some() || month_tok.len() != 3 {
        return None;
    }
    const MONTHS: [&str; 12] = [
        "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
    ];
    let month = MONTHS
        .iter()
        .position(|m| m.eq_ignore_ascii_case(month_tok))? as u32
        + 1;
    NaiveDate::from_ymd_opt(year, month, day)
}

/// Parse the argument of `KEYWORD` / `UNKEYWORD`.
///
/// RFC 9051 §9 spells these `"KEYWORD" SP flag-keyword` and `"UNKEYWORD" SP
/// flag-keyword`, and `flag-keyword = "$MDNSent" / "$Forwarded" / "$Junk" /
/// "$NotJunk" / "$Phishing" / atom` — a bare atom that **cannot** begin with
/// `\`. That is deliberately narrower than STORE, whose `flag-list` takes the
/// full `flag` production (system `\`-atoms *and* keywords). So SEARCH must
/// NOT reuse STORE's [`flags::parse_flag_token`] discriminator wholesale: a
/// `\`-prefixed argument here is a syntax error, not a system-flag search.
///
/// Nothing is lost by rejecting it — every system flag has a dedicated search
/// key in both polarities (`SEEN`/`UNSEEN`, `FLAGGED`/`UNFLAGGED`,
/// `ANSWERED`/`UNANSWERED`, `DELETED`/`UNDELETED`, `DRAFT`/`UNDRAFT`), which is
/// precisely why the grammar reserves `flag-keyword` for keywords.
///
/// (§6.4.4's prose renders the placeholder as `KEYWORD <flag>`, an editorial
/// carry-over from RFC 3501 that contradicts its own §9 ABNF. The formal
/// syntax governs; the prose gloss "the specified **keyword** flag" agrees
/// with it in substance.)
///
/// The normaliser is still shared with STORE — the same NFC + lowercase fold
/// on both the write and read paths is what makes a stored tag findable.
fn parse_keyword(tok: &str, negated: bool) -> Result<Crit, String> {
    if tok.starts_with('\\') {
        return Err(format!(
            "SEARCH KEYWORD/UNKEYWORD takes a keyword, not a system flag, got {tok:?} \
             (RFC 9051 §9 flag-keyword) — use SEEN/UNSEEN, FLAGGED/UNFLAGGED, \
             ANSWERED/UNANSWERED, DELETED/UNDELETED or DRAFT/UNDRAFT"
        ));
    }
    let parsed =
        flags::parse_flag_token(tok).map_err(|e| format!("SEARCH invalid keyword {tok:?}: {e}"))?;
    Ok(match (parsed, negated) {
        // Unreachable: `parse_flag_token` only yields `System` for a
        // `\`-prefixed token, and those are rejected above. Kept total rather
        // than `unreachable!()` so a future change to the discriminator can
        // never turn a parse into a panic on a client-supplied string.
        (FlagToken::System(bit), false) => Crit::FlagsSet(Flags(bit)),
        (FlagToken::System(bit), true) => Crit::FlagsClear(Flags(bit)),
        (FlagToken::User(name), false) => Crit::TagSet(name),
        (FlagToken::User(name), true) => Crit::Not(Box::new(Crit::TagSet(name))),
    })
}

/// The parts of a message that header, sent-date and body keys look
/// at, extracted once per message and lowercased for matching.
struct Doc {
    /// `(lowercased name, lowercased raw value, lowercased decoded value)`.
    headers: Vec<(String, String, String)>,
    /// Decoded text body parts, lowercased.
    body: String,
    /// Day of the `Date:` header, if it parsed.
    sent: Option<NaiveDate>,
}

impl Doc {
    fn from_raw(raw: &[u8]) -> Doc {
        let Some(message) = mail_parser::MessageParser::default().parse(raw) else {
            // Unparseable: no fields to match, body is the lossy bytes.
            return Doc {
                headers: Vec::new(),
                body: String::from_utf8_lossy(raw).to_lowercase(),
                sent: None,
            };
        };
        let headers = message
            .headers()
            .iter()
            .map(|h| {
                let raw_value = raw
                    .get(h.offset_start as usize..h.offset_end as usize)
                    .map(String::from_utf8_lossy)
                    .unwrap_or_default();
                (
                    h.name().to_ascii_lowercase(),
                    raw_value.to_lowercase(),
                    decoded_value(h.value()).to_lowercase(),
                )
            })
            .collect();
        let mut body = String::new();
        let mut pos = 0;
        while let Some(text) = message.body_text(pos) {
            if !body.is_empty() {
                body.push('\n');
            }
            body.push_str(&text.to_lowercase());
            pos += 1;
        }
        let sent = message
            .date()
            .and_then(|d| NaiveDate::from_ymd_opt(i32::from(d.year), d.month.into(), d.day.into()));
        Doc {
            headers,
            body,
            sent,
        }
    }

    fn header_has(&self, name: &str, needle: &str) -> bool {
        self.headers.iter().any(|(n, raw, decoded)| {
            n == name && (needle.is_empty() || raw.contains(needle) || decoded.contains(needle))
        })
    }

    fn text_has(&self, needle: &str) -> bool {
        self.body.contains(needle)
            || self.headers.iter().any(|(n, raw, decoded)| {
                n.contains(needle) || raw.contains(needle) || decoded.contains(needle)
            })
    }
}

/// Render a parsed header value as plain text for matching: addresses
/// as `Name <addr>`, text and text lists as written after RFC 2047
/// decoding. Other kinds (dates, content types) match on the raw value.
fn decoded_value(value: &mail_parser::HeaderValue<'_>) -> String {
    use mail_parser::HeaderValue;
    match value {
        HeaderValue::Address(addr) => addr
            .iter()
            .map(|a| match (a.name(), a.address()) {
                (Some(n), Some(e)) => format!("{n} <{e}>"),
                (Some(n), None) => n.to_string(),
                (None, Some(e)) => e.to_string(),
                (None, None) => String::new(),
            })
            .collect::<Vec<_>>()
            .join(", "),
        HeaderValue::Text(t) => t.to_string(),
        HeaderValue::TextList(list) => list.join(" "),
        _ => String::new(),
    }
}

/// UTC day of a unix-millis internal date.
fn internal_day(received_at_ms: i64) -> NaiveDate {
    chrono::DateTime::from_timestamp_millis(received_at_ms)
        .unwrap_or_default()
        .date_naive()
}

/// Evaluate a criterion against a single message, three-valued.
/// `exists` and `largest_uid` are the mailbox-wide bounds needed to
/// resolve `*` inside `MsnInSet` / `UidInSet` per RFC 9051 §9.
///
/// With `doc` = `None` only metadata is available, and a key that
/// needs the message content returns `None` (undecided) unless the
/// combinators already settle the answer (`false AND ?` = `false`,
/// `true OR ?` = `true`). With `doc` = `Some`, the result is always
/// `Some`.
fn eval(
    crit: &Crit,
    msn: u32,
    handle: &EmailHandle,
    exists: u32,
    largest_uid: u64,
    doc: Option<&Doc>,
) -> Option<bool> {
    let sent_day = |d: &Doc| d.sent.unwrap_or_else(|| internal_day(handle.received_at));
    Some(match crit {
        Crit::All => true,
        Crit::None => false,
        Crit::FlagsSet(f) => (handle.keywords.0 & f.0) == f.0,
        Crit::FlagsClear(f) => (handle.keywords.0 & f.0) == 0,
        Crit::TagSet(name) => handle.tags.contains(name),
        Crit::Smaller(n) => handle.size_bytes < *n,
        Crit::Larger(n) => handle.size_bytes > *n,
        Crit::MsnInSet(set) => set.contains_msn(msn, exists),
        Crit::UidInSet(set) => set.contains_uid(handle.uid, largest_uid),
        Crit::Before(day) => internal_day(handle.received_at) < *day,
        Crit::On(day) => internal_day(handle.received_at) == *day,
        Crit::Since(day) => internal_day(handle.received_at) >= *day,
        Crit::SentBefore(day) => sent_day(doc?) < *day,
        Crit::SentOn(day) => sent_day(doc?) == *day,
        Crit::SentSince(day) => sent_day(doc?) >= *day,
        Crit::Header(name, needle) => doc?.header_has(name, needle),
        Crit::Body(needle) => doc?.body.contains(needle.as_str()),
        Crit::Text(needle) => doc?.text_has(needle),
        Crit::And(ks) => {
            let mut undecided = false;
            for k in ks {
                match eval(k, msn, handle, exists, largest_uid, doc) {
                    Some(false) => return Some(false),
                    Some(true) => {}
                    None => undecided = true,
                }
            }
            if undecided {
                return None;
            }
            true
        }
        Crit::Or(a, b) => match (
            eval(a, msn, handle, exists, largest_uid, doc),
            eval(b, msn, handle, exists, largest_uid, doc),
        ) {
            (Some(true), _) | (_, Some(true)) => true,
            (Some(false), Some(false)) => false,
            _ => return None,
        },
        Crit::Not(k) => !eval(k, msn, handle, exists, largest_uid, doc)?,
    })
}

/// Judge every message in the snapshot, loading and parsing a blob
/// only for messages the metadata alone leaves undecided. Runs on a
/// blocking thread: blob reads are synchronous filesystem work.
fn collect_hits(
    crit: &Crit,
    handles: &[EmailHandle],
    store: &dyn MailStore,
) -> Vec<(u32, u64)> {
    let exists = u32::try_from(handles.len()).unwrap_or(u32::MAX);
    let largest_uid = handles.last().map(|h| h.uid).unwrap_or(0);
    let mut hits: Vec<(u32, u64)> = Vec::with_capacity(handles.len());
    for (idx, handle) in handles.iter().enumerate() {
        let msn = (idx + 1) as u32;
        let hit = match eval(crit, msn, handle, exists, largest_uid, None) {
            Some(decided) => decided,
            None => match store.read_blob(handle.blob_hash) {
                Ok(raw) => {
                    let doc = Doc::from_raw(&raw);
                    eval(crit, msn, handle, exists, largest_uid, Some(&doc)).unwrap_or(false)
                }
                Err(e) => {
                    // A missing blob cannot match a content key; skip
                    // the row rather than fail the whole SEARCH.
                    tracing::warn!(
                        error = %e,
                        item_id = %handle.id.0,
                        "SEARCH read_blob failed; treating message as non-matching"
                    );
                    false
                }
            },
        };
        if hit {
            hits.push((msn, handle.uid));
        }
    }
    hits
}

pub async fn handle(
    tag: &str,
    args: &str,
    account_id: AccountId,
    container: ContainerId,
    store: Arc<dyn MailStore>,
    mode: SearchMode,
) -> String {
    let req = match parse_args(args, mode) {
        Ok(r) => r,
        Err(e) => return tagged(tag, Status::Bad, None, &e),
    };

    let opts = ListOpts {
        sort_by: SortKey::Seq,
        limit: u32::MAX,
        offset: 0,
    };
    let store_for_list = store.clone();
    let mut handles = match tokio::task::spawn_blocking(move || {
        store_for_list.list_emails_in_mailbox(account_id, container, opts)
    })
    .await
    {
        Ok(Ok(h)) => h,
        Ok(Err(e)) => {
            tracing::warn!(error = %e, "SEARCH list_emails_in_mailbox failed");
            return tagged(tag, Status::No, Some("SERVERBUG"), "search failed");
        }
        Err(e) => {
            tracing::warn!(error = %e, "SEARCH spawn_blocking panicked");
            return tagged(tag, Status::No, Some("SERVERBUG"), "search failed");
        }
    };

    // SortKey::Seq returns ascending-by-seq, which equals
    // ascending-by-UID for mixos mailstore membership (Seq is the
    // u32 width of UID). Defensive sort + dedup here keeps MSN
    // assignment correct even if the storage backend ever returns
    // unsorted rows; matches the invariants the `SequenceMap`
    // constructor enforces (RFC 9051 §2.3.1 — UIDs strictly increase
    // by message position in a mailbox snapshot).
    handles.sort_by_key(|h| h.uid);
    handles.dedup_by_key(|h| h.uid);

    let crit = req.crit.clone();
    let hits = match tokio::task::spawn_blocking(move || {
        collect_hits(&crit, &handles, store.as_ref())
    })
    .await
    {
        Ok(h) => h,
        Err(e) => {
            tracing::warn!(error = %e, "SEARCH evaluation panicked");
            return tagged(tag, Status::No, Some("SERVERBUG"), "search failed");
        }
    };

    let mut out = String::new();
    out.push_str(&render_search_response(&hits, req.mode));
    let text = if req.mode == SearchMode::Uid {
        "UID SEARCH completed"
    } else {
        "SEARCH completed"
    };
    out.push_str(&tagged(tag, Status::Ok, None, text));
    out
}

fn render_search_response(hits: &[(u32, u64)], mode: SearchMode) -> String {
    if hits.is_empty() {
        return "* SEARCH\r\n".to_string();
    }
    let mut s = String::from("* SEARCH");
    for &(msn, uid) in hits {
        s.push(' ');
        match mode {
            SearchMode::Msn => s.push_str(&msn.to_string()),
            SearchMode::Uid => s.push_str(&uid.to_string()),
        }
    }
    s.push_str("\r\n");
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use mds::{BlobHash, ItemId, Seq, Tags};
    use uuid::Uuid;

    fn iid(seed: u64) -> ItemId {
        let mut bytes = [0u8; 16];
        bytes[..8].copy_from_slice(&seed.to_be_bytes());
        ItemId(Uuid::from_bytes(bytes))
    }

    fn make_handle(uid: u64, flags: u32, size: u64) -> EmailHandle {
        EmailHandle {
            id: iid(uid),
            blob_hash: BlobHash([0u8; 32]),
            keywords: Flags(flags),
            tags: Tags::new(),
            received_at: 0,
            seq: Seq(uid as u32),
            uid,
            mod_seq: 0,
            size_bytes: size,
        }
    }

    /// Metadata-only evaluation, for keys that never need the blob.
    fn matches(crit: &Crit, msn: u32, h: &EmailHandle, exists: u32, largest_uid: u64) -> bool {
        eval(crit, msn, h, exists, largest_uid, None).expect("metadata-only key")
    }

    /// Full evaluation against a raw message.
    fn matches_raw(crit: &Crit, h: &EmailHandle, raw: &[u8]) -> bool {
        let doc = Doc::from_raw(raw);
        eval(crit, 1, h, 1, h.uid.max(1), Some(&doc)).unwrap()
    }

    const RAW: &[u8] = b"From: Alice Example <alice@example.com>\r\n\
To: bob@example.net\r\n\
Cc: carol@example.org\r\n\
Subject: =?UTF-8?B?Q2Fmw6kgbWVudQ==?=\r\n\
Date: Tue, 27 Oct 2026 23:30:00 -0400\r\n\
X-Ticket: T-42\r\n\
Content-Type: text/plain; charset=utf-8\r\n\
\r\n\
The quarterly Invoice is attached.\r\n";

    /// Test-only matcher with msn-bound and uid-bound defaulted from
    /// the handle so the property tests stay readable. Real callers
    /// pass the actual mailbox-wide bounds (see `handle()`).
    fn m(crit: &Crit, msn: u32, handle: &EmailHandle) -> bool {
        matches(crit, msn, handle, msn.max(1), handle.uid.max(1))
    }

    #[test]
    fn parse_all() {
        let r = parse_args("ALL", SearchMode::Msn).unwrap();
        assert_eq!(r.crit, Crit::All);
    }

    #[test]
    fn parse_flag_predicates() {
        let r = parse_args("SEEN", SearchMode::Msn).unwrap();
        assert_eq!(r.crit, Crit::FlagsSet(Flags(Flags::SEEN)));
        let r = parse_args("UNSEEN", SearchMode::Msn).unwrap();
        assert_eq!(r.crit, Crit::FlagsClear(Flags(Flags::SEEN)));
        let r = parse_args("FLAGGED UNDELETED", SearchMode::Msn).unwrap();
        assert_eq!(
            r.crit,
            Crit::And(vec![
                Crit::FlagsSet(Flags(Flags::FLAGGED)),
                Crit::FlagsClear(Flags(Flags::DELETED)),
            ])
        );
    }

    #[test]
    fn parse_smaller_larger() {
        let r = parse_args("SMALLER 1024", SearchMode::Msn).unwrap();
        assert_eq!(r.crit, Crit::Smaller(1024));
        let r = parse_args("LARGER 4096", SearchMode::Msn).unwrap();
        assert_eq!(r.crit, Crit::Larger(4096));
    }

    #[test]
    fn parse_uid_and_seqset() {
        let r = parse_args("UID 1:10", SearchMode::Uid).unwrap();
        assert!(matches!(r.crit, Crit::UidInSet(_)));
        let r = parse_args("1:3", SearchMode::Msn).unwrap();
        assert!(matches!(r.crit, Crit::MsnInSet(_)));
    }

    #[test]
    fn parse_not_or() {
        let r = parse_args("NOT SEEN", SearchMode::Msn).unwrap();
        assert!(matches!(r.crit, Crit::Not(_)));
        let r = parse_args("OR SEEN FLAGGED", SearchMode::Msn).unwrap();
        assert!(matches!(r.crit, Crit::Or(_, _)));
    }

    #[test]
    fn parse_parenthesised_and() {
        let r = parse_args("(SEEN FLAGGED)", SearchMode::Msn).unwrap();
        match r.crit {
            Crit::And(ks) => assert_eq!(ks.len(), 2),
            _ => panic!("expected And"),
        }
    }

    /// RFC 9051 §9: `KEYWORD`/`UNKEYWORD` take `flag-keyword`, which is a bare
    /// atom — a `\`-prefixed system flag is a syntax error here, not a
    /// system-flag search. The dedicated keys cover that case.
    #[test]
    fn parse_keyword_rejects_system_flag_syntax() {
        for arg in ["KEYWORD \\Seen", "UNKEYWORD \\Flagged", "KEYWORD \\Nope"] {
            let e = parse_args(arg, SearchMode::Msn).unwrap_err();
            assert!(e.contains("not a system flag"), "{arg}: {e}");
            assert!(e.contains("SEEN/UNSEEN"), "{arg}: {e}");
        }

        // The dedicated search keys are what a client uses instead, and they
        // still resolve to the system bits.
        let r = parse_args("SEEN", SearchMode::Msn).unwrap();
        assert_eq!(r.crit, Crit::FlagsSet(Flags(Flags::SEEN)));
        let r = parse_args("UNFLAGGED", SearchMode::Msn).unwrap();
        assert_eq!(r.crit, Crit::FlagsClear(Flags(Flags::FLAGGED)));

        // A bare name is a user keyword even when it collides with a system
        // flag's name — `Flagged` the keyword is not `\Flagged` the bit.
        let r = parse_args("UNKEYWORD Flagged", SearchMode::Msn).unwrap();
        assert_eq!(r.crit, Crit::Not(Box::new(Crit::TagSet("flagged".into()))));
    }

    #[test]
    fn parse_keyword_and_unkeyword_user_keyword() {
        let r = parse_args("KEYWORD Important", SearchMode::Msn).unwrap();
        assert_eq!(r.crit, Crit::TagSet("important".into()));
        let r = parse_args("UNKEYWORD Important", SearchMode::Msn).unwrap();
        assert_eq!(
            r.crit,
            Crit::Not(Box::new(Crit::TagSet("important".into())))
        );
    }

    #[test]
    fn parse_keyword_case_folds_user_keyword() {
        for raw in ["ProjectX", "projectx", "PROJECTX"] {
            let r = parse_args(&format!("KEYWORD {raw}"), SearchMode::Msn).unwrap();
            assert_eq!(r.crit, Crit::TagSet("projectx".into()), "{raw}");
        }
    }

    #[test]
    fn parse_keyword_nfc_folds_user_keyword() {
        let precomposed = parse_args("KEYWORD café", SearchMode::Msn).unwrap();
        let decomposed = parse_args("KEYWORD cafe\u{301}", SearchMode::Msn).unwrap();
        assert_eq!(precomposed.crit, Crit::TagSet("café".into()));
        assert_eq!(decomposed.crit, precomposed.crit);
    }

    #[test]
    fn parse_keyword_accepts_registry_dollar_keyword() {
        let r = parse_args("KEYWORD $Junk", SearchMode::Msn).unwrap();
        assert_eq!(r.crit, Crit::TagSet("$junk".into()));
    }

    #[test]
    fn parse_keyword_rejects_reserved_system_dollar_name() {
        let e = parse_args("KEYWORD $Seen", SearchMode::Msn).unwrap_err();
        assert!(e.contains("$Seen"), "{e}");
        assert!(e.contains("reserved system name"), "{e}");
    }

    #[test]
    fn parse_recent_old_new_aliases() {
        // NEW = RECENT UNSEEN; we don't track \Recent so NEW must be
        // empty too. Mapping NEW to plain UNSEEN would emit unread
        // *old* mail as `NEW`, which is wrong.
        let r = parse_args("NEW", SearchMode::Msn).unwrap();
        assert_eq!(r.crit, Crit::None);
        let r = parse_args("OLD", SearchMode::Msn).unwrap();
        assert_eq!(r.crit, Crit::All);
        let r = parse_args("RECENT", SearchMode::Msn).unwrap();
        assert_eq!(r.crit, Crit::None);
    }

    #[test]
    fn parse_charset_prefix_is_ignored() {
        let r = parse_args("CHARSET UTF-8 SEEN", SearchMode::Msn).unwrap();
        assert_eq!(r.crit, Crit::FlagsSet(Flags(Flags::SEEN)));
    }

    #[test]
    fn parse_header_criteria() {
        let r = parse_args("FROM \"Alice\"", SearchMode::Uid).unwrap();
        assert_eq!(r.crit, Crit::Header("from".into(), "alice".into()));
        let r = parse_args("SUBJECT hello", SearchMode::Msn).unwrap();
        assert_eq!(r.crit, Crit::Header("subject".into(), "hello".into()));
        let r = parse_args("HEADER X-Ticket \"\"", SearchMode::Msn).unwrap();
        assert_eq!(r.crit, Crit::Header("x-ticket".into(), String::new()));
        let r = parse_args("BODY Invoice", SearchMode::Msn).unwrap();
        assert_eq!(r.crit, Crit::Body("invoice".into()));
        let e = parse_args("FROM", SearchMode::Msn).unwrap_err();
        assert!(e.contains("missing string"), "{e}");
        let e = parse_args("HEADER X-Ticket", SearchMode::Msn).unwrap_err();
        assert!(e.contains("missing string"), "{e}");
    }

    /// Thunderbird's quick filter shape on a Trash folder.
    #[test]
    fn parse_thunderbird_quick_filter() {
        let r = parse_args(
            "CHARSET UTF-8 UNDELETED OR FROM \"alice\" SUBJECT \"alice\"",
            SearchMode::Uid,
        )
        .unwrap();
        assert_eq!(
            r.crit,
            Crit::And(vec![
                Crit::FlagsClear(Flags(Flags::DELETED)),
                Crit::Or(
                    Box::new(Crit::Header("from".into(), "alice".into())),
                    Box::new(Crit::Header("subject".into(), "alice".into())),
                ),
            ])
        );
    }

    #[test]
    fn parse_dates() {
        let d = NaiveDate::from_ymd_opt(2026, 2, 1).unwrap();
        assert_eq!(parse_args("SINCE 1-Feb-2026", SearchMode::Msn).unwrap().crit, Crit::Since(d));
        assert_eq!(
            parse_args("BEFORE \"01-feb-2026\"", SearchMode::Msn).unwrap().crit,
            Crit::Before(d)
        );
        assert_eq!(parse_args("SENTON 1-FEB-2026", SearchMode::Msn).unwrap().crit, Crit::SentOn(d));
        for bad in ["SINCE 2026-02-01", "SINCE 31-Feb-2026", "SINCE 1-Febr-2026", "ON"] {
            assert!(parse_args(bad, SearchMode::Msn).is_err(), "{bad}");
        }
    }

    #[test]
    fn matches_header_fields_raw_and_decoded() {
        let h = make_handle(1, 0, 100);
        let hdr = |n: &str, s: &str| Crit::Header(n.into(), s.to_lowercase());
        assert!(matches_raw(&hdr("from", "ALICE"), &h, RAW));
        assert!(matches_raw(&hdr("from", "alice@example.com"), &h, RAW));
        assert!(!matches_raw(&hdr("from", "bob"), &h, RAW));
        assert!(matches_raw(&hdr("to", "bob@"), &h, RAW));
        assert!(matches_raw(&hdr("cc", "carol"), &h, RAW));
        assert!(!matches_raw(&hdr("bcc", ""), &h, RAW));
        // Encoded-word subject: found by its decoded text and its raw form.
        assert!(matches_raw(&hdr("subject", "café"), &h, RAW));
        assert!(matches_raw(&hdr("subject", "CAFÉ MENU"), &h, RAW));
        assert!(matches_raw(&hdr("subject", "=?utf-8?b?"), &h, RAW));
        assert!(matches_raw(&hdr("x-ticket", ""), &h, RAW));
        assert!(matches_raw(&hdr("x-ticket", "t-42"), &h, RAW));
    }

    #[test]
    fn matches_body_and_text() {
        let h = make_handle(1, 0, 100);
        assert!(matches_raw(&Crit::Body("invoice".into()), &h, RAW));
        assert!(!matches_raw(&Crit::Body("alice".into()), &h, RAW));
        assert!(matches_raw(&Crit::Text("alice".into()), &h, RAW));
        assert!(matches_raw(&Crit::Text("quarterly".into()), &h, RAW));
        assert!(!matches_raw(&Crit::Text("absent".into()), &h, RAW));
    }

    #[test]
    fn matches_dates() {
        // 2026-10-28 01:00 UTC.
        let mut h = make_handle(1, 0, 100);
        h.received_at = 1_793_149_200_000;
        let day = |d| NaiveDate::from_ymd_opt(2026, 10, d).unwrap();
        assert!(matches(&Crit::On(day(28)), 1, &h, 1, 1));
        assert!(matches(&Crit::Since(day(28)), 1, &h, 1, 1));
        assert!(!matches(&Crit::Before(day(28)), 1, &h, 1, 1));
        assert!(matches(&Crit::Before(day(29)), 1, &h, 1, 1));
        // The Date: header's own day (27th, -0400), not its UTC instant.
        assert!(matches_raw(&Crit::SentOn(day(27)), &h, RAW));
        assert!(matches_raw(&Crit::SentBefore(day(28)), &h, RAW));
        assert!(!matches_raw(&Crit::SentSince(day(28)), &h, RAW));
        // No Date: header falls back to the internal date.
        let no_date = b"Subject: x\r\n\r\nbody\r\n";
        assert!(matches_raw(&Crit::SentOn(day(28)), &h, no_date));
    }

    /// Metadata settles a content key when it can, so the blob is
    /// only loaded for messages that are still undecided.
    #[test]
    fn eval_short_circuits_without_blob() {
        let deleted = make_handle(1, Flags::DELETED, 100);
        let live = make_handle(2, 0, 100);
        let crit = parse_args("UNDELETED FROM alice", SearchMode::Uid).unwrap().crit;
        assert_eq!(eval(&crit, 1, &deleted, 2, 2, None), Some(false));
        assert_eq!(eval(&crit, 2, &live, 2, 2, None), None);
        let crit = parse_args("OR ALL FROM alice", SearchMode::Uid).unwrap().crit;
        assert_eq!(eval(&crit, 2, &live, 2, 2, None), Some(true));
        let crit = parse_args("NOT FROM alice", SearchMode::Uid).unwrap().crit;
        assert_eq!(eval(&crit, 2, &live, 2, 2, None), None);
        assert!(!matches_raw(&crit, &live, RAW));
    }

    #[test]
    fn parse_return_esearch_rejected() {
        let e = parse_args("RETURN (ALL) SEEN", SearchMode::Msn).unwrap_err();
        assert!(e.contains("ESEARCH"), "{e}");
    }

    #[test]
    fn parse_empty_args_rejected() {
        let e = parse_args("", SearchMode::Msn).unwrap_err();
        assert!(e.contains("at least one"), "{e}");
        let e = parse_args("   ", SearchMode::Msn).unwrap_err();
        assert!(e.contains("at least one"), "{e}");
    }

    #[test]
    fn parse_unterminated_paren_rejected() {
        let e = parse_args("(SEEN FLAGGED", SearchMode::Msn).unwrap_err();
        assert!(e.contains("closing paren"), "{e}");
    }

    #[test]
    fn matches_all_is_unconditional() {
        let h = make_handle(1, 0, 100);
        assert!(m(&Crit::All, 1, &h));
    }

    #[test]
    fn matches_flags_set_requires_all_named_bits() {
        let seen_flagged = make_handle(1, Flags::SEEN | Flags::FLAGGED, 100);
        let just_seen = make_handle(2, Flags::SEEN, 100);
        let both = Crit::FlagsSet(Flags(Flags::SEEN | Flags::FLAGGED));
        assert!(m(&both, 1, &seen_flagged));
        assert!(!m(&both, 2, &just_seen));
    }

    #[test]
    fn matches_flags_clear_rejects_when_any_named_bit_is_set() {
        let seen = make_handle(1, Flags::SEEN, 100);
        let none = make_handle(2, 0, 100);
        let unseen = Crit::FlagsClear(Flags(Flags::SEEN));
        assert!(!m(&unseen, 1, &seen));
        assert!(m(&unseen, 2, &none));
    }

    #[test]
    fn matches_keyword_and_unkeyword_against_tags() {
        let mut tagged = make_handle(1, 0, 100);
        tagged.tags.insert("projectx".into());
        let mut other = make_handle(2, 0, 100);
        other.tags.insert("other".into());
        let empty = make_handle(3, 0, 100);

        let keyword = parse_args("KEYWORD ProjectX", SearchMode::Msn)
            .unwrap()
            .crit;
        let unkeyword = parse_args("UNKEYWORD ProjectX", SearchMode::Msn)
            .unwrap()
            .crit;

        assert!(m(&keyword, 1, &tagged));
        assert!(!m(&keyword, 2, &other));
        assert!(!m(&keyword, 3, &empty));
        assert!(!m(&unkeyword, 1, &tagged));
        assert!(m(&unkeyword, 2, &other));
        assert!(m(&unkeyword, 3, &empty));
    }

    #[test]
    fn matches_smaller_larger_comparisons() {
        let h = make_handle(1, 0, 1000);
        assert!(m(&Crit::Smaller(2000), 1, &h));
        assert!(!m(&Crit::Smaller(1000), 1, &h));
        assert!(m(&Crit::Larger(500), 1, &h));
        assert!(!m(&Crit::Larger(1000), 1, &h));
    }

    #[test]
    fn matches_not_or_and_combinators() {
        let h = make_handle(1, Flags::SEEN, 100);
        let not_seen = Crit::Not(Box::new(Crit::FlagsSet(Flags(Flags::SEEN))));
        assert!(!m(&not_seen, 1, &h));
        let or = Crit::Or(
            Box::new(Crit::FlagsSet(Flags(Flags::FLAGGED))),
            Box::new(Crit::FlagsSet(Flags(Flags::SEEN))),
        );
        assert!(m(&or, 1, &h));
        let and = Crit::And(vec![Crit::FlagsSet(Flags(Flags::SEEN)), Crit::Smaller(200)]);
        assert!(m(&and, 1, &h));
    }

    #[test]
    fn matches_msn_in_set_respects_exists() {
        let h = make_handle(1, 0, 100);
        let set = SeqSet::parse("2:*").unwrap();
        let crit = Crit::MsnInSet(set);
        // exists=3, msn=2 → in set
        assert!(matches(&crit, 2, &h, 3, 1));
        // exists=3, msn=1 → not in set (range is 2..=3)
        assert!(!matches(&crit, 1, &h, 3, 1));
    }

    #[test]
    fn matches_uid_in_set_respects_largest_uid() {
        let h = make_handle(30, 0, 100);
        // RFC 9051 §6.4.8: `3291:*` covers the largest uid even if
        // out of nominal range.
        let set = SeqSet::parse("3291:*").unwrap();
        let crit = Crit::UidInSet(set);
        assert!(matches(&crit, 3, &h, 3, 30));
    }

    #[test]
    fn render_empty_emits_bare_search() {
        assert_eq!(render_search_response(&[], SearchMode::Msn), "* SEARCH\r\n");
        assert_eq!(render_search_response(&[], SearchMode::Uid), "* SEARCH\r\n");
    }

    #[test]
    fn render_emits_msn_for_search_mode() {
        let hits = [(1u32, 100u64), (3, 102), (5, 104)];
        assert_eq!(
            render_search_response(&hits, SearchMode::Msn),
            "* SEARCH 1 3 5\r\n"
        );
    }

    #[test]
    fn render_emits_uid_for_uid_search_mode() {
        let hits = [(1u32, 100u64), (3, 102), (5, 104)];
        assert_eq!(
            render_search_response(&hits, SearchMode::Uid),
            "* SEARCH 100 102 104\r\n"
        );
    }
}
