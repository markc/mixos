// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Motion probe (plan step 1): can smithay's GLES renderer draw an element
//! scaled with `RescaleRenderElement` and blended with its own alpha? Both
//! answers are read back from the GPU and checked. The probe uses solid
//! elements; a window element is a later check.
//!
//! Needs a GPU render node, so it is ignored by default:
//!
//! ```text
//! COMPD_TEST_RENDER_NODE=/dev/dri/renderD128 \
//!     cargo test -p ui --test motion_probe -- --ignored --nocapture
//! ```

use std::os::fd::OwnedFd;

use smithay::backend::allocator::Fourcc;
use smithay::backend::allocator::gbm::GbmDevice;
use smithay::backend::egl::{EGLContext, EGLDisplay};
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::element::solid::SolidColorRenderElement;
use smithay::backend::renderer::element::utils::RescaleRenderElement;
use smithay::backend::renderer::element::{CommitCounter, Element, Id, RenderElement};
use smithay::backend::renderer::gles::{GlesRenderer, GlesTexture};
use smithay::backend::renderer::{Bind, Color32F, ExportMem, Frame, Offscreen, Renderer};
use smithay::utils::{DeviceFd, Physical, Point, Rectangle, Scale, Size, Transform};

const W: i32 = 256;
const H: i32 = 256;

fn pixel(bytes: &[u8], x: i32, y: i32) -> [u8; 4] {
    let i = ((y * W + x) * 4) as usize;
    [bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]
}

fn gles() -> GlesRenderer {
    let node =
        std::env::var("COMPD_TEST_RENDER_NODE").unwrap_or_else(|_| "/dev/dri/renderD128".into());
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&node)
        .unwrap_or_else(|error| panic!("open {node}: {error}"));
    let gbm = GbmDevice::new(DeviceFd::from(OwnedFd::from(file))).expect("gbm device");
    let display = unsafe { EGLDisplay::new(gbm) }.expect("EGL display");
    let context = EGLContext::new(&display).expect("EGL context");
    unsafe { GlesRenderer::new(context) }.expect("GLES renderer")
}

/// Draw `draw` into a fresh transparent W x H buffer and return its bytes.
fn render(gles: &mut GlesRenderer, draw: impl FnOnce(&mut GlesRenderer, &mut GlesTexture)) -> Vec<u8> {
    let size: Size<i32, Physical> = (W, H).into();
    let mut texture: GlesTexture = gles
        .create_buffer(Fourcc::Abgr8888, size.to_logical(1).to_buffer(1, Transform::Normal))
        .expect("offscreen buffer");
    draw(gles, &mut texture);
    let region = Rectangle::from_size(size);
    let mapping = gles
        .copy_framebuffer(&texture, region, Fourcc::Abgr8888)
        .expect("read back");
    gles.map_texture(&mapping).expect("map readback").to_vec()
}

#[test]
#[ignore = "needs a GPU render node"]
fn scale_halves_a_red_square() {
    let mut gles = gles();
    let red = SolidColorRenderElement::new(
        Id::new(),
        Rectangle::from_size((200, 200).into()),
        CommitCounter::default(),
        [1.0, 0.0, 0.0, 1.0],
        Kind::Unspecified,
    );
    // Scale 0.5 about the origin: the 200-px square becomes 100 px.
    let scaled = RescaleRenderElement::from_element(red, Point::from((0, 0)), Scale::from(0.5));
    let bytes = render(&mut gles, |gles, texture| {
        gles.bind(texture).expect("bind");
        let mut frame = gles
            .render(Size::from((W, H)), Transform::Normal)
            .expect("frame");
        let damage = Rectangle::from_size((W, H).into());
        frame.clear(Color32F::TRANSPARENT, &[damage]).expect("clear");
        scaled
            .draw(&mut frame, scaled.src(), scaled.geometry(Scale::from(1.0)), &[damage], &[])
            .expect("draw scaled");
        frame.finish().expect("finish").sync.wait();
    });
    let inside = pixel(&bytes, 50, 50);
    let outside = pixel(&bytes, 150, 150);
    println!("inside (50,50) = {inside:?}, outside (150,150) = {outside:?}");
    assert!(inside[0] > 200 && inside[3] > 200, "scaled square is red at (50,50): {inside:?}");
    assert!(outside[0] < 20, "scaled square does not reach (150,150): {outside:?}");
}

#[test]
#[ignore = "needs a GPU render node"]
fn element_alpha_blends_green_at_half_opacity() {
    let mut gles = gles();
    let green = SolidColorRenderElement::new(
        Id::new(),
        Rectangle::from_size((200, 200).into()),
        CommitCounter::default(),
        [0.0, 1.0, 0.0, 0.5],
        Kind::Unspecified,
    );
    let bytes = render(&mut gles, |gles, texture| {
        gles.bind(texture).expect("bind");
        let mut frame = gles
            .render(Size::from((W, H)), Transform::Normal)
            .expect("frame");
        let damage = Rectangle::from_size((W, H).into());
        frame.clear(Color32F::TRANSPARENT, &[damage]).expect("clear");
        green
            .draw(&mut frame, green.src(), green.geometry(Scale::from(1.0)), &[damage], &[])
            .expect("draw green");
        frame.finish().expect("finish").sync.wait();
    });
    let p = pixel(&bytes, 100, 100);
    println!("half-opacity green at (100,100) = {p:?}");
    // Premultiplied or not, green should be about half on, and alpha about half.
    assert!((p[3] as i32 - 128).abs() < 16, "alpha ~ 0.5: {p:?}");
    assert!((p[1] as i32 - 128).abs() < 16 || (p[1] as i32 - 0).abs() < 16 && p[3] > 0, "green channel: {p:?}");
}
