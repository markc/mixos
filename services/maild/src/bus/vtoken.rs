// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>

//! Opaque token management for every Bus caller. Plaintext tokens are returned
//! once at mint time; browse responses contain only stored metadata and HMACs.

use std::sync::Arc;

use ::bus::native_client::IncomingCommand;
use serde_json::Value;

use crate::vtoken::{OpaqueRow, VtokenStore};

const RC_ERROR: u8 = 10;

/// Shared state for the `maild.vtoken.*` commands.
#[derive(Clone)]
pub struct VtokenBusState {
    pub store: Arc<VtokenStore>,
}

/// Dispatch token commands for every caller. Arguments use the same shape
/// as the other maild commands; no delegation envelope is required.
pub async fn dispatch(action: &str, cmd: &IncomingCommand, state: &VtokenBusState) -> (u8, String) {
    match action {
        // Opaque commands — the sender-locked single-segment namespace.
        "mint_opaque" => handle_mint_opaque(cmd, state).await,
        "list_opaque" => handle_list_opaque(state).await,
        "lookup_opaque" => handle_lookup_opaque(cmd, state).await,
        "disable_opaque" => handle_disable_opaque(cmd, state).await,
        other => (
            RC_ERROR,
            err_body(&format!("unknown vtoken action: {other}")),
        ),
    }
}

// ─── Opaque handlers ───────────────────────────────────────────

/// `maild.vtoken.mint_opaque` — mint an OPAQUE, sender-locked, single-segment
/// token. Args: `account`, `real_email`, `verification_strength`
/// (`local-auth`|`external-dmarc`), `service`; optional `allowed_sender`
/// (default = `real_email`) and `active` (default true). The recipient domain is
/// the account's domain. The plaintext token + address are returned ONCE; only
/// the HMAC is stored.
async fn handle_mint_opaque(cmd: &IncomingCommand, state: &VtokenBusState) -> (u8, String) {
    let args = match super::try_resolve_args(cmd) {
        Ok(v) => v,
        Err(e) => return (RC_ERROR, err_body(&format!("invalid args: {e}"))),
    };
    let active = match opt_bool(&args, "active", true) {
        Ok(b) => b,
        Err(e) => return (RC_ERROR, err_body(&e)),
    };
    mint_opaque_common(&args, active, state).await
}

/// Parse the target data and mint a sender-locked token.
async fn mint_opaque_common(args: &Value, active: bool, state: &VtokenBusState) -> (u8, String) {
    let account = match req_email(args, "account") {
        Ok(s) => s,
        Err(e) => return (RC_ERROR, err_body(&e)),
    };
    let real_email = match req_email(args, "real_email") {
        Ok(s) => s,
        Err(e) => return (RC_ERROR, err_body(&e)),
    };
    // allowed_sender defaults to real_email (the owner posts from their own
    // verified address); an explicit value sets a different sender-lock. The
    // address itself is validated inside `mint_opaque` (canonical_addr).
    let allowed_sender = match opt_str(args, "allowed_sender") {
        Ok(Some(s)) => s,
        Ok(None) => real_email.clone(),
        Err(e) => return (RC_ERROR, err_body(&e)),
    };
    let verification_strength = match req_str(args, "verification_strength") {
        Ok(s) => s,
        Err(e) => return (RC_ERROR, err_body(&e)),
    };
    let service = match req_str(args, "service") {
        Ok(s) => s,
        Err(e) => return (RC_ERROR, err_body(&e)),
    };
    // The recipient domain (where the `<token>@domain` address lives) is the
    // account's domain — `req_email` guaranteed exactly one '@'.
    let domain = account.rsplit_once('@').map(|(_, d)| d).unwrap_or("");
    match state
        .store
        .mint_opaque(
            domain,
            &account,
            &real_email,
            &allowed_sender,
            &verification_strength,
            &service,
            active,
        )
        .await
    {
        Ok(m) => (
            0,
            serde_json::json!({
                "token_hmac": m.token_hmac,
                "token": m.token,
                "address": format!("{}@{}", m.token, m.domain),
                "domain": m.domain,
                "account": account,
                "service": service,
                "allowed_sender": allowed_sender,
                "verification_strength": verification_strength,
                "active": active,
                "minted": true,
                "ok": true,
            })
            .to_string(),
        ),
        Err(e) => (RC_ERROR, err_body(&format!("mint_opaque failed: {e}"))),
    }
}

/// `maild.vtoken.list_opaque` — every opaque token row. No secret to redact (the
/// plaintext token is never stored, only its HMAC).
async fn handle_list_opaque(state: &VtokenBusState) -> (u8, String) {
    match state.store.list_opaque().await {
        Ok(rows) => {
            let out: Vec<Value> = rows.iter().map(opaque_row_json).collect();
            (
                0,
                serde_json::json!({ "tokens": out, "count": out.len() }).to_string(),
            )
        }
        Err(e) => (RC_ERROR, err_body(&format!("list_opaque failed: {e}"))),
    }
}

/// `maild.vtoken.lookup_opaque` — args `token_hmac` (the row id from list/mint).
async fn handle_lookup_opaque(cmd: &IncomingCommand, state: &VtokenBusState) -> (u8, String) {
    let args = match super::try_resolve_args(cmd) {
        Ok(v) => v,
        Err(e) => return (RC_ERROR, err_body(&format!("invalid args: {e}"))),
    };
    let token_hmac = match req_str(&args, "token_hmac") {
        Ok(s) => s,
        Err(e) => return (RC_ERROR, err_body(&e)),
    };
    match state.store.lookup_opaque_by_hmac(&token_hmac).await {
        Ok(Some(row)) => (
            0,
            serde_json::json!({ "token": opaque_row_json(&row) }).to_string(),
        ),
        Ok(None) => (
            RC_ERROR,
            err_body(&format!("unknown token_hmac: {token_hmac}")),
        ),
        Err(e) => (RC_ERROR, err_body(&format!("lookup_opaque failed: {e}"))),
    }
}

/// `maild.vtoken.disable_opaque` — args `token_hmac`. Revoke (active=0).
async fn handle_disable_opaque(cmd: &IncomingCommand, state: &VtokenBusState) -> (u8, String) {
    let args = match super::try_resolve_args(cmd) {
        Ok(v) => v,
        Err(e) => return (RC_ERROR, err_body(&format!("invalid args: {e}"))),
    };
    let token_hmac = match req_str(&args, "token_hmac") {
        Ok(s) => s,
        Err(e) => return (RC_ERROR, err_body(&e)),
    };
    match state.store.disable_opaque(&token_hmac).await {
        Ok(true) => (
            0,
            serde_json::json!({ "token_hmac": token_hmac, "disabled": true, "ok": true })
                .to_string(),
        ),
        Ok(false) => (
            RC_ERROR,
            err_body(&format!("unknown token_hmac: {token_hmac}")),
        ),
        Err(e) => (RC_ERROR, err_body(&format!("disable_opaque failed: {e}"))),
    }
}

/// An opaque token row as JSON for the browse surface (`list_opaque`/
/// `lookup_opaque`). No secret to redact — the plaintext token is never stored,
/// only its HMAC; `token_hmac` is the stable, one-way row id.
fn opaque_row_json(row: &OpaqueRow) -> Value {
    serde_json::json!({
        "token_hmac": row.token_hmac,
        "domain": row.domain,
        "account": row.account,
        "real_email": row.real_email,
        "allowed_sender": row.allowed_sender,
        "verification_strength": row.verification_strength,
        "token_len": row.token_len,
        "service": row.service,
        "active": row.active,
    })
}

/// Read an optional string arg, STRICTLY: absent/null → `None`; a present
/// non-empty string → `Some`; a present-but-EMPTY string or a present
/// non-string → an **error**. The strictness matters because `add`/`rotate`
/// switch mode on presence — treating `user_id=""` as "absent" would let
/// malformed explicit input silently fall through to the mint path and create
/// a live token. "Omit it entirely to mint" is the only way to mean absent.
fn opt_str(args: &Value, key: &str) -> Result<Option<String>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) if !s.is_empty() => Ok(Some(s.clone())),
        Some(Value::String(_)) => Err(format!(
            "{key} must not be empty (omit it entirely to mint a token)"
        )),
        Some(_) => Err(format!("{key} must be a string")),
    }
}

/// Require a string with EXACTLY one `@` and a non-empty local + domain — used
/// for `account`/`real_email` so `build_addresses` derives a sane domain and an
/// operator typo (`a@b@c`, `@x`, `x@`) can't mint a token with a surprising or
/// empty domain.
fn req_email(args: &Value, key: &str) -> Result<String, String> {
    let s = req_str(args, key)?;
    match s.split_once('@') {
        Some((local, domain))
            if !local.is_empty() && !domain.is_empty() && !domain.contains('@') =>
        {
            Ok(s)
        }
        _ => Err(format!(
            "{key} must be a valid email address (one '@', non-empty parts)"
        )),
    }
}

/// Read an optional boolean field STRICTLY: absent/null → `default`; a real
/// bool → its value; present-but-any-other-type → an error (no silent coercion
/// for a safety-sensitive flag).
fn opt_bool(args: &Value, key: &str, default: bool) -> Result<bool, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(Value::Bool(b)) => Ok(*b),
        Some(_) => Err(format!("{key} must be a boolean")),
    }
}

/// Require a non-empty string field from the args object.
fn req_str(args: &Value, key: &str) -> Result<String, String> {
    match args.get(key).and_then(Value::as_str) {
        Some(s) if !s.is_empty() => Ok(s.to_string()),
        _ => Err(format!("missing or empty required field: {key}")),
    }
}

fn err_body(msg: &str) -> String {
    serde_json::json!({ "error": msg }).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vtoken::VtokenStore;
    use std::collections::BTreeMap;

    async fn state() -> (VtokenBusState, tempfile::TempDir) {
        let tmp = tempfile::TempDir::new().unwrap();
        let store = VtokenStore::open(tmp.path().join("v.db")).await.unwrap();
        (
            VtokenBusState {
                store: Arc::new(store),
            },
            tmp,
        )
    }

    /// A request from `from` with a JSON body (the `client.call` shape the Mix
    /// CLI produces). An empty body models a no-arg command.
    fn cmd(from: &str, body: Value) -> IncomingCommand {
        IncomingCommand {
            generation: 0,
            from: from.to_string(),
            command: "maild.vtoken.test".to_string(),
            id: None,
            args: Value::Null,
            body: if body.is_null() {
                String::new()
            } else {
                body.to_string()
            },
            headers: BTreeMap::new(),
        }
    }

    // ── Opaque commands ────────────────────────────────────────────────

    #[tokio::test]
    async fn any_caller_mint_opaque_roundtrip_list_lookup_disable() {
        let (st, _tmp) = state().await;
        let body = serde_json::json!({
            "account": "blog@x.org", "real_email": "m@y.org",
            "verification_strength": "external-dmarc", "service": "posts"
        });
        let (rc, out) = dispatch("mint_opaque", &cmd("unlisted-client", body), &st).await;
        assert_eq!(rc, 0, "{out}");
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["minted"], serde_json::json!(true));
        let token = v["token"].as_str().unwrap().to_string();
        let hmac = v["token_hmac"].as_str().unwrap().to_string();
        assert_eq!(token.len(), 10, "external-dmarc → L=10");
        // allowed_sender defaulted to real_email; address uses the account domain.
        assert_eq!(v["allowed_sender"], serde_json::json!("m@y.org"));
        assert_eq!(v["address"], serde_json::json!(format!("{token}@x.org")));

        // list_opaque shows the row and NEVER leaks the plaintext token.
        let (rc, lout) = dispatch("list_opaque", &cmd("webd", Value::Null), &st).await;
        assert_eq!(rc, 0);
        assert!(
            !lout.contains(&token),
            "list_opaque leaked the plaintext token"
        );
        let lv: Value = serde_json::from_str(&lout).unwrap();
        assert_eq!(lv["count"], serde_json::json!(1));

        // lookup by token_hmac.
        let (rc, kout) = dispatch(
            "lookup_opaque",
            &cmd("", serde_json::json!({"token_hmac": hmac})),
            &st,
        )
        .await;
        assert_eq!(rc, 0, "{kout}");
        let kv: Value = serde_json::from_str(&kout).unwrap();
        assert_eq!(kv["token"]["service"], serde_json::json!("posts"));
        assert_eq!(kv["token"]["active"], serde_json::json!(true));

        // disable → inactive on the next lookup.
        let (rc, _) = dispatch(
            "disable_opaque",
            &cmd("another-client", serde_json::json!({"token_hmac": hmac})),
            &st,
        )
        .await;
        assert_eq!(rc, 0);
        let (_, kout2) = dispatch(
            "lookup_opaque",
            &cmd("", serde_json::json!({"token_hmac": hmac})),
            &st,
        )
        .await;
        let kv2: Value = serde_json::from_str(&kout2).unwrap();
        assert_eq!(kv2["token"]["active"], serde_json::json!(false));
    }
}
