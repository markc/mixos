// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Action ids: the Bus command a binding fires.
//!
//! An id is a `&'static str`. Constants use [`ActionId::from_static`]; ids
//! read from a keymap file or a Bus body are validated and interned for the
//! life of the process by [`ActionId::intern`], under a fixed ceiling so
//! untrusted input cannot grow the interner without bound.

use std::collections::HashSet;
use std::fmt;
use std::ops::Deref;
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Maximum UTF-8 byte length of an action id.
pub const MAX_ACTION_ID_LEN: usize = 128;

/// Maximum number of dynamically interned action ids in one process. Static
/// ids do not count; re-reading an interned id is free.
pub const MAX_INTERNED_ACTION_IDS: usize = 16_384;

static INTERNED: OnceLock<Mutex<HashSet<&'static str>>> = OnceLock::new();

/// A stable, process-lifetime action name.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ActionId(&'static str);

impl ActionId {
    /// An id from a program-owned literal. Panics at compile time (in a
    /// const context) on an empty, oversized or ill-formed id.
    pub const fn from_static(id: &'static str) -> Self {
        let bytes = id.as_bytes();
        assert!(!bytes.is_empty(), "action id must not be empty");
        assert!(
            bytes.len() <= MAX_ACTION_ID_LEN,
            "action id exceeds maximum length"
        );
        let mut index = 0;
        while index < bytes.len() {
            assert!(id_byte(bytes[index]), "invalid character in action id");
            index += 1;
        }
        Self(id)
    }

    /// Validate a runtime string and intern it for the rest of the process.
    pub fn intern(id: &str) -> Result<Self, ActionIdError> {
        validate(id)?;
        let mut interned = INTERNED
            .get_or_init(|| Mutex::new(HashSet::new()))
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(existing) = interned.get(id) {
            return Ok(Self(existing));
        }
        if interned.len() >= MAX_INTERNED_ACTION_IDS {
            return Err(ActionIdError::InternLimit {
                maximum: MAX_INTERNED_ACTION_IDS,
            });
        }
        let leaked: &'static str = Box::leak(id.to_owned().into_boxed_str());
        interned.insert(leaked);
        Ok(Self(leaked))
    }

    /// Validate an id without interning it.
    pub fn validate_str(id: &str) -> Result<(), ActionIdError> {
        validate(id)
    }

    /// The id's text.
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

const fn id_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_' | b':' | b'/')
}

fn validate(id: &str) -> Result<(), ActionIdError> {
    if id.is_empty() {
        return Err(ActionIdError::Empty);
    }
    if id.len() > MAX_ACTION_ID_LEN {
        return Err(ActionIdError::TooLong {
            length: id.len(),
            maximum: MAX_ACTION_ID_LEN,
        });
    }
    if !id.bytes().all(id_byte) {
        return Err(ActionIdError::InvalidCharacter);
    }
    Ok(())
}

/// Why a runtime action id was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ActionIdError {
    #[error("action id must not be empty")]
    Empty,
    #[error("action id may contain only ASCII letters, digits, '.', '-', '_', ':' and '/'")]
    InvalidCharacter,
    #[error("action id is {length} bytes; maximum is {maximum}")]
    TooLong { length: usize, maximum: usize },
    #[error("dynamic action-id interner limit of {maximum} reached")]
    InternLimit { maximum: usize },
}

impl fmt::Debug for ActionId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_tuple("ActionId").field(&self.0).finish()
    }
}

impl fmt::Display for ActionId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0)
    }
}

impl Deref for ActionId {
    type Target = str;

    fn deref(&self) -> &str {
        self.0
    }
}

impl Serialize for ActionId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.0)
    }
}

impl<'de> Deserialize<'de> for ActionId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let id = String::deserialize(deserializer)?;
        Self::intern(&id).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grammar_matches_the_legacy_vocabulary() {
        for good in ["user.f01", "desktop.workspace.next", "a", "x:y/z_w-1"] {
            assert_eq!(ActionId::validate_str(good), Ok(()), "{good}");
        }
        assert_eq!(ActionId::validate_str(""), Err(ActionIdError::Empty));
        assert_eq!(
            ActionId::validate_str("a b"),
            Err(ActionIdError::InvalidCharacter)
        );
        assert!(matches!(
            ActionId::validate_str(&"a".repeat(MAX_ACTION_ID_LEN + 1)),
            Err(ActionIdError::TooLong { .. })
        ));
    }

    #[test]
    fn interning_returns_the_same_text() {
        let a = ActionId::intern("inputd.test.intern").unwrap();
        let b = ActionId::intern("inputd.test.intern").unwrap();
        assert_eq!(a, b);
        assert!(std::ptr::eq(a.as_str(), b.as_str()), "one allocation");
        assert_eq!(
            ActionId::intern("bad id"),
            Err(ActionIdError::InvalidCharacter)
        );
    }

    #[test]
    fn serde_is_a_plain_string() {
        let id = ActionId::from_static("scenes.editor.open");
        assert_eq!(
            serde_json::to_string(&id).unwrap(),
            "\"scenes.editor.open\""
        );
        let back: ActionId = serde_json::from_str("\"scenes.editor.open\"").unwrap();
        assert_eq!(back, id);
        assert!(serde_json::from_str::<ActionId>("\"no spaces\"").is_err());
    }
}
