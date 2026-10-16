// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Errors, typed at the layer they occur. A JMAP method-level error triple
//! (`["error", {type, description}, callId]`) becomes [`Error::Method`]
//! before any response struct is deserialised, so a `serverFail` reads as
//! itself rather than as a missing-field serde failure.

/// The result type of every fallible call in this crate.
pub type Result<T> = std::result::Result<T, Error>;

/// Everything that can go wrong on the wire.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The HTTPS transport itself failed.
    #[error("transport: {0}")]
    Transport(#[from] reqwest::Error),
    /// A non-success HTTP status the caller cannot retry its way out of.
    #[error("HTTP {0}")]
    Status(reqwest::StatusCode),
    /// Authentication was refused (bad credentials, or an expired token
    /// that could not be exchanged again).
    #[error("unauthorized: {0}")]
    Unauthorized(String),
    /// The server answered a method call with an error triple.
    #[error("JMAP {error_type}: {description}")]
    Method {
        /// The triple's `type` (for example `unknownMethod`, `serverFail`).
        error_type: String,
        /// The triple's `description`, when the server sent one.
        description: String,
    },
    /// The response could not be read as the shape the call expects.
    #[error("wire: {0}")]
    Wire(#[from] serde_json::Error),
    /// The server answered, but not with what the protocol needs: an empty
    /// `methodResponses`, a `/set` reply whose `created` map is missing the
    /// id, a Session without the URL templates.
    #[error("protocol: {0}")]
    Protocol(String),
}

impl Error {
    /// An [`Error::Method`] with a type and optional description, as the
    /// error triple carries them.
    pub fn method(error_type: &str, description: Option<&str>) -> Self {
        Self::Method {
            error_type: error_type.to_owned(),
            description: description.unwrap_or_default().to_owned(),
        }
    }
}
