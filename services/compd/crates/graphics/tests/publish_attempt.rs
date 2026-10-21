// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
use graphics::bridge::publish::attempt::Attempt;

#[test]
fn failed_buffer_requests_retry_when_geometry_changes_or_success_clears_them() {
    let mut attempt = Attempt::new();
    assert!(attempt.worth_trying(&(800, 600, 3)));
    assert!(attempt.failed((800, 600, 3)));
    assert!(!attempt.worth_trying(&(800, 600, 3)));
    assert!(!attempt.failed((800, 600, 3)), "log the refusal once");
    assert!(attempt.worth_trying(&(1024, 768, 3)));
    assert!(attempt.worth_trying(&(800, 600, 1)));
    attempt.succeeded();
    assert!(attempt.worth_trying(&(800, 600, 3)));
}
