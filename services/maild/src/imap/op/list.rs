// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>

//! LIST-EXTENDED, LIST-STATUS, SPECIAL-USE and hierarchical reference names.
//! Selection options constrain matches; return options annotate them. Mailbox
//! strings are canonical UTF-8 internally; the session supplies the appropriate
//! UTF-8 or modified UTF-7 wire encoding. LSUB reuses positional parsing.

use std::sync::Arc;

use crate::imap::mailbox_name::{encode_all, matches_pattern, format_attrs, quote_mailbox};
use crate::imap::response::{Status, tagged};
use crate::mailstore::MailStore;

type AccountId = i32;

/// Parsed LIST/LSUB positional arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Args {
    pub reference: String,
    pub pattern: String,
}

/// Parse the `reference mailbox-pattern` argument pair shared by
/// `LIST` and `LSUB`. Exposed at module scope so `op::lsub` can
/// reuse the exact same wire grammar without duplicating the
/// parser.
pub fn parse_args(args: &str) -> Result<Args, String> {
    let s = args.trim_start();
    if s.is_empty() {
        return Err("LIST requires reference and mailbox pattern".to_string());
    }
    let (reference, rest) = take_mailbox_token(s)?;
    let rest = rest.trim_start();
    if rest.is_empty() {
        return Err("LIST requires a mailbox pattern".to_string());
    }
    let (pattern, tail) = take_mailbox_token(rest)?;
    let tail = tail.trim();
    if !tail.is_empty() {
        return Err(
            "unexpected arguments after mailbox pattern".to_string(),
        );
    }
    Ok(Args { reference, pattern })
}

/// Like [`take_mailbox_token`] but applies the stricter
/// `mailbox = astring` grammar (RFC 9051 §9). When the token is
/// quoted, defers to [`take_mailbox_token`]. When the token is an
/// unquoted atom, rejects IMAP `atom-specials` — `(` `)` `{` `"` `\`
/// `]` `%` `*` and any control byte — so a glued extension-parameter
/// block like `SELECT INBOX(CONDSTORE)` does not sneak through the
/// space-only tokeniser as one giant atom.
///
/// LIST / LSUB intentionally do *not* use this entry point because
/// their pattern argument legitimately contains the list-wildcards
/// `%` and `*` unquoted; they keep using [`take_mailbox_token`].
pub fn take_mailbox_name(s: &str) -> Result<(String, &str), String> {
    let bytes = s.as_bytes();
    if bytes.is_empty() {
        return Err("missing mailbox name".to_string());
    }
    if bytes[0] == b'"' {
        return take_mailbox_token(s);
    }
    let end = bytes.iter().position(|b| *b == b' ').unwrap_or(bytes.len());
    let atom = &s[..end];
    for b in atom.bytes() {
        let is_special = matches!(b, b'(' | b')' | b'{' | b'"' | b'\\' | b']' | b'%' | b'*')
            || b < 0x20
            || b == 0x7f;
        if is_special {
            return Err(format!(
                "mailbox name contains IMAP atom-special byte 0x{b:02x}; quote the name"
            ));
        }
    }
    Ok((atom.to_string(), &s[end..]))
}

/// Parse one atom or quoted-string from the head of `s`. Returns
/// `(value, remainder)`. Public so `op::lsub` reuses it.
pub fn take_mailbox_token(s: &str) -> Result<(String, &str), String> {
    let bytes = s.as_bytes();
    if bytes.is_empty() {
        return Err("missing argument".to_string());
    }
    if bytes[0] == b'"' {
        let mut out = Vec::new();
        let mut i = 1;
        while i < bytes.len() {
            let c = bytes[i];
            if c == b'\\' {
                i += 1;
                if i >= bytes.len() {
                    return Err("unterminated quoted-string escape".to_string());
                }
                let nc = bytes[i];
                if nc != b'"' && nc != b'\\' {
                    return Err("invalid quoted-string escape".to_string());
                }
                out.push(nc);
                i += 1;
            } else if c == b'"' {
                return String::from_utf8(out).map(|out| (out, &s[i + 1..])).map_err(|_| "invalid UTF-8 string".into());
            } else {
                if c == 0 || c == b'\r' || c == b'\n' { return Err("control byte in mailbox string".into()); }
                out.push(c);
                i += 1;
            }
        }
        Err("unterminated quoted-string".to_string())
    } else {
        let end = bytes.iter().position(|b| *b == b' ').unwrap_or(bytes.len());
        Ok((s[..end].to_string(), &s[end..]))
    }
}

/// Reject patterns containing non-printable-ASCII or control bytes.
/// The Phase 2 wire is ASCII-only (no UTF8=ACCEPT), so a pattern
/// with non-ASCII or control bytes can never match a wire name the
/// encoder will emit. Returning `BAD` instead of silently producing
/// an empty result set keeps the protocol honest with clients that
/// might otherwise mis-frame their next command. Wildcards `*` and
/// `%` are printable so they fall through the guard.
pub fn pattern_is_ascii_clean(pattern: &str) -> bool {
    pattern.chars().all(|c| !c.is_control())
}

/// Resolve the reference name in our single personal namespace. A leading
/// delimiter addresses the root; otherwise the pattern follows the reference.
pub fn canonical_pattern(reference: &str, pattern: &str) -> String {
    if let Some(root) = pattern.strip_prefix('/') { root.to_string() }
    else { format!("{reference}{pattern}") }
}

pub(crate) fn subscription_view(records: &[crate::mailstore::MailboxRecord], subscriptions: &[String]) -> crate::imap::mailbox_name::EncodedSet {
    use crate::imap::mailbox_name::EncodedMailbox;
    let mut set = encode_all(records);
    for name in subscriptions {
        if let Some(enc) = set.mailboxes.iter_mut().find(|enc| enc.wire_name == *name) { enc.is_subscribed = true; }
        else {
            set.mailboxes.push(EncodedMailbox { id: mds::ContainerId(uuid::Uuid::nil()), wire_name: name.clone(), has_children: false, role: None, is_subscribed: true });
        }
        let mut ancestor = name.as_str();
        while let Some((parent, _)) = ancestor.rsplit_once('/') {
            if !set.mailboxes.iter().any(|enc| enc.wire_name == parent) {
                set.mailboxes.push(EncodedMailbox { id: mds::ContainerId(uuid::Uuid::nil()), wire_name: parent.to_string(), has_children: true, role: None, is_subscribed: false });
            }
            ancestor = parent;
        }
    }
    for enc in &mut set.mailboxes {
        enc.has_children |= subscriptions.iter().any(|name| name.starts_with(&format!("{}/", enc.wire_name)));
    }
    set
}

pub(crate) async fn preserve_subscriptions(store: Arc<dyn MailStore>, account_id: AccountId) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        for enc in encode_all(&store.list_mailboxes(account_id)?).mailboxes {
            if enc.is_subscribed { store.imap_set_subscription(account_id, &enc.wire_name, true)?; }
        }
        Ok::<_, anyhow::Error>(())
    }).await.map_err(|e| e.to_string())?.map_err(|e| e.to_string())
}

#[derive(Debug)]
struct ExtendedArgs {
    reference: String,
    patterns: Vec<String>,
    selection: Vec<String>,
    returns: Vec<String>,
    status: Option<String>,
    extended: bool,
}

fn option_list(s: &str) -> Result<(&str, &str), String> {
    let s = s.strip_prefix('(').ok_or("LIST options require parentheses")?;
    let end = s.find(')').ok_or("LIST options missing closing paren")?;
    Ok((&s[..end], s[end+1..].trim_start()))
}

fn parse_extended(args: &str) -> Result<ExtendedArgs, String> {
    let mut rest = args.trim_start();
    let mut extended = rest.starts_with('(');
    let mut selection = Vec::new();
    if rest.starts_with('(') {
        extended = true;
        let (body, tail) = option_list(rest)?;
        for option in body.split_ascii_whitespace() {
            let option = option.to_ascii_uppercase();
            if !["SUBSCRIBED", "REMOTE", "RECURSIVEMATCH", "SPECIAL-USE"].contains(&option.as_str()) { return Err(format!("unknown LIST selection option {option}")); }
            if !selection.contains(&option) { selection.push(option); }
        }
        rest = tail;
    }
    if selection.iter().any(|s| s == "RECURSIVEMATCH") && !selection.iter().any(|s| s == "SUBSCRIBED" || s == "SPECIAL-USE") {
        return Err("RECURSIVEMATCH requires a substantive selection option".into());
    }
    let (reference, tail) = take_mailbox_token(rest)?;
    rest = tail.trim_start();
    let mut patterns = Vec::new();
    if rest.starts_with('(') {
        extended = true;
        rest = &rest[1..];
        loop {
            rest = rest.trim_start();
            if let Some(tail) = rest.strip_prefix(')') { rest = tail.trim_start(); break; }
            let (pattern, tail) = if rest.starts_with('"') { take_mailbox_token(rest)? } else {
                let end = rest.find(|c: char| c.is_ascii_whitespace() || c == ')').unwrap_or(rest.len());
                if end == 0 || rest.is_empty() { return Err("LIST missing closing pattern paren".into()); }
                (rest[..end].to_string(), &rest[end..])
            };
            patterns.push(pattern); rest = tail;
        }
        if patterns.is_empty() { return Err("LIST requires mailbox patterns".into()); }
    } else {
        let (pattern, tail) = take_mailbox_token(rest)?;
        patterns.push(pattern); rest = tail.trim_start();
    }
    let mut returns = Vec::new();
    let mut status = None;
    if !rest.is_empty() {
        extended = true;
        let (keyword, tail) = rest.split_once(' ').ok_or("LIST expected RETURN options")?;
        if !keyword.eq_ignore_ascii_case("RETURN") { return Err("LIST expected RETURN options".into()); }
        rest = tail.trim_start().strip_prefix('(').ok_or("LIST RETURN requires parentheses")?;
        loop {
            rest = rest.trim_start();
            if let Some(tail) = rest.strip_prefix(')') { rest = tail.trim(); break; }
            let end = rest.find(|c: char| c.is_ascii_whitespace() || c == ')').unwrap_or(rest.len());
            if end == 0 { return Err("LIST RETURN missing closing paren".into()); }
            let option = rest[..end].to_ascii_uppercase();
            rest = rest[end..].trim_start();
            match option.as_str() {
                "SUBSCRIBED" | "CHILDREN" | "SPECIAL-USE" => returns.push(option),
                "STATUS" => {
                    let (items, tail) = option_list(rest)?;
                    super::status::parse_args(&format!("INBOX ({items})"))?;
                    status = Some(items.to_string()); rest = tail;
                }
                _ => return Err(format!("unknown LIST return option {option}")),
            }
        }
    }
    if !rest.is_empty() { return Err("LIST trailing arguments".into()); }
    if !pattern_is_ascii_clean(&reference) || patterns.iter().any(|p| !pattern_is_ascii_clean(p)) { return Err("control bytes in LIST name".into()); }
    Ok(ExtendedArgs { reference, patterns, selection, returns, status, extended })
}

/// Execute LIST for an authenticated session.
///
/// Wraps the synchronous `MailStore::list_mailboxes` call in
/// `spawn_blocking` so the per-connection async task does not park
/// a tokio worker on the SQLite read. Mirrors the JMAP
/// `Mailbox/get` / `Mailbox/query` shape.
pub async fn handle(
    tag: &str,
    args: &str,
    account_id: AccountId,
    store: Arc<dyn MailStore>,
) -> String {
    handle_with_limit(tag, args, account_id, store, crate::imap::config::ImapConfig::default().max_literal_bytes).await
}

pub async fn handle_with_limit(tag: &str, args: &str, account_id: AccountId, store: Arc<dyn MailStore>, append_limit: u64) -> String {
    let parsed = match parse_extended(args) {
        Ok(p) => p,
        Err(e) => return tagged(tag, Status::Bad, None, &e),
    };
    // Hierarchy-delimiter probe: `LIST "" ""` per RFC 9051 §6.3.9
    // returns one row carrying the root delimiter and `\Noselect`.
    if !parsed.extended && parsed.patterns.len() == 1 && parsed.patterns[0].is_empty() {
        let mut out = "* LIST (\\Noselect) \"/\" \"\"\r\n".to_string();
        out.push_str(&tagged(tag, Status::Ok, None, "LIST completed"));
        return out;
    }
    let list_store = store.clone();
    let records = match tokio::task::spawn_blocking(move || list_store.list_mailboxes(account_id)).await
    {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => {
            tracing::warn!(error = %e, "LIST list_mailboxes failed");
            return tagged(tag, Status::No, Some("SERVERBUG"), "list failed");
        }
        Err(e) => {
            tracing::warn!(error = %e, "LIST spawn_blocking panicked");
            return tagged(tag, Status::No, Some("SERVERBUG"), "list failed");
        }
    };
    let subscriptions_store = store.clone();
    let subscriptions = match tokio::task::spawn_blocking(move || subscriptions_store.imap_subscription_names(account_id)).await {
        Ok(Ok(names)) => names,
        _ => return tagged(tag, Status::No, Some("UNAVAILABLE"), "subscriptions unavailable"),
    };
    let set = subscription_view(&records, &subscriptions);
    for om in &set.omitted {
        tracing::warn!(
            mailbox_id = ?om.id,
            stored_name = %om.stored_name,
            reason = ?om.reason,
            "mailbox omitted from LIST: not representable over IMAP"
        );
    }
    let mut out = String::new();
    let selected = |enc: &crate::imap::mailbox_name::EncodedMailbox| {
        (!parsed.selection.iter().any(|s| s == "SUBSCRIBED") || enc.is_subscribed)
        && (!parsed.selection.iter().any(|s| s == "SPECIAL-USE") || enc.role.is_some())
    };
    let recursive = parsed.selection.iter().any(|s| s == "RECURSIVEMATCH");
    for enc in &set.mailboxes {
        let missing = enc.id.0.is_nil();
        if missing && !parsed.selection.iter().any(|s| s == "SUBSCRIBED") { continue; }
        if !parsed.patterns.iter().filter(|p| !p.is_empty()).any(|p| matches_pattern(&enc.wire_name, &canonical_pattern(&parsed.reference, p))) { continue; }
        let child_match = recursive && set.mailboxes.iter().any(|child| child.wire_name.starts_with(&format!("{}/", enc.wire_name)) && selected(child));
        if !selected(enc) && !child_match { continue; }
        let mut attrs = format_attrs(enc);
        if missing { attrs.push_str(" \\NonExistent"); }
        if (parsed.returns.iter().any(|s| s == "SUBSCRIBED") || parsed.selection.iter().any(|s| s == "SUBSCRIBED")) && enc.is_subscribed { attrs.push_str(" \\Subscribed"); }
        // With RECURSIVEMATCH, CHILDINFO describes the descendants that
        // satisfied the selection even when the parent itself did not.
        let childinfo = if child_match { format!(" (CHILDINFO ({}))", parsed.selection.iter().filter(|s| s.as_str() == "SUBSCRIBED" || s.as_str() == "SPECIAL-USE").map(|s| quote_mailbox(s)).collect::<Vec<_>>().join(" ")) } else { String::new() };
        out.push_str(&format!("* LIST ({attrs}) \"/\" {}{childinfo}\r\n", quote_mailbox(&enc.wire_name)));
        if let Some(items) = &parsed.status && !missing {
            let status = super::status::handle_with_limit(tag, &format!("{} ({items})", quote_mailbox(&enc.wire_name)), account_id, store.clone(), append_limit).await;
            let Some(line) = status.lines().find(|l| l.starts_with("* STATUS ")) else { return out + &tagged(tag, Status::No, Some("UNAVAILABLE"), "LIST STATUS unavailable"); };
            out.push_str(line); out.push_str("\r\n");
        }
    }
    out.push_str(&tagged(tag, Status::Ok, None, "LIST completed"));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_args_empty_reference_with_star_pattern() {
        let p = parse_args(r#""" *"#).unwrap();
        assert_eq!(p.reference, "");
        assert_eq!(p.pattern, "*");
    }

    #[test]
    fn parse_args_quoted_pattern_with_spaces_is_one_token() {
        let p = parse_args(r#""" "INBOX/With Space""#).unwrap();
        assert_eq!(p.reference, "");
        assert_eq!(p.pattern, "INBOX/With Space");
    }

    #[test]
    fn parse_args_atom_reference_atom_pattern() {
        let p = parse_args("\"\" INBOX").unwrap();
        assert_eq!(p.reference, "");
        assert_eq!(p.pattern, "INBOX");
    }

    #[test]
    fn parse_args_handles_quoted_escapes() {
        let p = parse_args(r#""" "a\"b\\c""#).unwrap();
        assert_eq!(p.pattern, "a\"b\\c");
    }

    #[test]
    fn parse_args_rejects_missing_pattern() {
        let e = parse_args(r#""""#).unwrap_err();
        assert!(e.contains("mailbox pattern"), "{e}");
    }

    #[test]
    fn parse_args_rejects_trailing_extension_parameters() {
        let e = parse_args(r#""" * RETURN (SUBSCRIBED)"#).unwrap_err();
        assert!(e.contains("unexpected"), "{e}");
    }

    #[test]
    fn parse_args_rejects_unterminated_quoted_string() {
        let e = parse_args(r#""" "INBOX"#).unwrap_err();
        assert!(e.contains("unterminated"), "{e}");
    }
}
