// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>

//! LSUB shares LIST reference and wildcard rules and includes persistent
//! subscriptions whose mailbox no longer exists. Subscription names survive
//! DELETE and RENAME until explicitly removed with UNSUBSCRIBE.

use std::sync::Arc;

use crate::imap::mailbox_name::{matches_pattern, render_mailbox_line};
use crate::imap::op::list;
use crate::imap::response::{Status, tagged};
use crate::mailstore::MailStore;

type AccountId = i32;

/// Execute LSUB for an authenticated session.
pub async fn handle(
    tag: &str,
    args: &str,
    account_id: AccountId,
    store: Arc<dyn MailStore>,
) -> String {
    let parsed = match list::parse_args(args) {
        Ok(p) => p,
        Err(e) => return tagged(tag, Status::Bad, None, &e),
    };
    if !list::pattern_is_ascii_clean(&parsed.pattern) {
        return tagged(
            tag,
            Status::Bad,
            None,
            "non-ASCII or control bytes in LSUB pattern (UTF8=ACCEPT not advertised)",
        );
    }
    // Empty pattern: per RFC 9051 LSUB returns no rows (the
    // delimiter probe lives on LIST). Emit just the tagged OK.
    if parsed.pattern.is_empty() {
        return tagged(tag, Status::Ok, None, "LSUB completed");
    }
    let list_store = store.clone();
    let records = match tokio::task::spawn_blocking(move || list_store.list_mailboxes(account_id)).await
    {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => {
            tracing::warn!(error = %e, "LSUB list_mailboxes failed");
            return tagged(tag, Status::No, Some("SERVERBUG"), "lsub failed");
        }
        Err(e) => {
            tracing::warn!(error = %e, "LSUB spawn_blocking panicked");
            return tagged(tag, Status::No, Some("SERVERBUG"), "lsub failed");
        }
    };
    let names = match tokio::task::spawn_blocking(move || store.imap_subscription_names(account_id)).await {
        Ok(Ok(names)) => names,
        _ => return tagged(tag, Status::No, Some("UNAVAILABLE"), "subscriptions unavailable"),
    };
    let set = list::subscription_view(&records, &names);
    for om in &set.omitted {
        tracing::warn!(
            mailbox_id = ?om.id,
            stored_name = %om.stored_name,
            reason = ?om.reason,
            "mailbox omitted from LSUB: not representable over IMAP"
        );
    }
    let mut out = String::new();
    for enc in &set.mailboxes {
        if !enc.is_subscribed {
            continue;
        }
        if !matches_pattern(&enc.wire_name, &list::canonical_pattern(&parsed.reference, &parsed.pattern)) {
            continue;
        }
        out.push_str(&render_mailbox_line("LSUB", enc));
    }
    out.push_str(&tagged(tag, Status::Ok, None, "LSUB completed"));
    out
}
