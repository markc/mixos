// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>

//! `APPEND` — RFC 9051 §6.3.12 / RFC 4315 (UIDPLUS APPENDUID).
//!
//! The codec ([`crate::imap::codec`]) frames synchronising,
//! non-synchronising and binary literals. The session separates argument
//! strings from message octets and supplies one or more MULTIAPPEND entries.
//!
//! Wire grammar (after the command is stripped):
//!
//! ```text
//! args        = mailbox [SP "(" flag-list ")"] [SP date-time]
//! mailbox     = atom / quoted-string
//! date-time   = '"' DD "-" Mmm "-" YYYY SP HH ":" MM ":" SS SP ("+"/"-") ZHHMM '"'
//! flag-list   = flag *(SP flag)
//! flag        = system-flag / keyword
//! ```
//!
//! The literal itself is stripped from the line by the codec, so the
//! grammar here is the head only.
//!
//! ## Output
//!
//! On success, RFC 4315 §3 mandates:
//!
//! ```text
//! <tag> OK [APPENDUID <uidvalidity> <uid>] APPEND completed
//! ```
//!
//! where `<uidvalidity>` and `<uid>` come from the [`MembershipUid`]
//! returned by [`crate::mailstore::MailStore::append_emails`] — both
//! read inside the same `with_set_tx` that committed the membership,
//! so an APPENDUID for a freshly-inserted membership cannot diverge
//! from the actual stored row.
//!
//! ## Error mapping
//!
//! | Failure                                | Response                                  |
//! |----------------------------------------|-------------------------------------------|
//! | grammar parse                          | `BAD <reason>`                            |
//! | mailbox does not exist                 | `NO [TRYCREATE] mailbox does not exist`   |
//! | mail_parser rejects the literal        | `NO [PARSE] message body not parseable`   |
//! | unsupported binary representation      | `NO [UNKNOWN-CTE]`                       |
//! | substrate error                        | `NO [UNAVAILABLE] APPEND unavailable`     |
//!
//! `TRYCREATE` is RFC 9051 §7.1's hint that the client should retry
//! after `CREATE`-ing the mailbox; Thunderbird honours it on first-
//! send for an unseen Sent / Drafts.

use std::sync::Arc;

use mail_parser::MessageParser;
use mds::{Flags, Tags};

use crate::imap::mailbox_name::{ResolveResult, resolve};
use crate::imap::op::list;
use crate::imap::response::{Status, tagged, wire_uidvalidity};
use crate::jmap::email::{build_envelope, extract_in_reply_to};
use crate::mailstore::MailStore;

type AccountId = i32;

/// Parsed APPEND head (everything before the literal).
#[derive(Debug, Clone, PartialEq, Eq)]
struct AppendReq {
    mailbox: String,
    flags: Flags,
    /// User keywords (bare atoms) from the flag-list, normalised.
    tags: Tags,
    /// Parsed `INTERNALDATE` in unix-millis, or `None` if absent (the
    /// handler then defaults to "now").
    received_at_ms: Option<i64>,
}

/// Parse the APPEND argument head — mailbox, optional flag-list, and
/// optional date-time. The literal has already been stripped by the
/// codec, so `args` is everything between the command and the (now
/// removed) literal marker. Trailing whitespace is tolerated.
fn parse_args(args: &str) -> Result<AppendReq, String> {
    let s = args.trim_start();
    if s.is_empty() {
        return Err("APPEND requires a mailbox name".to_string());
    }

    let (mailbox, mut rest) = list::take_mailbox_name(s)?;
    if mailbox.is_empty() {
        return Err("APPEND requires a non-empty mailbox name".to_string());
    }
    rest = rest.trim_start();

    // Optional flag-list.
    let mut flags = Flags(0);
    let mut tags = Tags::new();
    if rest.starts_with('(') {
        let close = rest
            .find(')')
            .ok_or_else(|| "APPEND flag-list missing closing paren".to_string())?;
        let inner = &rest[1..close];
        let (f, t) = parse_flag_list(inner)?;
        flags = f;
        tags = t;
        rest = rest[close + 1..].trim_start();
    }

    // Optional date-time (quoted string).
    let received_at_ms = if rest.starts_with('"') {
        let close = rest[1..]
            .find('"')
            .ok_or_else(|| "APPEND date-time missing closing quote".to_string())?;
        let dt = &rest[1..1 + close];
        let parsed = parse_internaldate(dt)?;
        rest = rest[1 + close + 1..].trim_start();
        Some(parsed)
    } else {
        None
    };

    if !rest.is_empty() {
        return Err(format!("APPEND: unexpected trailing argument {:?}", rest));
    }

    Ok(AppendReq {
        mailbox,
        flags,
        tags,
        received_at_ms,
    })
}

/// Pre-validate so grammar failures bump the bad-command counter at
/// dispatch time. The handler re-parses on its own path; the second
/// parse is pure.
pub fn parse_args_for_dispatch(args: &str) -> Result<(), String> {
    parse_args(args).map(|_| ())
}

/// Parse the inner of an APPEND `<flag-list>` (parens already stripped by the
/// caller) into system flags + user keywords, via the shared [`super::flags`]
/// implementation STORE / FETCH also use.
fn parse_flag_list(s: &str) -> Result<(Flags, Tags), String> {
    super::flags::parse_flag_list_inner(s).map_err(|e| format!("APPEND: {e}"))
}

/// Parse an IMAP `date-time` — `"DD-Mmm-YYYY HH:MM:SS +ZZZZ"` — into
/// unix milliseconds. Returns `Err` on any structural deviation;
/// callers map to `BAD`. Mirrors the `chrono::DateTime::parse_from_str`
/// shape used elsewhere in the crate.
fn parse_internaldate(s: &str) -> Result<i64, String> {
    // `%d-%b-%Y %H:%M:%S %z` accepts e.g. `01-Jan-2026 10:00:00 +0000`.
    // chrono is case-insensitive on month names.
    let dt = chrono::DateTime::parse_from_str(s, "%d-%b-%Y %H:%M:%S %z")
        .map_err(|e| format!("APPEND: invalid date-time {s:?}: {e}"))?;
    Ok(dt.timestamp_millis())
}

/// Execute APPEND for an authenticated session.
///
/// The codec has already consumed the literal body, so `body` carries
/// the exact N bytes that landed on the wire. The mailbox resolution
/// runs against the freshest `list_mailboxes` snapshot — APPEND has
/// no SELECTED-state pre-condition, just AUTHENTICATED.
pub async fn handle(
    tag: &str,
    args: &str,
    body: Vec<u8>,
    account_id: AccountId,
    store: Arc<dyn MailStore>,
) -> String {
    handle_many(tag, vec![(args.to_string(), body)], account_id, store).await
}

/// Parse every message before the first mutation; commit the complete batch in
/// one native MailStore transaction (RFC 3502 all-or-nothing guarantee).
pub async fn handle_many(
    tag: &str,
    messages: Vec<(String, Vec<u8>)>,
    account_id: AccountId,
    store: Arc<dyn MailStore>,
) -> String {
    let mut prepared = Vec::with_capacity(messages.len());
    let mut mailbox = None;
    for (args, body) in messages {
        let req = match parse_args(&args) {
            Ok(r) => r,
            Err(e) => return tagged(tag, Status::Bad, None, &e),
        };
        if mailbox.as_ref().is_some_and(|name| name != &req.mailbox) {
            return tagged(
                tag,
                Status::Bad,
                None,
                "MULTIAPPEND has one destination mailbox",
            );
        }
        mailbox = Some(req.mailbox);
        if body.is_empty() {
            return tagged(tag, Status::No, Some("PARSE"), "empty message");
        }
        let body = if body.contains(&0) {
            match normalise_binary_message(&body) {
                Ok(body) => body,
                Err(()) => {
                    return tagged(
                        tag,
                        Status::No,
                        Some("UNKNOWN-CTE"),
                        "binary message cannot be represented losslessly",
                    );
                }
            }
        } else {
            body
        };
        let Some(parsed) = MessageParser::default().parse(&body) else {
            return tagged(tag, Status::No, Some("PARSE"), "message body not parseable");
        };
        let envelope = build_envelope(&parsed);
        let in_reply_to = extract_in_reply_to(&parsed);
        prepared.push((
            body,
            envelope,
            in_reply_to,
            req.flags,
            req.tags,
            req.received_at_ms
                .unwrap_or_else(|| chrono::Utc::now().timestamp_millis()),
        ));
    }
    let Some(mailbox) = mailbox else {
        return tagged(tag, Status::Bad, None, "APPEND requires a message");
    };
    let outcome = tokio::task::spawn_blocking(move || {
        let records = store.list_mailboxes(account_id)?;
        let container_id = match resolve(&mailbox, &records) {
            ResolveResult::Found(id) => id,
            _ => anyhow::bail!("mailbox not found"),
        };
        let mut imports = Vec::with_capacity(prepared.len());
        for (body, envelope, in_reply_to, flags, tags, received_at) in prepared {
            imports.push(crate::mailstore::EmailImport {
                blob: store.put_blob(&body)?,
                envelope,
                in_reply_to,
                flags,
                tags,
                received_at,
            });
        }
        store.append_emails(account_id, container_id, imports)
    })
    .await;
    match outcome {
        Ok(Ok(results)) if !results.is_empty() => {
            let validity = wire_uidvalidity(results[0].1.uid_validity);
            let uids: Vec<u64> = results.iter().map(|r| r.1.uid).collect();
            let code = format!(
                "APPENDUID {} {}",
                validity,
                super::copy::render_uid_set(&uids)
            );
            tagged(tag, Status::Ok, Some(&code), "APPEND completed")
        }
        Ok(Err(e)) if e.to_string().starts_with("mailbox not found") => {
            tagged(tag, Status::No, Some("TRYCREATE"), "mailbox does not exist")
        }
        _ => tagged(tag, Status::No, Some("UNAVAILABLE"), "APPEND unavailable"),
    }
}

/// BODY fetches are ordinary IMAP strings, which cannot contain NUL. RFC 3516
/// permits changing the transfer encoding during binary APPEND. Encode raw
/// binary leaf bodies without changing their decoded octets or other headers.
fn normalise_binary_message(raw: &[u8]) -> Result<Vec<u8>, ()> {
    use base64::Engine;
    use mail_parser::{MimeHeaders, PartType};
    if !raw.contains(&0) {
        return Ok(raw.to_vec());
    }
    let message = MessageParser::default().parse(raw).ok_or(())?;
    let mut replacements: Vec<(usize, usize, Vec<u8>)> = Vec::new();
    for part in &message.parts {
        let header_start = part.offset_header as usize;
        let body_start = part.offset_body as usize;
        let body_end = part.offset_end as usize;
        let header = raw.get(header_start..body_start).ok_or(())?;
        if header.contains(&0) {
            return Err(());
        }
        let data = raw.get(body_start..body_end).ok_or(())?;
        if !data.contains(&0) {
            continue;
        }
        let (cte, data) = match &part.body {
            PartType::Multipart(_) => continue,
            PartType::Message(inner) => (
                "8bit",
                normalise_binary_message(inner.raw_message.as_ref())?,
            ),
            _ => {
                let cte = part.content_transfer_encoding().unwrap_or("7bit");
                if !["7bit", "8bit", "binary"]
                    .iter()
                    .any(|s| s.eq_ignore_ascii_case(cte))
                {
                    return Err(());
                }
                let encoded = base64::engine::general_purpose::STANDARD.encode(data);
                let mut body = Vec::with_capacity(encoded.len() + encoded.len() / 38);
                for (i, line) in encoded.as_bytes().chunks(76).enumerate() {
                    if i != 0 {
                        body.extend_from_slice(b"\r\n");
                    }
                    body.extend_from_slice(line);
                }
                ("base64", body)
            }
        };
        let mut new_header = Vec::with_capacity(header.len() + 48);
        let mut cursor = header_start;
        for field in &part.headers {
            if field
                .name
                .as_str()
                .eq_ignore_ascii_case("Content-Transfer-Encoding")
            {
                let start = field.offset_field as usize;
                let mut end = field.offset_end as usize;
                if start < cursor || end > body_start {
                    return Err(());
                }
                while end < body_start && matches!(raw[end], b'\r' | b'\n') {
                    end += 1;
                }
                new_header.extend_from_slice(&raw[cursor..start]);
                cursor = end;
            }
        }
        new_header.extend_from_slice(&raw[cursor..body_start]);
        if new_header.ends_with(b"\r\n\r\n") {
            new_header.truncate(new_header.len() - 2);
        } else if new_header.ends_with(b"\n\n") {
            new_header.pop();
        } else if !new_header.is_empty() && !new_header.ends_with(b"\n") {
            new_header.extend_from_slice(b"\r\n");
        }
        new_header
            .extend_from_slice(format!("Content-Transfer-Encoding: {cte}\r\n\r\n").as_bytes());
        new_header.extend_from_slice(&data);
        replacements.push((header_start, body_end, new_header));
    }
    replacements.sort_by_key(|r| r.0);
    let mut output = Vec::with_capacity(raw.len());
    let mut cursor = 0;
    for (start, end, bytes) in replacements {
        if start < cursor || end < start {
            return Err(());
        }
        output.extend_from_slice(&raw[cursor..start]);
        output.extend_from_slice(&bytes);
        cursor = end;
    }
    output.extend_from_slice(&raw[cursor..]);
    if output.contains(&0) {
        return Err(());
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_args_minimal() {
        let r = parse_args("INBOX").unwrap();
        assert_eq!(r.mailbox, "INBOX");
        assert_eq!(r.flags.0, 0);
        assert!(r.received_at_ms.is_none());
    }

    #[test]
    fn parse_args_quoted_mailbox() {
        let r = parse_args(r#""Sent Items""#).unwrap();
        assert_eq!(r.mailbox, "Sent Items");
    }

    #[test]
    fn parse_args_with_flags() {
        let r = parse_args(r"INBOX (\Seen \Draft)").unwrap();
        assert_eq!(r.flags.0, Flags::SEEN | Flags::DRAFT);
        assert!(r.received_at_ms.is_none());
    }

    #[test]
    fn parse_args_with_date_time() {
        let r = parse_args(r#"INBOX "01-Jan-2026 10:00:00 +0000""#).unwrap();
        assert!(r.received_at_ms.is_some());
    }

    #[test]
    fn parse_args_with_flags_and_date_time() {
        let r = parse_args(r#"INBOX (\Seen) "01-Jan-2026 10:00:00 +1000""#).unwrap();
        assert_eq!(r.flags.0, Flags::SEEN);
        assert!(r.received_at_ms.is_some());
    }

    #[test]
    fn parse_args_empty_flag_list_is_zero() {
        let r = parse_args("INBOX ()").unwrap();
        assert_eq!(r.flags.0, 0);
    }

    #[test]
    fn parse_args_case_insensitive_flag_names() {
        let r = parse_args(r"INBOX (\seen \DRAFT)").unwrap();
        assert_eq!(r.flags.0, Flags::SEEN | Flags::DRAFT);
    }

    #[test]
    fn parse_args_rejects_empty() {
        assert!(parse_args("").is_err());
    }

    #[test]
    fn parse_args_rejects_unknown_flag() {
        let e = parse_args(r"INBOX (\Bogus)").unwrap_err();
        assert!(e.contains("unknown system flag"), "{e}");
    }

    #[test]
    fn parse_args_rejects_trailing_junk() {
        let e = parse_args(r"INBOX trailing").unwrap_err();
        assert!(e.contains("trailing"), "{e}");
    }

    #[test]
    fn parse_args_rejects_unclosed_flag_list() {
        let e = parse_args(r"INBOX (\Seen").unwrap_err();
        assert!(e.contains("closing paren"), "{e}");
    }

    #[test]
    fn parse_args_rejects_unclosed_date_time() {
        let e = parse_args(r#"INBOX "01-Jan-2026"#).unwrap_err();
        assert!(e.contains("closing quote"), "{e}");
    }

    #[test]
    fn parse_args_rejects_bad_date_time() {
        let e = parse_args(r#"INBOX "not-a-date""#).unwrap_err();
        assert!(e.contains("invalid date-time"), "{e}");
    }
}
