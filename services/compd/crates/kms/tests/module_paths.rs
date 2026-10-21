// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
use kms::edid::{identity, parse};
use kms::seat::lifecycle::resume::{self, ResumeSteps};
use std::cell::RefCell;

#[test]
fn monitor_identity_uses_edid_and_keeps_connector_fallback_for_zero_serial() {
    let mut bytes = vec![0; 128];
    bytes[..8].copy_from_slice(&[0, 255, 255, 255, 255, 255, 255, 0]);
    bytes[8..10].copy_from_slice(&0x0443u16.to_be_bytes()); // ABC
    bytes[10..12].copy_from_slice(&0x1234u16.to_le_bytes());
    let parsed = parse::parse(&parse::RawEdid(bytes.clone())).unwrap();
    assert_eq!(identity::identity(Some(&parsed), "connector").key(), "ABC 1234 connector");
    bytes[12..16].copy_from_slice(&42u32.to_le_bytes());
    let parsed = parse::parse(&parse::RawEdid(bytes)).unwrap();
    assert_eq!(identity::identity(Some(&parsed), "connector").key(), "ABC 1234 42");
    assert!(parse::parse(&parse::RawEdid(vec![0; 128])).is_none());
}

#[test]
fn session_resume_runs_every_recovery_step_after_an_input_failure() {
    let calls = RefCell::new(Vec::new());
    resume::resume(ResumeSteps {
        resume_input: || { calls.borrow_mut().push("input"); Err("input unavailable".into()) },
        activate_display: |force| { assert!(force); calls.borrow_mut().push("display"); Ok(()) },
        reset_surface: || { calls.borrow_mut().push("surface"); Ok(()) },
        reset_buffers: || calls.borrow_mut().push("buffers"),
        remap_output: || calls.borrow_mut().push("remap"),
    });
    assert_eq!(calls.into_inner(), ["input", "display", "surface", "buffers", "remap"]);
}
