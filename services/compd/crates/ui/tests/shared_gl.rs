// SPDX-License-Identifier: MIT OR Apache-2.0
//! The shared-GL proof (plan checkpoint 4): egui paints with egui-wgpu into a
//! texture on smithay's own EGL context, then smithay's GLES renderer
//! composites that texture, and both results are read back and checked:
//! colour channels, orientation, premultiplied alpha, scale, and that
//! smithay's draws still work after wgpu used the context.
//!
//! Needs a GPU render node, so it is ignored by default. Run it on a machine
//! with one:
//!
//! ```text
//! COMPD_TEST_RENDER_NODE=/dev/dri/renderD128 \
//!     cargo test -p ui --test shared_gl -- --ignored --nocapture
//! ```
//!
//! `COMPD_TEST_OUT=<dir>` also writes the two images as PNG for a look.

use std::os::fd::OwnedFd;

use smithay::backend::allocator::Fourcc;
use smithay::backend::allocator::gbm::GbmDevice;
use smithay::backend::egl::{EGLContext, EGLDisplay};
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::{Bind, Color32F, ExportMem, Frame, Offscreen, Renderer};
use smithay::utils::{DeviceFd, Physical, Rectangle, Size, Transform};
use ui::{UiProgram, UiRuntime};

const W: i32 = 256;
const H: i32 = 128;
/// Physical pixels per egui point.
const SCALE: f32 = 2.0;

/// Top half: four 64-px quarters (opaque red, opaque green, white at 50 %,
/// empty with text). Bottom half: opaque blue.
struct Bars;

impl UiProgram for Bars {
    type Message = ();

    fn show(&mut self, ui: &mut egui::Ui, _emit: &mut Vec<()>) {
        let painter = ui.ctx().layer_painter(egui::LayerId::background());
        // Points: the surface is 128 x 64 points at scale 2.
        let quarter = |i: f32| egui::Rect::from_min_size(egui::pos2(32.0 * i, 0.0), egui::vec2(32.0, 32.0));
        painter.rect_filled(quarter(0.0), 0.0, egui::Color32::from_rgb(255, 0, 0));
        painter.rect_filled(quarter(1.0), 0.0, egui::Color32::from_rgb(0, 255, 0));
        painter.rect_filled(quarter(2.0), 0.0, egui::Color32::from_white_alpha(128));
        painter.text(
            quarter(3.0).center(),
            egui::Align2::CENTER_CENTER,
            "Ag",
            egui::FontId::proportional(16.0),
            egui::Color32::WHITE,
        );
        painter.rect_filled(
            egui::Rect::from_min_size(egui::pos2(0.0, 32.0), egui::vec2(128.0, 32.0)),
            0.0,
            egui::Color32::from_rgb(0, 0, 255),
        );
    }

    fn update(&mut self, _message: ()) {}

    fn setup(&mut self, _ctx: &egui::Context) {}
}

fn pixel(bytes: &[u8], x: i32, y: i32) -> [u8; 4] {
    let i = ((y * W + x) * 4) as usize;
    [bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]
}

fn close(actual: [u8; 4], expected: [u8; 4], what: &str) {
    let off = actual
        .iter()
        .zip(expected)
        .any(|(a, e)| (i16::from(*a) - i16::from(e)).abs() > 3);
    assert!(!off, "{what}: got {actual:?}, want {expected:?}");
}

fn save(bytes: &[u8], name: &str) {
    let Some(dir) = std::env::var_os("COMPD_TEST_OUT") else { return };
    let path = std::path::Path::new(&dir).join(name);
    let file = std::fs::File::create(&path).expect("create png");
    let mut encoder = png::Encoder::new(file, W as u32, H as u32);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .and_then(|mut writer| writer.write_image_data(bytes))
        .expect("write png");
    println!("wrote {}", path.display());
}

#[test]
#[ignore = "needs a GPU render node"]
fn egui_paints_into_the_shared_context_and_smithay_composites_it() {
    let node = std::env::var("COMPD_TEST_RENDER_NODE").unwrap_or_else(|_| "/dev/dri/renderD128".into());
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&node)
        .unwrap_or_else(|error| panic!("open {node}: {error}"));
    let gbm = GbmDevice::new(DeviceFd::from(OwnedFd::from(file))).expect("gbm device");
    let display = unsafe { EGLDisplay::new(gbm) }.expect("EGL display");
    let context = EGLContext::new(&display).expect("EGL context");
    let mut gles = unsafe { GlesRenderer::new(context) }.expect("GLES renderer");

    let registrar = render_gles::format::registrar::registrar::Registrar::new();
    let wgpu = graphics::surface::create_wgpu_gl_context(&registrar, &mut gles).expect("wgpu on the GL context");
    let size: Size<i32, Physical> = (W, H).into();
    let mut surface =
        graphics::surface::UiSurface::allocate(&node, &wgpu, &mut gles, size).expect("UI surface");

    // ── egui paints into the surface ────────────────────────────────────
    let mut runtime = UiRuntime::new(Bars, (W as u32, H as u32), SCALE);
    assert!(runtime.tick());
    graphics::surface::make_current(&mut gles);
    wgpu.acquire_gl_state();
    let view = surface.begin_render_view().expect("resident surface");
    runtime.render_into(&wgpu.device, &wgpu.queue, graphics::surface::FORMAT, &view);
    surface.submitted(false);
    let _ = wgpu.device.poll(wgpu::PollType::wait_indefinitely());
    wgpu.release_gl_state();
    surface.poll();

    let texture = surface.gles_texture().expect("published texture").clone();
    let region = Rectangle::from_size((W, H).into());
    let mapping = gles.copy_texture(&texture, region, Fourcc::Abgr8888).expect("copy UI texture");
    let ui_pixels = gles.map_texture(&mapping).expect("map UI texture").to_vec();
    save(&ui_pixels, "shared_gl_ui.png");

    // Memory order R, G, B, A; premultiplied.
    close(pixel(&ui_pixels, 32, 16), [255, 0, 0, 255], "red quarter, top-left");
    close(pixel(&ui_pixels, 96, 16), [0, 255, 0, 255], "green quarter");
    close(pixel(&ui_pixels, 160, 16), [128, 128, 128, 128], "white at 50 %, premultiplied");
    close(pixel(&ui_pixels, 224, 4), [0, 0, 0, 0], "empty corner stays transparent");
    close(pixel(&ui_pixels, 128, 100), [0, 0, 255, 255], "blue bottom half (orientation)");
    let text_lit = (192..256)
        .flat_map(|x| (8..56).map(move |y| (x, y)))
        .any(|(x, y)| pixel(&ui_pixels, x, y)[3] > 200);
    assert!(text_lit, "the text drew some opaque pixels");

    // ── smithay composites the texture after wgpu used the context ──────
    let mut target = Offscreen::<smithay::backend::renderer::gles::GlesTexture>::create_buffer(
        &mut gles,
        Fourcc::Abgr8888,
        (W, H).into(),
    )
    .expect("offscreen target");
    {
        let mut framebuffer = gles.bind(&mut target).expect("bind target");
        let mut frame = gles
            .render(&mut framebuffer, size, Transform::Normal)
            .expect("begin frame");
        let full = Rectangle::from_size(size);
        frame
            .clear(Color32F::new(0.0, 0.0, 0.0, 1.0), &[full])
            .expect("clear");
        frame
            .render_texture_from_to(
                &texture,
                Rectangle::from_size((W as f64, H as f64).into()),
                full,
                &[full],
                &[],
                Transform::Normal,
                1.0,
            )
            .expect("draw UI texture");
        frame.finish().expect("finish frame");
    }
    let framebuffer = gles.bind(&mut target).expect("rebind target");
    let mapping = gles
        .copy_framebuffer(&framebuffer, region, Fourcc::Abgr8888)
        .expect("copy composite");
    drop(framebuffer);
    let composite = gles.map_texture(&mapping).expect("map composite").to_vec();
    save(&composite, "shared_gl_composite.png");

    close(pixel(&composite, 32, 16), [255, 0, 0, 255], "composite red");
    close(pixel(&composite, 96, 16), [0, 255, 0, 255], "composite green");
    close(pixel(&composite, 160, 16), [128, 128, 128, 255], "50 % white over black");
    close(pixel(&composite, 224, 4), [0, 0, 0, 255], "transparent over black");
    close(pixel(&composite, 128, 100), [0, 0, 255, 255], "composite blue (orientation)");
}
