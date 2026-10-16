// SPDX-License-Identifier: MIT OR Apache-2.0
//! The GPU grid against the CPU painter, through real egui-wgpu.
//!
//! Each case paints one screen both ways at a device scale: `Raster::render`
//! on the CPU, and a [`Pane`] paint callback inside an offscreen egui frame on
//! the GPU. The CPU blends with truncating integer division and the GPU in
//! float with round-to-nearest, so a blended channel may differ by one; any
//! pixel off by more, and any fully covered or background pixel off at all,
//! is a renderer bug.
use egui_kittest::Harness;
use egui_kittest::wgpu::{WgpuTestRenderer, create_render_state, default_wgpu_setup};
use std::sync::{Arc, Mutex};
use term_core::config::Cursor;
use term_core::raster::Raster;
use term_core::terminal::{Screen, Terminal};
use term_render::Pane;

const TEXT: &str = "Ag|_ q\u{e9}\u{2500}\u{253c}\u{2588} \u{1f469}\u{200d}\u{1f4bb}x\r\n\
                    \u{c911}a \u{1f44d}\u{1f3fd} e\u{301}!\r\n\u{1f1eb}\u{1f1f7} ~ $ ls -la";

fn screen(cursor: (usize, usize)) -> Screen {
    let mut grid = Terminal::from_test_vt(16, 4, TEXT.as_bytes()).screen(false);
    for (i, cell) in grid.cells.iter_mut().enumerate() {
        cell.bg = [(i * 37 % 256) as u8, 29, (i * 11 % 256) as u8];
        cell.fg = [230, (i * 53 % 256) as u8, 120];
    }
    grid.cursor = cursor;
    grid.cursor_visible = true;
    grid
}

struct Frame {
    gpu: Vec<u8>,
    width: usize,
    stats: term_render::Stats,
}

/// Render `screens` in successive frames through one pane and return the
/// last frame's pixels, cropped to the grid.
fn gpu_frames(raster: Arc<Mutex<Raster>>, scale: f32, screens: Vec<(Screen, Vec<bool>)>) -> Frame {
    let (width, height) = {
        let r = raster.lock().unwrap();
        r.target_size(&screens[0].0)
    };
    let state = create_render_state(
        default_wgpu_setup(),
        egui_wgpu::RendererOptions::PREDICTABLE,
    );
    term_render::install(&state);
    let pane = Pane::new();
    let size = egui::vec2(width as f32 / scale, height as f32 / scale);
    let ui_pane = pane.clone();
    let ui_raster = raster.clone();
    let mut harness = Harness::builder()
        .with_size(size)
        .with_pixels_per_point(scale)
        .renderer(WgpuTestRenderer::from_render_state(state))
        .build_ui(move |ui| {
            let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
            ui.painter().add(ui_pane.callback(rect, ui_raster.clone()));
        });
    let mut image = None;
    for (screen, dirty) in screens {
        pane.update(screen, dirty);
        harness.step();
        image = Some(harness.render().expect("offscreen render"));
    }
    let image = image.unwrap();
    let mut gpu = Vec::with_capacity(width as usize * height as usize * 4);
    for y in 0..height {
        for x in 0..width {
            gpu.extend_from_slice(&image.get_pixel(x, y).0);
        }
    }
    Frame {
        gpu,
        width: width as usize,
        stats: pane.stats(),
    }
}

/// (pixels off by exactly one in some channel, pixels off by more)
fn compare(gpu: &[u8], cpu: &[u8], width: usize, label: &str) -> (usize, usize) {
    assert_eq!(gpu.len(), cpu.len(), "{label}: frame sizes");
    let mut one = 0;
    let mut more = Vec::new();
    for (i, (g, c)) in gpu.chunks_exact(4).zip(cpu.chunks_exact(4)).enumerate() {
        let worst = (0..3).map(|k| g[k].abs_diff(c[k])).max().unwrap();
        match worst {
            0 => {}
            1 => one += 1,
            _ => more.push((i % width, i / width, [g[0], g[1], g[2]], [c[0], c[1], c[2]])),
        }
    }
    let shown: Vec<_> = more.iter().take(8).collect();
    eprintln!(
        "{label}: {one} pixels off by one, {} off by more {shown:?}",
        more.len()
    );
    (one, more.len())
}

#[test]
fn gpu_grid_matches_the_cpu_painter_at_every_scale() {
    for scale in [1.0, 1.25, 2.5] {
        for cursor in [Cursor::Block, Cursor::Underline, Cursor::Bar] {
            let label = format!("scale {scale} {cursor:?}");
            let raster = Raster::for_test(scale, 13.0, cursor).unwrap();
            let raster = Arc::new(Mutex::new(raster));
            let grid = screen((3, 1));
            let rows = grid.rows;
            let cpu = raster.lock().unwrap().render(&grid);
            let frame = gpu_frames(raster, scale, vec![(grid, vec![true; rows])]);
            let (_, more) = compare(&frame.gpu, &cpu, frame.width, &label);
            assert_eq!(more, 0, "{label}");
            assert!(frame.stats.glyphs > 0, "{label}: no glyphs drawn");
        }
    }
}

#[test]
fn a_dirty_row_rebuilds_alone_and_the_frame_still_matches() {
    let scale = 1.25;
    let raster = Arc::new(Mutex::new(
        Raster::for_test(scale, 13.0, Cursor::Block).unwrap(),
    ));
    let first = screen((0, 0));
    let mut second = screen((5, 2));
    let row = 2;
    let cols = second.cols;
    for cell in &mut second.cells[row * cols..(row + 1) * cols] {
        cell.c = 'Z';
        cell.extra = 0;
        cell.width = Default::default();
    }
    let mut dirty = vec![false; second.rows];
    dirty[row] = true;
    let cpu = raster.lock().unwrap().render(&second);
    let rows = first.rows;
    let frame = gpu_frames(
        raster,
        scale,
        vec![(first, vec![true; rows]), (second, dirty)],
    );
    assert_eq!(frame.stats.rows_built, 1, "only the dirty row rebuilds");
    assert!(frame.stats.atlas_bytes > 0, "'Z' is a new glyph");
    let (_, more) = compare(&frame.gpu, &cpu, frame.width, "dirty row");
    assert_eq!(more, 0);
}

/// Two panes share the atlas. When the right pane empties it during its
/// reserve phase (as a full atlas does), the left pane, which has no new
/// output and was drawn in an earlier frame, must rebuild from its retained
/// snapshot and draw its own glyphs, not whatever the right pane put there.
#[test]
fn an_atlas_reset_by_one_pane_rebuilds_an_idle_neighbour() {
    let scale = 1.0;
    let raster = Arc::new(Mutex::new(
        Raster::for_test(scale, 13.0, Cursor::Block).unwrap(),
    ));
    let left_screen = screen((1, 0));
    let mut right_first = screen((2, 1));
    let mut right_second = screen((4, 3));
    let fill = |s: &mut Screen, chars: &[char]| {
        for (i, cell) in s.cells.iter_mut().enumerate() {
            cell.c = chars[i % chars.len()];
            cell.extra = 0;
            cell.width = Default::default();
        }
    };
    fill(&mut right_first, &['#', '%', '&']);
    fill(&mut right_second, &['K', 'L', 'W', '@', '8']);
    let (cpu_left, cpu_right) = {
        let mut r = raster.lock().unwrap();
        (r.render(&left_screen), r.render(&right_second))
    };
    let (width, height) = raster.lock().unwrap().target_size(&left_screen);
    let state = create_render_state(
        default_wgpu_setup(),
        egui_wgpu::RendererOptions::PREDICTABLE,
    );
    term_render::install(&state);
    let (left, right) = (Pane::new(), Pane::new());
    let size = egui::vec2(width as f32 / scale, height as f32 / scale);
    let (ui_left, ui_right, ui_raster) = (left.clone(), right.clone(), raster.clone());
    let mut harness = Harness::builder()
        .with_size(egui::vec2(size.x * 2.0, size.y))
        .with_pixels_per_point(scale)
        .renderer(WgpuTestRenderer::from_render_state(state))
        .build_ui(move |ui| {
            let painter = ui.painter();
            painter.add(ui_left.callback(
                egui::Rect::from_min_size(egui::Pos2::ZERO, size),
                ui_raster.clone(),
            ));
            painter.add(ui_right.callback(
                egui::Rect::from_min_size(egui::pos2(size.x, 0.0), size),
                ui_raster.clone(),
            ));
        });
    let rows = left_screen.rows;
    left.update(left_screen, vec![true; rows]);
    right.update(right_first, vec![true; rows]);
    harness.step();
    harness.render().expect("first frame");
    right.update(right_second, vec![true; rows]);
    right.reset_atlas_next_frame();
    harness.step();
    let image = harness.render().expect("second frame");
    assert_eq!(
        left.stats().rows_built,
        rows,
        "the idle pane rebuilt every row"
    );
    let crop = |x0: u32| {
        let mut out = Vec::new();
        for y in 0..height {
            for x in x0..x0 + width {
                out.extend_from_slice(&image.get_pixel(x, y).0);
            }
        }
        out
    };
    let (_, more) = compare(&crop(0), &cpu_left, width as usize, "idle left pane");
    assert_eq!(more, 0);
    let (_, more) = compare(&crop(width), &cpu_right, width as usize, "right pane");
    assert_eq!(more, 0);
}
