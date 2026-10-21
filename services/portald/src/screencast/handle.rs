// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Request and session object paths. Both are derived from the caller's unique
//! name and a caller-chosen token, as the portal specification requires, so a
//! client can subscribe to `Response` before it makes the call.
use crate::portal::PortalError;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use zbus::zvariant::{OwnedObjectPath, OwnedValue};

pub const REQUEST_ROOT: &str = "/org/freedesktop/portal/desktop/request";
pub const SESSION_ROOT: &str = "/org/freedesktop/portal/desktop/session";

/// `:1.42` becomes `1_42`.
pub fn sender_segment(unique: &str) -> String {
    unique.trim_start_matches(':').replace('.', "_")
}

/// A token is one object-path element: ASCII letters, digits and `_`.
pub fn valid_token(token: &str) -> bool {
    !token.is_empty()
        && token.len() <= 64
        && token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

fn path(root: &str, sender: &str, token: &str) -> Result<OwnedObjectPath, PortalError> {
    OwnedObjectPath::try_from(format!("{root}/{}/{token}", sender_segment(sender)))
        .map_err(|error| PortalError::InvalidArgument(format!("bad object path: {error}")))
}

/// The token named by `key` in `options`, or a generated one when absent.
pub fn token(
    options: &HashMap<String, OwnedValue>,
    key: &str,
    counter: &AtomicU64,
) -> Result<String, PortalError> {
    match options.get(key) {
        None => Ok(format!(
            "portald{}",
            counter.fetch_add(1, Ordering::Relaxed)
        )),
        Some(value) => {
            let token = value
                .downcast_ref::<&str>()
                .map_err(|_| PortalError::InvalidArgument(format!("{key} must be a string")))?;
            if !valid_token(token) {
                return Err(PortalError::InvalidArgument(format!(
                    "{key} must be 1-64 ASCII letters, digits or underscores"
                )));
            }
            Ok(token.to_owned())
        }
    }
}

pub fn request_path(sender: &str, token: &str) -> Result<OwnedObjectPath, PortalError> {
    path(REQUEST_ROOT, sender, token)
}

pub fn session_path(sender: &str, token: &str) -> Result<OwnedObjectPath, PortalError> {
    path(SESSION_ROOT, sender, token)
}

/// Whether `path` is a session path owned by `sender`. A caller may only name
/// sessions under its own sender segment.
pub fn owned_by(path: &str, sender: &str) -> bool {
    path.strip_prefix(SESSION_ROOT)
        .and_then(|rest| rest.strip_prefix('/'))
        .and_then(|rest| rest.split_once('/'))
        .is_some_and(|(segment, _)| segment == sender_segment(sender))
}

#[cfg(test)]
mod tests {
    use super::*;
    use zbus::zvariant::Value;

    #[test]
    fn paths_follow_the_specification() {
        assert_eq!(sender_segment(":1.42"), "1_42");
        assert_eq!(
            request_path(":1.42", "t1").unwrap().as_str(),
            "/org/freedesktop/portal/desktop/request/1_42/t1"
        );
        assert_eq!(
            session_path(":1.7", "s").unwrap().as_str(),
            "/org/freedesktop/portal/desktop/session/1_7/s"
        );
    }

    #[test]
    fn tokens_are_validated_or_generated() {
        let counter = AtomicU64::new(3);
        let mut options = HashMap::new();
        assert_eq!(
            token(&options, "handle_token", &counter).unwrap(),
            "portald3"
        );
        options.insert(
            "handle_token".into(),
            OwnedValue::try_from(Value::from("bad-token")).unwrap(),
        );
        assert!(token(&options, "handle_token", &counter).is_err());
        options.insert(
            "handle_token".into(),
            OwnedValue::try_from(Value::from("ok_1")).unwrap(),
        );
        assert_eq!(token(&options, "handle_token", &counter).unwrap(), "ok_1");
    }

    #[test]
    fn sessions_are_owned_by_their_sender_segment() {
        let path = session_path(":1.7", "s").unwrap();
        assert!(owned_by(path.as_str(), ":1.7"));
        assert!(!owned_by(path.as_str(), ":1.70"));
        assert!(!owned_by(
            "/org/freedesktop/portal/desktop/request/1_7/s",
            ":1.7"
        ));
    }
}
