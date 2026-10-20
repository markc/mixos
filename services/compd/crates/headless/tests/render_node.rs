// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Phase 0 spike for the headless backend: can compd render a client's shm
//! buffer offscreen on a render node with no DRM master, and read it back?
//!
//! Two display paths are tried and reported separately:
//! - GBM on the render node (the planned path), with the EGL device checked
//!   against the node that was opened;
//! - `EGLSurfacelessDisplay` (the fallback), which binds no node itself.
//!
//! The client buffer goes through `ImportMem::import_memory`, the same import a
//! `wl_shm` buffer takes. A negative control checks that a wrong colour fails.
//!
//! Needs a GPU render node, so ignored by default:
//!
//! ```text
//! COMPD_TEST_RENDER_NODE=/dev/dri/renderD128 \
//!     cargo test -p headless --test render_node -- --ignored --nocapture
//! ```

use std::os::fd::OwnedFd;

use smithay::backend::allocator::Fourcc;
use smithay::backend::allocator::gbm::GbmDevice;
use smithay::backend::egl::native::EGLSurfacelessDisplay;
use smithay::backend::egl::{EGLContext, EGLDevice, EGLDisplay};
use smithay::backend::renderer::gles::{GlesRenderer, GlesTexture};
use smithay::backend::renderer::{
    Bind, Color32F, ExportMem, Frame, ImportMem, Offscreen, Renderer,
};
use smithay::utils::{DeviceFd, Physical, Rectangle, Size, Transform};

const W: i32 = 256;
const H: i32 = 128;

const RED: [u8; 4] = [255, 0, 0, 255];
const GREEN: [u8; 4] = [0, 255, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];
const WHITE: [u8; 4] = [255, 255, 255, 255];

fn node_path() -> String {
    std::env::var("COMPD_TEST_RENDER_NODE").unwrap_or_else(|_| "/dev/dri/renderD128".into())
}

/// A client buffer in memory order R, G, B, A (Abgr8888): four quadrants,
/// red top-left, green top-right, blue bottom-left, white bottom-right.
fn client_buffer() -> Vec<u8> {
    let mut bytes = Vec::with_capacity((W * H * 4) as usize);
    for y in 0..H {
        for x in 0..W {
            let colour = match (x < W / 2, y < H / 2) {
                (true, true) => RED,
                (false, true) => GREEN,
                (true, false) => BLUE,
                (false, false) => WHITE,
            };
            bytes.extend_from_slice(&colour);
        }
    }
    bytes
}

fn pixel(bytes: &[u8], x: i32, y: i32) -> [u8; 4] {
    let i = ((y * W + x) * 4) as usize;
    [bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]
}

fn near(actual: [u8; 4], expected: [u8; 4]) -> bool {
    actual
        .iter()
        .zip(expected)
        .all(|(a, e)| (i16::from(*a) - i16::from(e)).abs() <= 3)
}

/// Import the client buffer, composite it into an offscreen texture over a
/// black clear, read the result back, and check the four quadrants. Returns
/// the read-back pixels.
fn render_and_check(gles: &mut GlesRenderer, label: &str) -> Vec<u8> {
    let size: Size<i32, Physical> = (W, H).into();
    let full = Rectangle::from_size(size);

    let client = gles
        .import_memory(&client_buffer(), Fourcc::Abgr8888, (W, H).into(), false)
        .unwrap_or_else(|error| panic!("{label}: import client buffer: {error:?}"));

    let mut target = Offscreen::<GlesTexture>::create_buffer(gles, Fourcc::Abgr8888, (W, H).into())
        .unwrap_or_else(|error| panic!("{label}: offscreen target: {error:?}"));
    {
        let mut framebuffer = gles
            .bind(&mut target)
            .unwrap_or_else(|error| panic!("{label}: bind target: {error:?}"));
        let mut frame = gles
            .render(&mut framebuffer, size, Transform::Normal)
            .unwrap_or_else(|error| panic!("{label}: begin frame: {error:?}"));
        frame
            .clear(Color32F::new(0.0, 0.0, 0.0, 1.0), &[full])
            .expect("clear");
        Frame::render_texture_from_to(
            &mut frame,
            &client,
            Rectangle::from_size((W as f64, H as f64).into()),
            full,
            &[full],
            &[],
            Transform::Normal,
            1.0,
        )
        .expect("draw client texture");
        // finish() returns a sync point; wait on it so the readback below sees
        // completed rendering, not a queued submission.
        let sync = frame.finish().expect("finish frame");
        sync.wait().expect("wait for the GPU");
    }
    let framebuffer = gles.bind(&mut target).expect("rebind target");
    let mapping = gles
        .copy_framebuffer(&framebuffer, full, Fourcc::Abgr8888)
        .unwrap_or_else(|error| panic!("{label}: copy framebuffer: {error:?}"));
    drop(framebuffer);
    let pixels = gles.map_texture(&mapping).expect("map readback").to_vec();

    let checks = [
        ((W / 4, H / 4), RED, "red, top-left"),
        ((3 * W / 4, H / 4), GREEN, "green, top-right"),
        ((W / 4, 3 * H / 4), BLUE, "blue, bottom-left"),
        ((3 * W / 4, 3 * H / 4), WHITE, "white, bottom-right"),
    ];
    for ((x, y), want, what) in checks {
        let got = pixel(&pixels, x, y);
        assert!(near(got, want), "{label}: {what}: got {got:?}, want {want:?}");
    }
    // Negative control: the comparison must reject a wrong colour, or the
    // checks above prove nothing.
    let top_left = pixel(&pixels, W / 4, H / 4);
    assert!(
        !near(top_left, GREEN),
        "{label}: negative control: red pixel {top_left:?} matched green"
    );
    println!("{label}: four quadrants read back correctly; negative control rejected");
    pixels
}

#[test]
#[ignore = "needs a GPU render node"]
fn gbm_render_node_renders_a_client_buffer_offscreen() {
    let node = node_path();
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&node)
        .unwrap_or_else(|error| panic!("open {node}: {error}"));
    let gbm = GbmDevice::new(DeviceFd::from(OwnedFd::from(file))).expect("gbm device");
    let display = unsafe { EGLDisplay::new(gbm) }.expect("EGL display on GBM");

    // The EGL device behind this display must be the node we opened.
    let device = EGLDevice::device_for_display(&display).expect("EGL device for the display");
    let egl_node = device
        .try_get_render_node()
        .expect("query the EGL device's render node")
        .and_then(|n| n.dev_path());
    println!("gbm: opened {node}; EGL device render node = {egl_node:?}");
    let opened = std::fs::canonicalize(&node).expect("canonicalise the opened node");
    let reported = egl_node
        .as_ref()
        .map(|p| std::fs::canonicalize(p).unwrap_or_else(|_| p.clone()));
    assert_eq!(
        reported.as_ref(),
        Some(&opened),
        "the EGL device does not report the render node that was opened"
    );

    let context = EGLContext::new(&display).expect("EGL context");
    let mut gles = unsafe { GlesRenderer::new(context) }.expect("GLES renderer");
    render_and_check(&mut gles, "gbm");
}

#[test]
#[ignore = "needs a GPU render node"]
fn surfaceless_renders_a_client_buffer_offscreen() {
    let display =
        unsafe { EGLDisplay::new(EGLSurfacelessDisplay) }.expect("EGL display, surfaceless");
    let device = EGLDevice::device_for_display(&display);
    let egl_node = device
        .as_ref()
        .ok()
        .and_then(|d| d.try_get_render_node().ok().flatten())
        .and_then(|n| n.dev_path());
    // Surfaceless binds no node itself; record which device Mesa picked.
    println!("surfaceless: EGL device render node = {egl_node:?} (device query ok: {})", device.is_ok());
    let context = EGLContext::new(&display).expect("EGL context, surfaceless");
    let mut gles = unsafe { GlesRenderer::new(context) }.expect("GLES renderer, surfaceless");
    render_and_check(&mut gles, "surfaceless");
}
