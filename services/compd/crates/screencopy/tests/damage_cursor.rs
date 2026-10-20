// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Strict-source regression through the real protocol and service, without a GPU.
use smithay::backend::renderer::{Bind, pixman::PixmanRenderer};
use smithay::reexports::pixman::{FormatCode, Image};
use smithay::utils::{Physical, Rectangle};
use testkit::Harness;
use wayland_client::protocol::wl_shm;

#[test]
fn coalesced_damage_serves_pending_cursor_copy() {
    let mut h = Harness::new();
    let region = Some((100, 100, 10, 10));
    // Establish a damage baseline for this client and output.
    let (first, a) = h.client.capture_with(region, true);
    h.roundtrip();
    let (buffer, _) = h
        .client
        .shm_buffer_with(10, 10, 40, wl_shm::Format::Xrgb8888);
    first.copy_with_damage(&buffer);
    h.roundtrip();
    h.service_screencopy(|_, _| 0x00123456, None);
    assert!(h.client.state.frames[a].ready);

    let (next, b) = h.client.capture_with(region, true);
    h.roundtrip();
    let (buffer, file) = h
        .client
        .shm_buffer_with(10, 10, 40, wl_shm::Format::Xrgb8888);
    next.copy_with_damage(&buffer);
    h.roundtrip();
    let output = h.wire.inner.output.clone();
    // 17 rectangles exceeds MAX_ACCUMULATED_RECTS (16). None touches the
    // region, but their bounding box contains it.
    let damage: Vec<Rectangle<i32, Physical>> = (0..17)
        .map(|i| Rectangle::new((i * 20, i * 20).into(), (1, 1).into()))
        .collect();
    let region = Rectangle::new((100, 100).into(), (10, 10).into());
    // Move the one diagonal point that would otherwise touch the region.
    let mut damage = damage;
    damage[5] = Rectangle::new((100, 0).into(), (1, 1).into());
    assert!(damage.iter().all(|rect| !rect.overlaps(region)));
    assert!(!screencopy::sources_due(&output, Some(&damage)).any());
    assert!(!screencopy::wants_full_frame(&output));
    assert!(screencopy::cursor_pending(&output));

    let mut renderer = PixmanRenderer::new().unwrap();
    let mut image = Image::new(FormatCode::X8R8G8B8, 1920, 1080, true).unwrap();
    let target = renderer.bind(&mut image).unwrap();
    // As in headless, another scene change caused this frame to render; the
    // pending helper offers the cursor despite sources_due's negative answer.
    let cursor = screencopy::cursor_pending(&output).then_some(screencopy::Source {
        framebuffer: &target,
        readback: screencopy::Readback {
            size: (1920, 1080).into(),
            origin_bottom_left: false,
        },
    });
    let captures = screencopy::service_with_sources(
        &mut renderer,
        screencopy::Sources {
            cursor,
            cursorless: None,
        },
        &output,
        Some(&damage),
    );
    assert!(!captures.is_empty(), "coalescing must make the copy due");
    captures.finish(&mut renderer);
    h.roundtrip();
    assert!(h.client.state.frames[b].ready);
    assert!(!h.client.state.frames[b].failed);
    assert_eq!(h.client.state.frames[b].damage, [(0, 0, 10, 10)]);
    assert_eq!(h.client.read_buffer(file).len(), 400);
    assert!(!screencopy::cursor_pending(&output));
}
