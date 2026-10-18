// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Credentials and the token exchange. Basic auth costs the server a
//! bcrypt verify (~480 ms on a cold cache), so it is spent exactly once —
//! on `POST /auth/tokens/issue` — and every later call carries the opaque
//! Bearer token instead. When a server offers no token endpoint (a webd
//! vhost that proxies only `/jmap`), the client falls back to Basic on
//! every request, the Dioxus client's behaviour, but as a deliberate
//! fallback rather than the only mode.

use serde::{Deserialize, Deserializer, Serialize};

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
    /// The account id the token reaches (maild: the numeric row id). Some
    /// servers send it as a JSON number and some as a string; both are kept
    /// as a string.
    #[serde(
        rename = "accountId",
        alias = "account_id",
        deserialize_with = "string_or_number"
    )]
    pub account_id: String,
    /// When the token expires, RFC 3339, when the server says. Optional: a
    /// token without an expiry is still a token.
    #[serde(rename = "expiresAt", alias = "expires_at", default)]
    pub expires_at: Option<String>,
}

/// The request body of the token exchange. Both fields optional server-side;
/// the label names the token in server logs.
#[derive(Serialize)]
pub struct IssueRequest {
    pub label: String,
}

/// An id the server sends as either a JSON string or a JSON number.
fn string_or_number<'de, D: Deserializer<'de>>(de: D) -> Result<String, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Id {
        Text(String),
        Number(i64),
    }
    Ok(match Id::deserialize(de)? {
        Id::Text(text) => text,
        Id::Number(number) => number.to_string(),
    })
}

impl Token {
    /// A parseable expiry, if the server sent one and it parses.
    pub fn expires(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        chrono::DateTime::parse_from_rfc3339(self.expires_at.as_deref()?)
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

    #[test]
    fn reads_a_numeric_account_id_as_a_string() {
        let token: Token =
            serde_json::from_str(r#"{"token":"sZGsOqsYAA","account_id":2,"expires_at":null}"#)
                .unwrap();
        assert_eq!(token.account_id, "2");
    }

    #[test]
    fn reads_the_camel_case_account_id_as_a_number() {
        let token: Token = serde_json::from_str(r#"{"token":"t","accountId":12}"#).unwrap();
        assert_eq!(token.account_id, "12");
    }
}
