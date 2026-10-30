// SPDX-License-Identifier: MIT OR Apache-2.0
// Compiled as egui-wgpu's unit-test module to exercise its private painter state.
use super::*;

#[test]
fn readback_copy_preserves_aligned_unaligned_and_partial_vectors() {
    #[repr(align(16))]
    struct Aligned([u8; 512]);
    let mut bytes = Aligned([0; 512]);
    for (index, byte) in bytes.0.iter_mut().enumerate() {
        *byte = (index % 251) as u8;
    }
    for offset in 0..16 {
        for length in [0, 1, 15, 16, 17, 127, 128, 129, 256] {
            let source = &bytes.0[offset..offset + length];
            assert_eq!(crate::capture::copy_readback(source), source);
        }
    }
}

fn block_on<F: std::future::Future>(future: F) -> F::Output {
    struct WakeThread(std::thread::Thread);
    impl std::task::Wake for WakeThread {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
        fn wake_by_ref(self: &Arc<Self>) {
            self.0.unpark();
        }
    }
    let waker = std::task::Waker::from(Arc::new(WakeThread(std::thread::current())));
    let mut context = std::task::Context::from_waker(&waker);
    let mut future = std::pin::pin!(future);
    loop {
        match future.as_mut().poll(&mut context) {
            std::task::Poll::Ready(output) => return output,
            std::task::Poll::Pending => std::thread::park(),
        }
    }
}

/// No window, surface, compositor, or subsequent paint: callbacks must still complete.
#[test]
fn hidden_window_capture_without_surface() {
    let config = WgpuConfiguration {
        wgpu_setup: crate::WgpuSetup::CreateNew(crate::WgpuSetupCreateNew::without_display_handle()),
        ..Default::default()
    };
    let options = RendererOptions::default();
    let mut painter = block_on(Painter::new(
        Context::default(),
        config.clone(),
        false,
        options,
    ));
    painter.render_state = Some(
        block_on(RenderState::create(
            &config,
            &painter.instance,
            None,
            options,
        ))
        .expect("headless wgpu adapter"),
    );

    for format in [
        wgpu::TextureFormat::Rgba8Unorm,
        wgpu::TextureFormat::Bgra8Unorm,
    ] {
        let state = painter.render_state.as_mut().unwrap();
        state.target_format = format;
        *state.renderer.write() = crate::Renderer::new(&state.device, format, options);
        painter.screen_capture_state = None;
        // Odd widths exercise padded rows; desktop size exercises bulk readback.
        for size in [[67, 31], [79, 23], [3840, 2160]] {
            let ctx = Context::default();
            let output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(size[0] as f32 / 2.0, size[1] as f32 / 2.0),
                    )),
                    ..Default::default()
                },
                |ui| {
                    ui.painter().rect_filled(
                        egui::Rect::from_min_size(egui::pos2(2.0, 2.0), egui::vec2(8.0, 6.0)),
                        0.0,
                        egui::Color32::RED,
                    );
                },
            );
            let primitives = ctx.tessellate(output.shapes, 2.0);
            painter.context = ctx.clone();
            let mut delta = output.textures_delta;
            delta.free.insert(epaint::TextureId::Managed(0));
            let (tx, rx) = std::sync::mpsc::channel();
            let callbacks = (0..2)
                .map(|_| {
                    let sender = tx.clone();
                    egui::ScreenshotCallback::new(move |image| {
                        sender.send(image).unwrap();
                    })
                })
                .collect();
            drop(tx);
            painter.paint_and_update_textures_impl(
                ViewportId::ROOT,
                size,
                2.0,
                [0.0, 0.0, 1.0, 1.0],
                &primitives,
                &mut delta,
                callbacks,
                None,
            );
            let receipt = egui::Id::unique(("egui-wgpu.capture_submitted", ViewportId::ROOT));
            assert_eq!(
                ctx.data(|data| data.get_temp::<u64>(receipt)),
                Some(ctx.cumulative_pass_nr()),
                "only the submitted GPU copy protects readback from quiet refusal"
            );
            for _ in 0..2 {
                let image = rx
                    .recv_timeout(core::time::Duration::from_secs(15))
                    .expect("offscreen callback without another paint");
                assert_eq!(image.size, [size[0] as usize, size[1] as usize]);
                assert_eq!(image[(0, 0)], egui::Color32::BLUE);
                assert_eq!(image[(8, 8)], egui::Color32::RED);
            }
            assert!(delta.is_empty(), "texture uploads and frees consumed");
            assert!(
                painter
                    .render_state
                    .as_ref()
                    .unwrap()
                    .renderer
                    .read()
                    .texture(&epaint::TextureId::Managed(0))
                    .is_none(),
                "texture freed after submission"
            );
            assert!(painter.surfaces.is_empty(), "no surface was created");
            ctx.data_mut(|data| data.remove::<u64>(receipt));
            painter.paint_and_update_textures_impl(
                ViewportId::ROOT,
                size,
                2.0,
                [0.0, 0.0, 1.0, 1.0],
                &[],
                &mut delta,
                Vec::new(),
                None,
            );
            assert_eq!(
                ctx.data(|data| data.get_temp::<u64>(receipt)),
                None,
                "painting without a screenshot must not claim a submitted copy"
            );
        }
    }
}

/// The real GPU pass must cut all panels, preserve premultiplied colour, draw
/// the border, fill unpainted gaps opaquely, and consume only this viewport's
/// one-shot request. Every registered window clears opaquely, rounded or not.
#[test]
fn rounded_native_frame_fills_unpainted_gaps_and_masks_only_corners() {
    let config = WgpuConfiguration {
        wgpu_setup: crate::WgpuSetup::CreateNew(crate::WgpuSetupCreateNew::without_display_handle()),
        ..Default::default()
    };
    let options = RendererOptions::default();
    let ctx = Context::default();
    let mut painter = block_on(Painter::new(ctx.clone(), config.clone(), true, options));
    painter.render_state = Some(
        block_on(RenderState::create(
            &config,
            &painter.instance,
            None,
            options,
        ))
        .expect("headless adapter"),
    );
    let key = egui::Id::unique(("egui-wgpu.window-frame", ViewportId::ROOT));
    let alpha_key = egui::Id::unique(("egui-wgpu.transparent", ViewportId::ROOT));
    let fill = egui::Color32::from_rgb(37, 73, 109);
    for format in [
        wgpu::TextureFormat::Rgba8Unorm,
        wgpu::TextureFormat::Bgra8Unorm,
    ] {
        let state = painter.render_state.as_mut().unwrap();
        state.target_format = format;
        *state.renderer.write() = crate::Renderer::new(&state.device, format, options);
        painter.screen_capture_state = None;
        // A new renderer needs a fresh font-atlas upload, just as a new device
        // does. Reusing the previous context would leave coloured meshes
        // referring to a missing white texel in the second format.
        let ctx = Context::default();
        painter.context = ctx.clone();
        for scale in [1, 2] {
            let size = [67 * scale, 31 * scale];
            for (present, rounded, transparent) in [
                (true, true, true),
                (true, false, true),
                (true, true, false),
                (false, false, true),
            ] {
                let mut output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(67.0, 31.0),
                        )),
                        ..Default::default()
                    },
                    |ui| {
                        for (left, right) in [(0.0, 20.0), (47.0, 67.0)] {
                            ui.painter().rect_filled(
                                egui::Rect::from_min_max(
                                    egui::pos2(left, 0.0),
                                    egui::pos2(right, 31.0),
                                ),
                                0.0,
                                egui::Color32::BLUE,
                            );
                        }
                    },
                );
                let (radius, border, colour) = if rounded {
                    (10.0 * scale as f32, scale as f32, [1.0, 0.0, 0.0, 1.0])
                } else {
                    (0.0, 0.0, [0.0; 4])
                };
                // Even an accidentally translucent bridge fill must become
                // opaque; transparency is confined to the masked corners.
                let [r, g, b, _] = fill.to_normalized_gamma_f32();
                let request = (radius, border, colour, [r, g, b, 0.25]);
                let child =
                    egui::Id::unique(("egui-wgpu.window-frame", ViewportId::from_hash_of("child")));
                ctx.data_mut(|data| {
                    if present {
                        data.insert_temp(key, request);
                    }
                    data.insert_temp(child, request);
                    data.insert_temp(alpha_key, transparent);
                });
                let (tx, rx) = std::sync::mpsc::channel();
                painter.paint_and_update_textures_impl(
                    ViewportId::ROOT,
                    size,
                    scale as f32,
                    [0.0, 1.0, 0.0, 1.0],
                    &ctx.tessellate(output.shapes, scale as f32),
                    &mut output.textures_delta,
                    vec![egui::ScreenshotCallback::new(move |image| {
                        tx.send(image).unwrap();
                    })],
                    None,
                );
                let image = rx
                    .recv_timeout(core::time::Duration::from_secs(15))
                    .expect("GPU corner readback");
                assert_eq!(
                    image[(33 * scale as usize, 15 * scale as usize)],
                    if present {
                        fill
                    } else {
                        egui::Color32::GREEN
                    },
                    "unpainted gap: format={format:?} scale={scale} present={present} rounded={rounded} transparent={transparent}"
                );
                assert_eq!(
                    image[(10 * scale as usize, 15 * scale as usize)],
                    egui::Color32::BLUE
                );
                if rounded && transparent {
                    for (x, y) in [
                        (0, 0),
                        (size[0] - 1, 0),
                        (0, size[1] - 1),
                        (size[0] - 1, size[1] - 1),
                    ] {
                        assert_eq!(image[(x as usize, y as usize)], egui::Color32::TRANSPARENT);
                    }
                    assert_eq!(
                        image[(33 * scale as usize, 0)],
                        egui::Color32::RED,
                        "shared hairline border"
                    );
                    assert!(
                        image.pixels.iter().any(|c| c.a() > 0 && c.a() < 255),
                        "antialiased coverage"
                    );
                    assert!(
                        image
                            .pixels
                            .iter()
                            .all(|c| c.r() <= c.a() && c.g() <= c.a() && c.b() <= c.a())
                    );
                    for y in 10 * scale..size[1] - 10 * scale {
                        for x in 10 * scale..size[0] - 10 * scale {
                            assert_eq!(
                                image[(x as usize, y as usize)].a(),
                                255,
                                "the interior stays opaque"
                            );
                        }
                    }
                    if format == wgpu::TextureFormat::Bgra8Unorm
                        && scale == 2
                        && let Some(path) = std::env::var_os("MIXOS_FRAME_PREVIEW")
                    {
                        let mut ppm = format!("P6\n{} {}\n255\n", size[0], size[1]).into_bytes();
                        for (i, pixel) in image.pixels.iter().enumerate() {
                            let x = i % size[0] as usize;
                            let y = i / size[0] as usize;
                            let background = if (x / 8 + y / 8) % 2 == 0 {
                                190_u16
                            } else {
                                230_u16
                            };
                            for channel in [pixel.r(), pixel.g(), pixel.b()] {
                                ppm.push(
                                    (u16::from(channel)
                                        + background * u16::from(255 - pixel.a()) / 255)
                                        as u8,
                                );
                            }
                        }
                        std::fs::write(path, ppm).unwrap();
                    }
                } else {
                    assert_eq!(
                        image[(0, 0)],
                        egui::Color32::BLUE,
                        "square or Opaque stays unmasked"
                    );
                }
                assert_eq!(
                    ctx.data(|data| data.get_temp::<(f32, f32, [f32; 4], [f32; 4])>(key)),
                    None
                );
                assert_eq!(
                    ctx.data(|data| data.get_temp::<(f32, f32, [f32; 4], [f32; 4])>(child)),
                    Some(request)
                );
            }
        }
    }
}
