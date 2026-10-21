// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The format registry keeps its completeness and client-offer contracts.

use render_gles::format::{
    answer,
    registrar::{Device, Registrar},
    role::Role,
};
use smithay::backend::allocator::{Format, Fourcc, Modifier};

#[test]
fn every_role_must_answer_even_when_it_has_no_device() {
    let registry = Registrar::new();
    assert_eq!(registry.missing(&registry.view()).len(), 5);
    registry.absent(Role::Sample, "test");
    registry.absent(Role::GlesSample, "test");
    assert_eq!(
        registry.missing(&registry.view()),
        ["composite-render", "scanout-egl", "plane-scanout"]
    );
    registry.absent(Role::Render, "test");
    registry.absent(Role::ScanoutEgl, "test");
    registry.absent(Role::Plane, "test");
    assert!(registry.missing(&registry.view()).is_empty());
}

#[test]
fn client_feedback_keeps_the_compositors_published_pair() {
    let registry = Registrar::new();
    let pair = Format {
        code: Fourcc::Argb8888,
        modifier: Modifier::Linear,
    };
    for role in [Role::Sample, Role::GlesSample] {
        registry.register(
            Device::UNSPECIFIED,
            role,
            [pair].into_iter().collect(),
            "test",
        );
    }
    for role in [Role::Render, Role::ScanoutEgl, Role::Plane] {
        registry.absent(role, "no device in this test");
    }
    let answer::Answer::Set { set, .. } =
        answer::available(&registry, answer::Consumer::ClientFeedback)
    else {
        panic!("client feedback is a set");
    };
    assert_eq!(set.iter().copied().collect::<Vec<_>>(), [pair]);
}
