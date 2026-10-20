// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Protocol-level regression for strict per-capture sources, without a GPU.
use smithay::backend::renderer::{Bind, pixman::PixmanRenderer};
use smithay::reexports::pixman::{FormatCode, Image};
use testkit::Harness;
use wayland_client::protocol::wl_shm;

#[test]
fn missing_picture_fails_only_its_own_cursor_policy() {
    for offered_cursor in [false, true] {
        let mut h = Harness::new();
        let (on, a) = h.client.capture_with(Some((0, 0, 2, 2)), true);
        let (off, b) = h.client.capture_with(Some((0, 0, 2, 2)), false);
        h.roundtrip();
        let (on_buffer, _) = h.client.shm_buffer_with(2, 2, 8, wl_shm::Format::Xrgb8888);
        let (off_buffer, _) = h.client.shm_buffer_with(2, 2, 8, wl_shm::Format::Xrgb8888);
        on.copy(&on_buffer);
        off.copy(&off_buffer);
        h.roundtrip();
        let output = h.wire.inner.output.clone();
        let mut renderer = PixmanRenderer::new().unwrap();
        let mut image = Image::new(FormatCode::X8R8G8B8, 1920, 1080, true).unwrap();
        let target = renderer.bind(&mut image).unwrap();
        let source = Some(screencopy::Source {
            framebuffer: &target,
            readback: screencopy::Readback {
                size: (1920, 1080).into(),
                origin_bottom_left: false,
            },
        });
        let sources = if offered_cursor {
            screencopy::Sources {
                cursor: source,
                cursorless: None,
            }
        } else {
            screencopy::Sources {
                cursor: None,
                cursorless: source,
            }
        };
        screencopy::service_with_sources(&mut renderer, sources, &output, None)
            .finish(&mut renderer);
        h.roundtrip();
        assert_eq!(h.client.state.frames[a].ready, offered_cursor);
        assert_eq!(h.client.state.frames[a].failed, !offered_cursor);
        assert_eq!(h.client.state.frames[b].ready, !offered_cursor);
        assert_eq!(h.client.state.frames[b].failed, offered_cursor);
    }
}
