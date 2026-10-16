// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! A binding's scope and repeat policy, with the keymap.v1 spellings
//! (`"global"`, `"modal:<name>"`, `"focus:<name>"`; `"ignore"`, `"allow"`).

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Where a binding applies. inputd fires every row globally today; the
/// other scopes are carried so that a row written with one round-trips.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum BindingScope {
    /// Whenever no modal owns input.
    #[default]
    Global,
    /// Only for the named captured modal.
    Modal(String),
    /// When the named focus tag is present.
    FocusTag(String),
}

/// Why a scope spelling was refused.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ScopeError {
    #[error("unknown scope {0:?}")]
    Unknown(String),
    #[error("invalid scope name {0:?}")]
    InvalidName(String),
}

fn validate_scope_name(name: &str) -> Result<(), ScopeError> {
    if name.is_empty()
        || name.len() > 64
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_' | b':' | b'/'))
    {
        return Err(ScopeError::InvalidName(name.to_owned()));
    }
    Ok(())
}

impl fmt::Display for BindingScope {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Global => formatter.write_str("global"),
            Self::Modal(name) => write!(formatter, "modal:{name}"),
            Self::FocusTag(name) => write!(formatter, "focus:{name}"),
        }
    }
}

impl FromStr for BindingScope {
    type Err = ScopeError;

    fn from_str(source: &str) -> Result<Self, ScopeError> {
        if source == "global" {
            return Ok(Self::Global);
        }
        if let Some(name) = source.strip_prefix("modal:").filter(|n| !n.is_empty()) {
            validate_scope_name(name)?;
            return Ok(Self::Modal(name.to_owned()));
        }
        if let Some(name) = source.strip_prefix("focus:").filter(|n| !n.is_empty()) {
            validate_scope_name(name)?;
            return Ok(Self::FocusTag(name.to_owned()));
        }
        Err(ScopeError::Unknown(source.to_owned()))
    }
}

impl Serialize for BindingScope {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for BindingScope {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

/// What a bound row does with an auto-repeat.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepeatPolicy {
    /// Fire once per press: toggles, dialogs.
    #[default]
    Ignore,
    /// Fire again on every repeat: incremental navigation.
    Allow,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_spellings_round_trip() {
        for (text, scope) in [
            ("global", BindingScope::Global),
            ("modal:picker", BindingScope::Modal("picker".into())),
            ("focus:editor", BindingScope::FocusTag("editor".into())),
        ] {
            let json = format!("\"{text}\"");
            let parsed: BindingScope = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, scope);
            assert_eq!(serde_json::to_string(&parsed).unwrap(), json);
        }
        for bad in ["", "Global", "modal:", "focus:a b", "other"] {
            assert!(bad.parse::<BindingScope>().is_err(), "{bad:?}");
        }
    }

    #[test]
    fn repeat_policy_spellings() {
        assert_eq!(serde_json::to_string(&RepeatPolicy::Ignore).unwrap(), "\"ignore\"");
        assert_eq!(serde_json::to_string(&RepeatPolicy::Allow).unwrap(), "\"allow\"");
    }
}
