// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
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
        let quarter =
            |i: f32| egui::Rect::from_min_size(egui::pos2(32.0 * i, 0.0), egui::vec2(32.0, 32.0));
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
    let Some(dir) = std::env::var_os("COMPD_TEST_OUT") else {
        return;
    };
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
    let mut gles = unsafe { GlesRenderer::new(context) }.expect("GLES renderer");

    let registrar = render_gles::format::registrar::registrar::Registrar::new();
    let wgpu = graphics::surface::create_wgpu_gl_context(&registrar, &mut gles)
        .expect("wgpu on the GL context");
    let size: Size<i32, Physical> = (W, H).into();
    let mut surface =
        graphics::surface::UiSurface::allocate(&node, &wgpu, &mut gles, size).expect("UI surface");

    // ── egui paints into the surface ────────────────────────────────────
    let gpu = ui::engine::Gpu {
        device: wgpu.device.clone(),
        queue: wgpu.queue.clone(),
        format: graphics::surface::FORMAT,
    };
    let mut runtime = UiRuntime::new(Bars, (W as u32, H as u32), SCALE, gpu);
    assert!(runtime.tick());
    graphics::surface::make_current(&mut gles);
    wgpu.acquire_gl_state();
    let view = surface.begin_render_view().expect("resident surface");
    runtime.render_into(&view);
    surface.submitted(false);
    let _ = wgpu.device.poll(wgpu::PollType::wait_indefinitely());
    wgpu.release_gl_state();
    surface.poll();

    let texture = surface.gles_texture().expect("published texture").clone();
    let region = Rectangle::from_size((W, H).into());
    let mapping = gles
        .copy_texture(&texture, region, Fourcc::Abgr8888)
        .expect("copy UI texture");
    let ui_pixels = gles.map_texture(&mapping).expect("map UI texture").to_vec();
    save(&ui_pixels, "shared_gl_ui.png");

    // Memory order R, G, B, A; premultiplied.
    close(
        pixel(&ui_pixels, 32, 16),
        [255, 0, 0, 255],
        "red quarter, top-left",
    );
    close(pixel(&ui_pixels, 96, 16), [0, 255, 0, 255], "green quarter");
    close(
        pixel(&ui_pixels, 160, 16),
        [128, 128, 128, 128],
        "white at 50 %, premultiplied",
    );
    close(
        pixel(&ui_pixels, 224, 4),
        [0, 0, 0, 0],
        "empty corner stays transparent",
    );
    close(
        pixel(&ui_pixels, 128, 100),
        [0, 0, 255, 255],
        "blue bottom half (orientation)",
    );
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
        Frame::render_texture_from_to(
            &mut frame,
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
    close(
        pixel(&composite, 96, 16),
        [0, 255, 0, 255],
        "composite green",
    );
    close(
        pixel(&composite, 160, 16),
        [128, 128, 128, 255],
        "50 % white over black",
    );
    close(
        pixel(&composite, 224, 4),
        [0, 0, 0, 255],
        "transparent over black",
    );
    close(
        pixel(&composite, 128, 100),
        [0, 0, 255, 255],
        "composite blue (orientation)",
    );
}

/// A full surface of one colour, changed by message.
struct Fill(egui::Color32);

impl UiProgram for Fill {
    type Message = egui::Color32;

    fn show(&mut self, ui: &mut egui::Ui, _emit: &mut Vec<egui::Color32>) {
        let rect = ui.ctx().content_rect();
        ui.ctx()
            .layer_painter(egui::LayerId::background())
            .rect_filled(rect, 0.0, self.0);
    }

    fn update(&mut self, colour: egui::Color32) {
        self.0 = colour;
    }

    fn setup(&mut self, _ctx: &egui::Context) {}
}

/// Two runtimes, each with its own painter, alternate with smithay draws for
/// several rounds: neither leaves GL state that breaks the other.
#[test]
#[ignore = "needs a GPU render node"]
fn two_painters_alternate_with_smithay_draws() {
    let node =
        std::env::var("COMPD_TEST_RENDER_NODE").unwrap_or_else(|_| "/dev/dri/renderD128".into());
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&node)
        .expect("render node");
    let gbm = GbmDevice::new(DeviceFd::from(OwnedFd::from(file))).expect("gbm device");
    let display = unsafe { EGLDisplay::new(gbm) }.expect("EGL display");
    let context = EGLContext::new(&display).expect("EGL context");
    let mut gles = unsafe { GlesRenderer::new(context) }.expect("GLES renderer");
    let registrar = render_gles::format::registrar::registrar::Registrar::new();
    let wgpu = graphics::surface::create_wgpu_gl_context(&registrar, &mut gles).expect("wgpu");
    let size: Size<i32, Physical> = (W, H).into();
    let gpu = || ui::engine::Gpu {
        device: wgpu.device.clone(),
        queue: wgpu.queue.clone(),
        format: graphics::surface::FORMAT,
    };
    let mut left = UiRuntime::new(Fill(egui::Color32::RED), (W as u32, H as u32), 1.0, gpu());
    let mut right = UiRuntime::new(Fill(egui::Color32::GREEN), (W as u32, H as u32), 1.0, gpu());
    let mut left_surface =
        graphics::surface::UiSurface::allocate(&node, &wgpu, &mut gles, size).expect("surface");
    let mut right_surface =
        graphics::surface::UiSurface::allocate(&node, &wgpu, &mut gles, size).expect("surface");
    let mut target = Offscreen::<smithay::backend::renderer::gles::GlesTexture>::create_buffer(
        &mut gles,
        Fourcc::Abgr8888,
        (W, H).into(),
    )
    .expect("offscreen target");
    let rounds = [
        (egui::Color32::RED, egui::Color32::GREEN),
        (egui::Color32::BLUE, egui::Color32::YELLOW),
        (egui::Color32::WHITE, egui::Color32::BLACK),
    ];
    for (round, (a, b)) in rounds.into_iter().enumerate() {
        left.queue_message(a);
        right.queue_message(b);
        for (runtime, surface) in [
            (&mut left, &mut left_surface),
            (&mut right, &mut right_surface),
        ] {
            runtime.tick();
            graphics::surface::make_current(&mut gles);
            wgpu.acquire_gl_state();
            let view = surface.begin_render_view().expect("view");
            runtime.render_into(&view);
            surface.submitted(false);
            let _ = wgpu.device.poll(wgpu::PollType::wait_indefinitely());
            wgpu.release_gl_state();
            surface.poll();
        }
        // smithay: left texture on the left half, right on the right half.
        let left_texture = left_surface.gles_texture().expect("left").clone();
        let right_texture = right_surface.gles_texture().expect("right").clone();
        {
            let mut framebuffer = gles.bind(&mut target).expect("bind");
            let mut frame = gles
                .render(&mut framebuffer, size, Transform::Normal)
                .expect("frame");
            let full = Rectangle::from_size(size);
            frame
                .clear(Color32F::new(0.5, 0.5, 0.5, 1.0), &[full])
                .expect("clear");
            let half = Rectangle::new((0, 0).into(), (W / 2, H).into());
            let other = Rectangle::new((W / 2, 0).into(), (W / 2, H).into());
            let src = Rectangle::from_size(((W / 2) as f64, H as f64).into());
            // Damage is relative to the destination rectangle.
            let local = Rectangle::from_size(half.size);
            Frame::render_texture_from_to(
                &mut frame,
                &left_texture,
                src,
                half,
                &[local],
                &[],
                Transform::Normal,
                1.0,
            )
            .expect("left draw");
            Frame::render_texture_from_to(
                &mut frame,
                &right_texture,
                src,
                other,
                &[local],
                &[],
                Transform::Normal,
                1.0,
            )
            .expect("right draw");
            frame.finish().expect("finish");
        }
        let framebuffer = gles.bind(&mut target).expect("rebind");
        let mapping = gles
            .copy_framebuffer(
                &framebuffer,
                Rectangle::from_size((W, H).into()),
                Fourcc::Abgr8888,
            )
            .expect("copy");
        drop(framebuffer);
        let pixels = gles.map_texture(&mapping).expect("map").to_vec();
        save(&pixels, &format!("shared_gl_round{round}.png"));
        close(
            pixel(&pixels, 32, 64),
            a.to_array(),
            &format!("round {round} left"),
        );
        close(
            pixel(&pixels, 224, 64),
            b.to_array(),
            &format!("round {round} right"),
        );
    }
}

/// Loads a texture, draws it and drops the handle, all in one pass: egui frees
/// it at the end of that very pass, and its shapes still draw it.
struct Transient;

impl UiProgram for Transient {
    type Message = ();

    fn show(&mut self, ui: &mut egui::Ui, _emit: &mut Vec<()>) {
        let image = egui::ColorImage::filled([4, 4], egui::Color32::from_rgb(200, 100, 50));
        let texture = ui
            .ctx()
            .load_texture("transient", image, egui::TextureOptions::NEAREST);
        let rect = ui.ctx().content_rect();
        let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
        ui.ctx().layer_painter(egui::LayerId::background()).image(
            texture.id(),
            rect,
            uv,
            egui::Color32::WHITE,
        );
    }

    fn update(&mut self, _message: ()) {}

    fn setup(&mut self, _ctx: &egui::Context) {}
}

#[test]
#[ignore = "needs a GPU render node"]
fn a_texture_dropped_in_its_own_pass_still_draws() {
    let node =
        std::env::var("COMPD_TEST_RENDER_NODE").unwrap_or_else(|_| "/dev/dri/renderD128".into());
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&node)
        .expect("render node");
    let gbm = GbmDevice::new(DeviceFd::from(OwnedFd::from(file))).expect("gbm device");
    let display = unsafe { EGLDisplay::new(gbm) }.expect("EGL display");
    let context = EGLContext::new(&display).expect("EGL context");
    let mut gles = unsafe { GlesRenderer::new(context) }.expect("GLES renderer");
    let registrar = render_gles::format::registrar::registrar::Registrar::new();
    let wgpu = graphics::surface::create_wgpu_gl_context(&registrar, &mut gles).expect("wgpu");
    let size: Size<i32, Physical> = (W, H).into();
    let gpu = ui::engine::Gpu {
        device: wgpu.device.clone(),
        queue: wgpu.queue.clone(),
        format: graphics::surface::FORMAT,
    };
    let mut runtime = UiRuntime::new(Transient, (W as u32, H as u32), 1.0, gpu);
    let mut surface =
        graphics::surface::UiSurface::allocate(&node, &wgpu, &mut gles, size).expect("surface");
    // Two passes: the second supersedes the first, whose texture is freed then.
    for round in 0..2 {
        runtime.request_redraw();
        runtime.tick();
        graphics::surface::make_current(&mut gles);
        wgpu.acquire_gl_state();
        let view = surface.begin_render_view().expect("view");
        runtime.render_into(&view);
        surface.submitted(false);
        let _ = wgpu.device.poll(wgpu::PollType::wait_indefinitely());
        wgpu.release_gl_state();
        surface.poll();
        let texture = surface.gles_texture().expect("published").clone();
        let mapping = gles
            .copy_texture(
                &texture,
                Rectangle::from_size((W, H).into()),
                Fourcc::Abgr8888,
            )
            .expect("copy");
        let pixels = gles.map_texture(&mapping).expect("map").to_vec();
        close(
            pixel(&pixels, 100, 60),
            [200, 100, 50, 255],
            &format!("round {round}: the transient texture drew"),
        );
    }
}
