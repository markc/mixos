// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Credentials and the token exchange. Basic auth costs the server a
//! bcrypt verify (~480 ms on a cold cache), so it is spent exactly once —
//! on `POST /auth/tokens/issue` — and every later call carries the opaque
//! Bearer token instead. When a server offers no token endpoint (a webd
//! vhost that proxies only `/jmap`), the client falls back to Basic on
//! every request, the Dioxus client's behaviour, but as a deliberate
//! fallback rather than the only mode.

use serde::{Deserialize, Serialize};

/// How an account reaches its server.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    /// The server base URL, no trailing slash (for example the maild
    /// HTTPS listener, or a webd vhost that proxies `/jmap`).
    pub url: String,
    /// The login address; the Basic username at the token endpoint.
    pub email: String,
    /// The account password. Held for the token exchange and re-exchanges;
    /// never sent on a method call when a token is held.
    pub password: String,
    /// A PEM bundle to trust in addition to the system roots, for servers
    /// whose certificates are self-issued. Per-account, never a global
    /// accept-anything.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ca_pem: Option<String>,
}

/// A server-issued bearer token, as `/auth/tokens/issue` returns it.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Token {
    /// The opaque token. Shown once by the server; this is the only copy.
    pub token: String,
    /// The account id the token reaches (maild: the numeric row id as a
    /// string).
    #[serde(rename = "accountId", alias = "account_id")]
    pub account_id: String,
    /// When the token expires, RFC 3339.
    #[serde(rename = "expiresAt", alias = "expires_at")]
    pub expires_at: String,
}

/// The request body of the token exchange. Both fields optional server-side;
/// the label names the token in server logs.
#[derive(Serialize)]
pub struct IssueRequest {
    pub label: String,
}

impl Token {
    /// A parseable expiry, if the server sent one and it parses.
    pub fn expires(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        chrono::DateTime::parse_from_rfc3339(&self.expires_at)
            .ok()
            .map(|dt| dt.with_timezone(&chrono::Utc))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_issue_response() {
        let token: Token = serde_json::from_str(
            r#"{"token":"sZGsOqsYAA","account_id":"7","issued_at":"2026-10-13T06:00:00Z","expires_at":"2026-11-12T06:00:00Z"}"#,
        )
        .unwrap();
        assert_eq!(token.account_id, "7");
        assert!(token.expires().is_some());
    }
}
