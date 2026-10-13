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
use smithay::backend::renderer::element::solid::SolidColorRenderElement;
use smithay::backend::renderer::element::texture::TextureRenderElement;
use smithay::backend::renderer::element::utils::RescaleRenderElement;
use smithay::backend::renderer::element::{Element, Id, Kind, RenderElement};
use smithay::backend::renderer::gles::{GlesRenderer, GlesTexture};
use smithay::backend::renderer::utils::CommitCounter;
use smithay::backend::renderer::{Bind, Color32F, ExportMem, Frame, Offscreen, Renderer};
use smithay::backend::renderer::ContextId;
use smithay::utils::{Buffer, DeviceFd, Physical, Point, Rectangle, Scale, Size, Transform};

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

/// Clear a fresh W x H buffer to transparent, draw `element` into it, and
/// return the bytes read back from the GPU.
fn draw_once<E: RenderElement<GlesRenderer>>(gles: &mut GlesRenderer, element: &E) -> Vec<u8> {
    let size: Size<i32, Physical> = (W, H).into();
    let full = Rectangle::from_size(size);
    let mut texture: GlesTexture = gles
        .create_buffer(Fourcc::Abgr8888, Size::<i32, Buffer>::from((W, H)))
        .expect("offscreen buffer");
    {
        let mut target = gles.bind(&mut texture).expect("bind target");
        let mut frame = gles
            .render(&mut target, size, Transform::Normal)
            .expect("begin frame");
        frame
            .clear(Color32F::new(0.0, 0.0, 0.0, 0.0), &[full])
            .expect("clear");
        element
            .draw(
                &mut frame,
                element.src(),
                element.geometry(Scale::from(1.0)),
                &[full],
                &[],
                None,
            )
            .expect("draw element");
        let sync = frame.finish().expect("finish frame");
        let _ = sync.wait();
    }
    let mapping = gles
        .copy_texture(
            &texture,
            Rectangle::from_size(Size::<i32, Buffer>::from((W, H))),
            Fourcc::Abgr8888,
        )
        .expect("copy texture");
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
    let bytes = draw_once(&mut gles, &scaled);
    let inside = pixel(&bytes, 50, 50);
    let outside = pixel(&bytes, 150, 150);
    println!("inside (50,50) = {inside:?}, outside (150,150) = {outside:?}");
    assert!(
        inside[0] > 200 && inside[3] > 200,
        "scaled square is red at (50,50): {inside:?}"
    );
    assert!(
        outside[0] < 20,
        "scaled square does not reach (150,150): {outside:?}"
    );
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
    let bytes = draw_once(&mut gles, &green);
    let p = pixel(&bytes, 100, 100);
    println!("half-opacity green at (100,100) = {p:?}");
    // The element alpha is 0.5. Expect the output alpha near 128.
    assert!((p[3] as i32 - 128).abs() < 16, "alpha near 0.5: {p:?}");
}

/// A W x H texture filled with `colour`, the stand-in for a window's content.
fn texture_filled(gles: &mut GlesRenderer, colour: [f32; 4]) -> GlesTexture {
    let size: Size<i32, Physical> = (W, H).into();
    let mut texture: GlesTexture = gles
        .create_buffer(Fourcc::Abgr8888, Size::<i32, Buffer>::from((W, H)))
        .expect("content buffer");
    {
        let mut target = gles.bind(&mut texture).expect("bind content");
        let mut frame = gles
            .render(&mut target, size, Transform::Normal)
            .expect("begin content frame");
        frame
            .clear(Color32F::new(colour[0], colour[1], colour[2], colour[3]), &[Rectangle::from_size(size)])
            .expect("fill content");
        let sync = frame.finish().expect("finish content");
        let _ = sync.wait();
    }
    texture
}

/// A 200 x 200 logical texture element at the origin, with an optional alpha.
fn content_element(
    gles: &GlesRenderer,
    texture: GlesTexture,
    alpha: Option<f32>,
) -> TextureRenderElement<GlesTexture> {
    TextureRenderElement::from_static_texture(
        Id::new(),
        gles.context_id(),
        Point::<f64, Physical>::from((0.0, 0.0)),
        texture,
        1,
        Transform::Normal,
        alpha,
        None,
        Some(Size::from((200, 200))),
        None,
        Kind::Unspecified,
    )
}

#[test]
#[ignore = "needs a GPU render node"]
fn scale_halves_a_content_texture() {
    let mut gles = gles();
    let texture = texture_filled(&mut gles, [1.0, 0.0, 0.0, 1.0]);
    let content = content_element(&gles, texture, None);
    let scaled = RescaleRenderElement::from_element(content, Point::from((0, 0)), Scale::from(0.5));
    let bytes = draw_once(&mut gles, &scaled);
    let inside = pixel(&bytes, 50, 50);
    let outside = pixel(&bytes, 150, 150);
    println!("content inside (50,50) = {inside:?}, outside (150,150) = {outside:?}");
    assert!(inside[0] > 200 && inside[3] > 200, "scaled content is red at (50,50): {inside:?}");
    assert!(outside[0] < 20, "scaled content does not reach (150,150): {outside:?}");
}

#[test]
#[ignore = "needs a GPU render node"]
fn content_alpha_fades_a_texture_to_half() {
    let mut gles = gles();
    let texture = texture_filled(&mut gles, [0.0, 1.0, 0.0, 1.0]);
    let content = content_element(&gles, texture, Some(0.5));
    let bytes = draw_once(&mut gles, &content);
    let p = pixel(&bytes, 100, 100);
    println!("content at half alpha (100,100) = {p:?}");
    assert!((p[3] as i32 - 128).abs() < 16, "content alpha near 0.5: {p:?}");
}
