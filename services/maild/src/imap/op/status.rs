// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>

//! STATUS reports mailbox counters, UID identity, SIZE, DELETED and the
//! configured APPENDLIMIT. SIZE and DELETED use membership metadata without
//! loading message blobs. RECENT remains zero for rev1 compatibility.
//! HIGHESTMODSEQ remains gated on the complete imap-ext CONDSTORE implementation.

use std::sync::Arc;

use crate::imap::mailbox_name::{ResolveResult, encode_all, quote_mailbox, resolve};
use crate::imap::op::list;
use crate::imap::response::{Status, tagged, wire_uidvalidity};
use crate::mailstore::MailStore;

type AccountId = i32;

const SUPPORTED_ITEMS: &str = "MESSAGES UIDNEXT UIDVALIDITY UNSEEN RECENT SIZE DELETED APPENDLIMIT";

/// Parsed STATUS arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Args {
    pub mailbox: String,
    pub items: Vec<StatusItem>,
}

/// One STATUS data item the v1 handler supports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StatusItem {
    Messages,
    UidNext,
    UidValidity,
    Unseen,
    /// Constant `0` — maild never tracks `\Recent` (see module doc).
    Recent,
    Size,
    Deleted,
    AppendLimit,
}

impl StatusItem {
    fn parse(s: &str) -> Result<Self, String> {
        match s.to_ascii_uppercase().as_str() {
            "MESSAGES" => Ok(Self::Messages),
            "UIDNEXT" => Ok(Self::UidNext),
            "UIDVALIDITY" => Ok(Self::UidValidity),
            "UNSEEN" => Ok(Self::Unseen),
            "RECENT" => Ok(Self::Recent),
            "SIZE" => Ok(Self::Size),
            "DELETED" => Ok(Self::Deleted),
            "APPENDLIMIT" => Ok(Self::AppendLimit),
            // imap-ext hook: enable HIGHESTMODSEQ together with its CONDSTORE
            // session state and durable per-membership MODSEQ semantics.
            other => Err(format!(
                "STATUS item {other:?} not supported (supported: {SUPPORTED_ITEMS})"
            )),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Messages => "MESSAGES",
            Self::UidNext => "UIDNEXT",
            Self::UidValidity => "UIDVALIDITY",
            Self::Unseen => "UNSEEN",
            Self::Recent => "RECENT",
            Self::Size => "SIZE",
            Self::Deleted => "DELETED",
            Self::AppendLimit => "APPENDLIMIT",
        }
    }
}

/// Parse `STATUS <mailbox> (<item>...)`. The parenthesised item
/// list MUST be non-empty (RFC 9051 §6.3.11) and contain at least
/// one supported atom.
pub fn parse_args(args: &str) -> Result<Args, String> {
    let s = args.trim_start();
    if s.is_empty() {
        return Err("STATUS requires a mailbox and item list".to_string());
    }
    let (mailbox, rest) = list::take_mailbox_name(s)?;
    if mailbox.is_empty() {
        return Err("STATUS requires a non-empty mailbox name".to_string());
    }
    let rest = rest.trim_start();
    let Some(rest) = rest.strip_prefix('(') else {
        return Err("STATUS item list must be parenthesised".to_string());
    };
    let Some(end) = rest.find(')') else {
        return Err("STATUS item list missing closing paren".to_string());
    };
    let body = &rest[..end];
    let tail = rest[end + 1..].trim();
    if !tail.is_empty() {
        return Err("STATUS trailing tokens after item list".to_string());
    }
    let mut items = Vec::new();
    for tok in body.split_whitespace() {
        items.push(StatusItem::parse(tok)?);
    }
    if items.is_empty() {
        return Err("STATUS item list must be non-empty".to_string());
    }
    Ok(Args { mailbox, items })
}

/// Execute STATUS for an authenticated session.
pub async fn handle(
    tag: &str,
    args: &str,
    account_id: AccountId,
    store: Arc<dyn MailStore>,
) -> String {
    handle_with_limit(tag, args, account_id, store, crate::imap::config::ImapConfig::default().max_literal_bytes).await
}

pub async fn handle_with_limit(tag: &str, args: &str, account_id: AccountId, store: Arc<dyn MailStore>, append_limit: u64) -> String {
    let parsed = match parse_args(args) {
        Ok(p) => p,
        Err(e) => return tagged(tag, Status::Bad, None, &e),
    };

    let list_store = store.clone();
    let records = match tokio::task::spawn_blocking(move || list_store.list_mailboxes(account_id)).await
    {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => {
            tracing::warn!(error = %e, "STATUS list_mailboxes failed");
            return tagged(tag, Status::No, Some("SERVERBUG"), "status failed");
        }
        Err(e) => {
            tracing::warn!(error = %e, "STATUS spawn_blocking panicked");
            return tagged(tag, Status::No, Some("SERVERBUG"), "status failed");
        }
    };

    let resolved_id = match resolve(&parsed.mailbox, &records) {
        ResolveResult::Found(id) => id,
        ResolveResult::NotFound => {
            return tagged(
                tag,
                Status::No,
                Some("NONEXISTENT"),
                "mailbox does not exist",
            );
        }
        ResolveResult::Unrepresentable => {
            return tagged(
                tag,
                Status::No,
                Some("SERVERBUG"),
                "mailbox name not representable over IMAP",
            );
        }
    };

    let Some(rec) = records.iter().find(|r| r.id == resolved_id) else {
        tracing::error!(
            resolved_id = ?resolved_id,
            "STATUS resolve returned id absent from list_mailboxes (storage race?)"
        );
        return tagged(tag, Status::No, Some("SERVERBUG"), "status failed");
    };

    let set = encode_all(&records);
    let wire_name = match set
        .mailboxes
        .iter()
        .find(|enc| enc.id == resolved_id)
        .map(|enc| enc.wire_name.clone())
    {
        Some(n) => n,
        None => {
            tracing::error!(
                resolved_id = ?resolved_id,
                "STATUS could not re-encode wire name for resolved record (encoder/resolve drift)"
            );
            return tagged(tag, Status::No, Some("SERVERBUG"), "status failed");
        }
    };

    let (size, deleted) = if parsed.items.iter().any(|i| matches!(i, StatusItem::Size | StatusItem::Deleted)) {
        match tokio::task::spawn_blocking(move || {
            let handles = store.list_emails_in_mailbox(account_id, resolved_id, crate::mailstore::ListOpts { sort_by: crate::mailstore::SortKey::Seq, limit: u32::MAX, offset: 0 })?;
            Ok::<_, anyhow::Error>(handles.iter().fold((0u64, 0u64), |(size, deleted), h| (size.saturating_add(h.size_bytes), deleted + u64::from(h.keywords.0 & mds::Flags::DELETED != 0))))
        }).await {
            Ok(Ok(totals)) => totals,
            _ => return tagged(tag, Status::No, Some("UNAVAILABLE"), "status unavailable"),
        }
    } else { (0, 0) };
    let mut body = String::new();
    for (i, item) in parsed.items.iter().enumerate() {
        if i > 0 {
            body.push(' ');
        }
        let value: u64 = match item {
            StatusItem::Messages => rec.total_emails,
            StatusItem::UidNext => rec.uid_next,
            StatusItem::UidValidity => u64::from(wire_uidvalidity(rec.uid_validity)),
            StatusItem::Unseen => rec.unread_emails,
            StatusItem::Recent => 0,
            StatusItem::Size => size,
            StatusItem::Deleted => deleted,
            StatusItem::AppendLimit => append_limit,
        };
        body.push_str(item.as_str());
        body.push(' ');
        body.push_str(&value.to_string());
    }

    let mut out = format!("* STATUS {} ({})\r\n", quote_mailbox(&wire_name), body);
    out.push_str(&tagged(tag, Status::Ok, None, "STATUS completed"));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_args_accepts_single_item() {
        let p = parse_args("INBOX (MESSAGES)").unwrap();
        assert_eq!(p.mailbox, "INBOX");
        assert_eq!(p.items, vec![StatusItem::Messages]);
    }

    #[test]
    fn parse_args_accepts_quoted_mailbox_and_multiple_items() {
        let p = parse_args(r#""Archive/2025" (MESSAGES UIDNEXT UIDVALIDITY UNSEEN)"#).unwrap();
        assert_eq!(p.mailbox, "Archive/2025");
        assert_eq!(
            p.items,
            vec![
                StatusItem::Messages,
                StatusItem::UidNext,
                StatusItem::UidValidity,
                StatusItem::Unseen
            ]
        );
    }

    #[test]
    fn parse_args_item_matching_is_case_insensitive() {
        let p = parse_args("INBOX (messages uidnext)").unwrap();
        assert_eq!(p.items, vec![StatusItem::Messages, StatusItem::UidNext]);
    }

    #[test]
    fn parse_args_accepts_recent_as_constant_zero_item() {
        // Thunderbird's rev1 folder poll: every item must parse or the
        // client's whole folder machinery (draft save, Sent fcc) fails.
        let p = parse_args(r#""Sent" (UIDNEXT MESSAGES UNSEEN RECENT)"#).unwrap();
        assert_eq!(p.mailbox, "Sent");
        assert_eq!(
            p.items,
            vec![
                StatusItem::UidNext,
                StatusItem::Messages,
                StatusItem::Unseen,
                StatusItem::Recent
            ]
        );
    }

    #[test]
    fn parse_args_rejects_unsupported_items() {
        for item in ["HIGHESTMODSEQ", "BOGUS"] {
            let e = parse_args(&format!("INBOX ({item})")).unwrap_err();
            assert!(e.contains("not supported"), "{item} -> {e}");
        }
    }

    #[test]
    fn parse_args_rejects_empty_item_list() {
        let e = parse_args("INBOX ()").unwrap_err();
        assert!(e.contains("non-empty"), "{e}");
    }

    #[test]
    fn parse_args_rejects_missing_parens() {
        let e = parse_args("INBOX MESSAGES").unwrap_err();
        assert!(e.contains("parenthesised"), "{e}");
    }

    #[test]
    fn parse_args_rejects_unclosed_paren() {
        let e = parse_args("INBOX (MESSAGES").unwrap_err();
        assert!(e.contains("closing paren"), "{e}");
    }

    #[test]
    fn parse_args_rejects_trailing_tokens() {
        let e = parse_args("INBOX (MESSAGES) trailing").unwrap_err();
        assert!(e.contains("trailing"), "{e}");
    }

    #[test]
    fn parse_args_rejects_empty() {
        assert!(parse_args("").is_err());
    }

    #[test]
    fn parse_args_rejects_empty_mailbox_name() {
        let e = parse_args(r#""" (MESSAGES)"#).unwrap_err();
        assert!(e.contains("non-empty mailbox"), "{e}");
    }
}
