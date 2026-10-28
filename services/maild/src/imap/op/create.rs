// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>

//! CREATE with hierarchical parents and CREATE-SPECIAL-USE. Validate every
//! path segment before mutation. Names must be Net-Unicode (NFC). Existing
//! mailboxes, including INBOX, produce NO; a trailing delimiter is ignored.
//! USE assigns one role to the leaf; role uniqueness is enforced by MailStore.
//! Intermediate parents may remain after a failed create and can be reused
//! on retry. No messages are changed by CREATE.

use std::sync::Arc;

use mds::ContainerId;
use mds::container::UPLOAD_STAGING_NAME;

use crate::imap::mailbox_name::{HIERARCHY_DELIMITER, ResolveResult, resolve};
use crate::imap::op::list;
use crate::imap::response::{Status, tagged};
use crate::mailstore::{MailStore, MailboxRecord, MailboxRole, jmap_attrs_for_create};

type AccountId = i32;

/// One token of the `(USE (...))` argument list, after the leading
/// `(USE (` and trailing `))` have been peeled. We keep the raw
/// `\Flag` text here so the semantic check (known? Inbox-only? unique?)
/// can run after the wire grammar has validated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UseFlag(pub String);

/// Parsed CREATE wire arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Args {
    pub name: String,
    /// RFC 6154 §5.1 `(USE (\Drafts...))` arguments. `None` means the
    /// client didn't send a USE clause at all; `Some(vec![])` means
    /// they sent `(USE ())`, which RFC 6154 allows but our handler
    /// treats as "no role" (same as `None`).
    pub use_flags: Option<Vec<UseFlag>>,
}

/// Parse `CREATE <mailbox-name> [(USE (\Flag...))]`. The mailbox-name
/// is a single atom or quoted-string; the optional USE clause is the
/// RFC 6154 §5.1 wire form. Trailing tokens after either are `BAD`.
pub fn parse_args(args: &str) -> Result<Args, String> {
    let s = args.trim_start();
    if s.is_empty() {
        return Err("CREATE requires a mailbox name".to_string());
    }
    let (name, rest) = list::take_mailbox_name(s)?;
    if name.is_empty() {
        return Err("CREATE requires a non-empty mailbox name".to_string());
    }
    let rest = rest.trim_start();
    if rest.is_empty() {
        return Ok(Args {
            name,
            use_flags: None,
        });
    }
    let (use_flags, tail) = parse_use_clause(rest)?;
    let tail = tail.trim();
    if !tail.is_empty() {
        return Err("CREATE trailing tokens after USE clause".to_string());
    }
    Ok(Args {
        name,
        use_flags: Some(use_flags),
    })
}

/// Parse the RFC 6154 §5.1 `(USE (\Flag...))` wire form, returning the
/// flag list and the unconsumed tail. Caller already trimmed leading
/// whitespace.
fn parse_use_clause(s: &str) -> Result<(Vec<UseFlag>, &str), String> {
    let s = s
        .strip_prefix('(')
        .ok_or_else(|| "CREATE: expected '(' starting parameter list".to_string())?
        .trim_start();
    let s = if s.len() >= 3 && s.as_bytes()[..3].eq_ignore_ascii_case(b"USE") {
        &s[3..]
    } else {
        return Err("CREATE: only the USE parameter is supported".to_string());
    };
    let s = s.trim_start();
    let s = s
        .strip_prefix('(')
        .ok_or_else(|| "CREATE USE: expected '(' starting flag list".to_string())?;
    let mut flags = Vec::new();
    let mut rest = s;
    loop {
        rest = rest.trim_start();
        if let Some(after) = rest.strip_prefix(')') {
            rest = after;
            break;
        }
        let end = rest
            .find([' ', ')'])
            .ok_or_else(|| "CREATE USE: unterminated flag list".to_string())?;
        let flag = &rest[..end];
        if flag.is_empty() {
            return Err("CREATE USE: empty flag token".to_string());
        }
        if !flag.starts_with('\\') {
            return Err(format!("CREATE USE: flag {flag:?} must start with '\\\\'"));
        }
        flags.push(UseFlag(flag.to_string()));
        rest = &rest[end..];
    }
    let rest = rest.trim_start();
    let rest = rest
        .strip_prefix(')')
        .ok_or_else(|| "CREATE: expected ')' ending parameter list".to_string())?;
    Ok((flags, rest))
}

/// Resolve a parsed `Option<Vec<UseFlag>>` into the role we should
/// assign to the leaf segment. `None` (or `Some(empty)`) → no role.
/// Multiple flags, unknown flags, and `\Inbox` are all `Err(message)`
/// for the caller to wrap in `NO [USEATTR]`.
fn role_from_use_flags(use_flags: &Option<Vec<UseFlag>>) -> Result<Option<MailboxRole>, String> {
    let Some(flags) = use_flags else {
        return Ok(None);
    };
    if flags.is_empty() {
        return Ok(None);
    }
    if flags.len() > 1 {
        return Err(format!(
            "only one SPECIAL-USE attribute supported per mailbox (got {})",
            flags.len()
        ));
    }
    let raw = &flags[0].0;
    let role = role_from_special_use_ci(raw)
        .ok_or_else(|| format!("unknown SPECIAL-USE attribute {raw:?}"))?;
    if role == MailboxRole::Inbox {
        return Err("\\Inbox cannot be assigned by CREATE".to_string());
    }
    Ok(Some(role))
}

/// ASCII-case-insensitive `MailboxRole::from_special_use`. RFC 9051 §1.2
/// mandates IMAP keywords are case-insensitive; clients in the wild
/// send `\drafts`, `\DRAFTS`, `\Drafts`. Storage canonicalises to the
/// title-case `\Drafts` form via `MailboxRole::as_special_use`, so this
/// lookup only needs to accept the variants on the wire.
fn role_from_special_use_ci(s: &str) -> Option<MailboxRole> {
    let lower = s.to_ascii_lowercase();
    match lower.as_str() {
        "\\inbox" => Some(MailboxRole::Inbox),
        "\\archive" => Some(MailboxRole::Archive),
        "\\drafts" => Some(MailboxRole::Drafts),
        "\\flagged" => Some(MailboxRole::Flagged),
        "\\important" => Some(MailboxRole::Important),
        "\\junk" => Some(MailboxRole::Junk),
        "\\sent" => Some(MailboxRole::Sent),
        "\\trash" => Some(MailboxRole::Trash),
        "\\all" => Some(MailboxRole::All),
        _ => None,
    }
}

/// Reject a path segment that the wire layer cannot round-trip
/// safely. See module-level docs for the rule list.
///
/// `pub(super)` so `op::rename` reuses the exact same validation —
/// the two commands share the segment grammar.
pub(super) fn validate_segment(seg: &str) -> Result<(), String> {
    if seg.is_empty() {
        return Err("empty mailbox path segment".to_string());
    }
    if !unicode_normalization::is_nfc(seg) || seg.chars().any(|c| (c as u32 & 0xffff) >= 0xfffe || (0xfdd0..=0xfdef).contains(&(c as u32))) {
        return Err("mailbox name must be Net-Unicode (NFC)".into());
    }
    for b in seg.bytes() {
        if b < 0x20 || b == 0x7f {
            return Err(format!(
                "non-ASCII or control byte 0x{b:02x} in mailbox name (UTF8=ACCEPT not advertised)"
            ));
        }
    }
    if seg == UPLOAD_STAGING_NAME {
        return Err(format!("{seg:?} is a reserved mailbox name"));
    }
    Ok(())
}

/// Outcome of comparing `leaf_role` against an existing mailbox we're
/// about to return `OK` for through an idempotent CREATE path. The
/// caller maps `Mismatch` to `NO [USEATTR]`; `Match` and `NoRoleAsked`
/// both let the OK proceed.
enum LeafRoleCheck {
    /// Client didn't ask for any role, or the existing mailbox already
    /// carries the requested role.
    Ok,
    /// Client asked for a role; the existing mailbox carries a
    /// different (or no) role. Returning `OK` here would silently
    /// ignore the requested SPECIAL-USE attribute.
    Mismatch,
}

/// Resolve whether the existing mailbox at `id` already satisfies the
/// requested SPECIAL-USE attribute. Used at every idempotent CREATE
/// return point: bare `INBOX` (handled inline, since INBOX is rejected
/// for non-`\Inbox` roles unconditionally), `Found` + trailing slash,
/// and race-reload `Found`.
fn check_existing_leaf_role(
    records: &[MailboxRecord],
    id: ContainerId,
    leaf_role: Option<MailboxRole>,
) -> LeafRoleCheck {
    let Some(role) = leaf_role else {
        return LeafRoleCheck::Ok;
    };
    let existing = records.iter().find(|r| r.id == id);
    match existing.and_then(|r| r.role) {
        Some(existing_role) if existing_role == role => LeafRoleCheck::Ok,
        _ => LeafRoleCheck::Mismatch,
    }
}

/// Find any *other* mailbox already carrying `role`. Per-account role
/// uniqueness is enforced here — storage has no `(account, role)`
/// unique index and `mailbox_by_role` returns first-match, so allowing
/// a second holder would be a user-visible ambiguity.
///
/// `target_id` is the id of the mailbox we're idempotently re-OKing
/// (when applicable) — a record at that id is excluded from the scan,
/// so `CREATE Drafts/ (USE (\Drafts))` against a pre-existing `Drafts`
/// with `\Drafts` is not self-conflicting. Pass `None` from tail-create
/// paths (the target doesn't exist yet).
fn other_role_holder_exists(
    records: &[MailboxRecord],
    role: MailboxRole,
    target_id: Option<ContainerId>,
) -> bool {
    records
        .iter()
        .any(|r| r.role == Some(role) && Some(r.id) != target_id)
}

/// Fetch the mailbox record set for the account, mapping any
/// storage error to a pre-built tagged response.
async fn load_records(
    store: &Arc<dyn MailStore>,
    account_id: AccountId,
    tag: &str,
) -> Result<Vec<MailboxRecord>, String> {
    let s = store.clone();
    match tokio::task::spawn_blocking(move || s.list_mailboxes(account_id)).await {
        Ok(Ok(r)) => Ok(r),
        Ok(Err(e)) => {
            tracing::warn!(error = %e, "CREATE list_mailboxes failed");
            Err(tagged(tag, Status::No, Some("SERVERBUG"), "create failed"))
        }
        Err(e) => {
            tracing::warn!(error = %e, "CREATE list_mailboxes spawn_blocking panicked");
            Err(tagged(tag, Status::No, Some("SERVERBUG"), "create failed"))
        }
    }
}

/// Execute CREATE for an authenticated session.
pub async fn handle(tag: &str, args: &str, account_id: AccountId, store: Arc<dyn MailStore>) -> String {
    let parsed = match parse_args(args) {
        Ok(parsed) => parsed,
        Err(e) => return tagged(tag, Status::Bad, None, &e),
    };
    let leaf_role = match role_from_use_flags(&parsed.use_flags) {
        Ok(role) => role,
        Err(e) => return tagged(tag, Status::No, Some("USEATTR"), &e),
    };
    let name = parsed.name.strip_suffix(HIERARCHY_DELIMITER).unwrap_or(&parsed.name);
    let segments: Vec<&str> = name.split(HIERARCHY_DELIMITER).collect();
    for segment in &segments {
        if let Err(e) = validate_segment(segment) { return tagged(tag, Status::No, Some("CANNOT"), &e); }
    }
    let mut records = match load_records(&store, account_id, tag).await {
        Ok(records) => records, Err(response) => return response,
    };
    if let Some(role) = leaf_role
        && other_role_holder_exists(&records, role, None) {
        return tagged(tag, Status::No, Some("USEATTR"), "SPECIAL-USE attribute already in use");
    }
    let mut parent = None;
    for (i, segment) in segments.iter().enumerate() {
        let last = i+1 == segments.len();
        let path = segments[..=i].join("/");
        match resolve(&path, &records) {
            ResolveResult::Found(id) => {
                if last {
                    if let LeafRoleCheck::Mismatch = check_existing_leaf_role(&records, id, leaf_role) {
                        return tagged(tag, Status::No, Some("USEATTR"), "existing mailbox has a different SPECIAL-USE attribute");
                    }
                    return tagged(tag, Status::No, Some("ALREADYEXISTS"), "mailbox already exists");
                }
                parent = Some(id);
            }
            ResolveResult::Unrepresentable => return tagged(tag, Status::No, Some("CANNOT"), "mailbox name cannot be represented"),
            ResolveResult::NotFound => {
                if i == 0 && segment.eq_ignore_ascii_case("INBOX") {
                    return tagged(tag, Status::No, Some("SERVERBUG"), "INBOX unavailable");
                }
                let name = segment.to_string();
                let attrs = jmap_attrs_for_create(if last { leaf_role } else { None }, 0, false);
                let creating = store.clone();
                let created = tokio::task::spawn_blocking(move || creating.create_mailbox(account_id, &name, parent, attrs)).await;
                match created {
                    Ok(Ok(id)) => parent = Some(id),
                    Ok(Err(e)) if e.to_string().starts_with("mailbox already exists") && !last => {
                        records = match load_records(&store, account_id, tag).await { Ok(records) => records, Err(response) => return response };
                        match resolve(&path, &records) {
                            ResolveResult::Found(id) => parent = Some(id),
                            _ => return tagged(tag, Status::No, Some("UNAVAILABLE"), "mailbox hierarchy changed; retry"),
                        }
                    }
                    Ok(Err(e)) if e.to_string().starts_with("mailbox special-use") => return tagged(tag, Status::No, Some("USEATTR"), "SPECIAL-USE attribute already in use"),
                    Ok(Err(e)) if e.to_string().starts_with("mailbox already exists") => return tagged(tag, Status::No, Some("ALREADYEXISTS"), "mailbox already exists"),
                    _ => return tagged(tag, Status::No, Some("UNAVAILABLE"), "mailbox creation unavailable"),
                }
                if last { return tagged(tag, Status::Ok, None, "CREATE completed"); }
                records = match load_records(&store, account_id, tag).await { Ok(records) => records, Err(response) => return response };
            }
        }
    }
    tagged(tag, Status::No, Some("CANNOT"), "invalid mailbox path")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_args_atom() {
        let a = parse_args("Archive").unwrap();
        assert_eq!(a.name, "Archive");
        assert!(a.use_flags.is_none());
    }

    #[test]
    fn parse_args_quoted_with_space() {
        let a = parse_args(r#""Archive/2025 plans""#).unwrap();
        assert_eq!(a.name, "Archive/2025 plans");
        assert!(a.use_flags.is_none());
    }

    #[test]
    fn parse_args_with_single_use_flag() {
        let a = parse_args(r"Drafts (USE (\Drafts))").unwrap();
        assert_eq!(a.name, "Drafts");
        let flags = a.use_flags.expect("USE present");
        assert_eq!(flags.len(), 1);
        assert_eq!(flags[0].0, r"\Drafts");
    }

    #[test]
    fn parse_args_with_empty_use_list() {
        let a = parse_args("Archive (USE ())").unwrap();
        assert_eq!(a.name, "Archive");
        assert_eq!(a.use_flags.as_deref(), Some(&[][..]));
    }

    #[test]
    fn parse_args_with_multiple_use_flags() {
        let a = parse_args(r"Foo (USE (\Drafts \Sent))").unwrap();
        assert_eq!(a.use_flags.as_ref().map(|v| v.len()), Some(2));
    }

    #[test]
    fn parse_args_rejects_non_use_parameter() {
        let e = parse_args(r"Foo (RETURN (\Drafts))").unwrap_err();
        assert!(e.contains("USE"), "{e}");
    }

    #[test]
    fn parse_args_rejects_unbalanced_paren() {
        assert!(parse_args(r"Foo (USE (\Drafts").is_err());
        assert!(parse_args(r"Foo (USE (\Drafts)").is_err());
    }

    #[test]
    fn role_from_use_flags_known_flag() {
        let f = Some(vec![UseFlag(r"\Drafts".to_string())]);
        assert_eq!(role_from_use_flags(&f).unwrap(), Some(MailboxRole::Drafts));
    }

    #[test]
    fn role_from_use_flags_rejects_inbox() {
        let f = Some(vec![UseFlag(r"\Inbox".to_string())]);
        let e = role_from_use_flags(&f).unwrap_err();
        assert!(e.contains("Inbox"), "{e}");
    }

    #[test]
    fn role_from_use_flags_rejects_unknown() {
        let f = Some(vec![UseFlag(r"\Bogus".to_string())]);
        assert!(role_from_use_flags(&f).is_err());
    }

    #[test]
    fn parse_args_use_keyword_is_case_insensitive() {
        // RFC 9051 §1.2 — IMAP keywords are case-insensitive.
        let a = parse_args(r"Drafts (use (\Drafts))").unwrap();
        assert_eq!(a.use_flags.as_ref().map(|v| v.len()), Some(1));
        let a = parse_args(r"Drafts (UsE (\Drafts))").unwrap();
        assert_eq!(a.use_flags.as_ref().map(|v| v.len()), Some(1));
    }

    #[test]
    fn role_from_use_flags_flag_is_case_insensitive() {
        let f = Some(vec![UseFlag(r"\drafts".to_string())]);
        assert_eq!(role_from_use_flags(&f).unwrap(), Some(MailboxRole::Drafts));
        let f = Some(vec![UseFlag(r"\DRAFTS".to_string())]);
        assert_eq!(role_from_use_flags(&f).unwrap(), Some(MailboxRole::Drafts));
        let f = Some(vec![UseFlag(r"\InBoX".to_string())]);
        // Still rejected — `\Inbox` is the role we forbid CREATE from
        // assigning, regardless of input casing.
        assert!(role_from_use_flags(&f).is_err());
    }

    #[test]
    fn role_from_use_flags_rejects_multiple() {
        let f = Some(vec![
            UseFlag(r"\Drafts".to_string()),
            UseFlag(r"\Sent".to_string()),
        ]);
        let e = role_from_use_flags(&f).unwrap_err();
        assert!(e.contains("only one"), "{e}");
    }

    #[test]
    fn parse_args_rejects_empty() {
        assert!(parse_args("").is_err());
    }

    #[test]
    fn parse_args_rejects_empty_quoted() {
        assert!(parse_args(r#""""#).is_err());
    }

    #[test]
    fn parse_args_rejects_trailing_tokens() {
        // After the mailbox name, only an RFC 6154 `(USE (...))`
        // clause is accepted; a bare unparenthesised token is `BAD`.
        let e = parse_args("Archive extra").unwrap_err();
        assert!(
            e.contains("USE") || e.contains("'('") || e.contains("trailing"),
            "{e}"
        );
    }

    #[test]
    fn parse_args_rejects_trailing_after_use_clause() {
        let e = parse_args(r"Foo (USE (\Drafts)) leftover").unwrap_err();
        assert!(e.contains("trailing"), "{e}");
    }

    #[test]
    fn validate_segment_allows_printable_ascii() {
        assert!(validate_segment("Archive").is_ok());
        assert!(validate_segment("Folder 1").is_ok());
        assert!(validate_segment("inbox").is_ok());
    }

    #[test]
    fn validate_segment_rejects_empty() {
        assert!(validate_segment("").is_err());
    }

    #[test]
    fn validate_segment_accepts_normalised_unicode() {
        assert!(validate_segment("Caf\u{00e9}").is_ok());
        assert!(validate_segment("Cafe\u{301}").is_err());
    }

    #[test]
    fn validate_segment_accepts_ampersand_after_wire_decoding() {
        assert!(validate_segment("R&D").is_ok());
    }

    #[test]
    fn validate_segment_rejects_reserved() {
        let e = validate_segment(UPLOAD_STAGING_NAME).unwrap_err();
        assert!(e.contains("reserved"), "{e}");
    }

    #[test]
    fn validate_segment_rejects_control_byte() {
        let e = validate_segment("foo\nbar").unwrap_err();
        assert!(e.contains("non-ASCII"), "{e}");
    }
}
