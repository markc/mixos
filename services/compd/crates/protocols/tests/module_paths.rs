// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
use protocols::clipboard::capture::ClipboardCapture;
use protocols::clipboard::policy;
use protocols::session::store::{GrantError, SessionStore};
use protocols::tearing::pacer::{TearingTag, Verdict};

#[test]
fn clipboard_capture_rejects_old_generations_and_keeps_advertised_order() {
    let mut capture = ClipboardCapture::default();
    let old = capture.arm(2);
    let current = capture.arm(2);
    assert!(!capture.admit(old, 0, "text/plain".into(), vec![1], 4));
    assert!(capture.admit(current, 1, "image/png".into(), vec![2, 3], 4));
    assert!(!capture.admit(current, 0, "text/plain".into(), vec![1, 2, 3], 4));
    assert!(capture.admit(current, 0, "text/plain".into(), vec![4], 4));
    assert_eq!(capture.mime_types(), ["text/plain", "image/png"]);
    assert_eq!(capture.bytes_for("image/png").unwrap().as_slice(), [2, 3]);
    assert_eq!(capture.headroom(4), 1);
    assert_eq!(policy::ordered(&["image/png".into(), "text/plain".into()])[0].0, 1);
}

#[test]
fn session_grants_preserve_ownership_across_rename_and_release() {
    let mut store = SessionStore::default();
    store.grant("session", "editor", 1).unwrap();
    assert_eq!(store.grant("session", "editor", 2), Err(GrantError::NameInUse));
    assert_eq!(store.grant("session", "other", 1), Err(GrantError::AlreadyAdded));
    store.rename("session", "editor", "renamed").unwrap();
    store.release("session", "renamed", 2);
    assert_eq!(store.grant("session", "renamed", 2), Err(GrantError::NameInUse));
    store.release("session", "renamed", 1);
    store.grant("session", "renamed", 2).unwrap();
    assert!(store.knows("session", "renamed"));
    assert!(!store.knows("session", "editor"));
}

#[test]
fn a_client_vsync_hint_overrules_the_heuristic_but_not_forcing() {
    let tag = TearingTag::default();
    tag.set_heuristic(true);
    tag.set_hint(false);
    let mut verdict = Verdict::default();
    verdict.absorb(&tag);
    assert!(!verdict.is_target());
    tag.set_forced(true);
    verdict.absorb(&tag);
    assert!(verdict.is_target());
}
