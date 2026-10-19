// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! CPU cost of the GPU grid on the 2026-10-07 renderer workloads: a
//! 2250x1250 physical grid at scale 2.5, 20 warm-up and 200 measured frames
//! each. `prepare` is the renderer's whole CPU side per frame (snapshot to
//! instances, uploads queued). Frame time here includes a software GPU
//! rasterising the frame, so it is not desktop frame time.
//!
//! `cargo test --release -p term-render --test bench -- --ignored --nocapture`
use egui_kittest::Harness;
use egui_kittest::wgpu::{WgpuTestRenderer, create_render_state, default_wgpu_setup};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use term_core::config::Cursor;
use term_core::raster::Raster;
use term_core::terminal::{Cell, Screen};
use term_render::Pane;

const SCALE: f32 = 2.5;

fn screen(cols: usize, rows: usize, cells: &[Cell], cursor: (usize, usize)) -> Screen {
    Screen {
        clusters: Default::default(),
        cols,
        rows,
        display_offset: 0,
        cursor,
        cursor_visible: true,
        cells: cells.to_vec(),
        updated: Instant::now(),
    }
}

fn summary(label: &str, mut samples: Vec<f64>) {
    samples.sort_by(f64::total_cmp);
    let mean = samples.iter().sum::<f64>() / samples.len() as f64;
    eprintln!(
        "{label}: mean={mean:.3}ms p50={:.3}ms p99={:.3}ms",
        samples[samples.len() / 2],
        samples[samples.len() * 99 / 100]
    );
}

#[test]
#[ignore = "benchmark; run with --ignored --nocapture"]
fn gpu_grid_workloads() {
    let raster = Raster::for_test(SCALE, 13.0, Cursor::Block).unwrap();
    let (cw, ch) = raster.cell();
    let (cols, rows) = ((2250 / cw) as usize, (1250 / ch) as usize);
    let text = b"The quick brown fox jumps over the lazy dog 0123456789 {}[]()<>=+-*/ ";
    let base: Vec<Cell> = (0..cols * rows)
        .map(|i| Cell {
            c: char::from(text[i % text.len()]),
            extra: 0,
            width: Default::default(),
            fg: [210, 214, 220],
            bg: [24, 26, 32],
            bold: false,
        })
        .collect();
    let raster = Arc::new(Mutex::new(raster));
    let (width, height) = raster
        .lock()
        .unwrap()
        .target_size(&screen(cols, rows, &base, (0, 0)));
    let state = create_render_state(
        default_wgpu_setup(),
        egui_wgpu::RendererOptions::PREDICTABLE,
    );
    eprintln!(
        "adapter: {:?}; grid {cols}x{rows} = {width}x{height} px at scale {SCALE}",
        state.adapter.get_info().name
    );
    term_render::install(&state);
    let pane = Pane::new();
    let size = egui::vec2(width as f32 / SCALE, height as f32 / SCALE);
    let (ui_pane, ui_raster) = (pane.clone(), raster.clone());
    let mut harness = Harness::builder()
        .with_size(size)
        .with_pixels_per_point(SCALE)
        .renderer(WgpuTestRenderer::from_render_state(state))
        .build_ui(move |ui| {
            let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
            ui.painter().add(ui_pane.callback(rect, ui_raster.clone()));
        });
    let mut frame = |screen: Screen, dirty: Vec<bool>| -> (f64, f64) {
        pane.update(screen, dirty);
        let started = Instant::now();
        harness.step();
        harness.render().expect("render");
        let wall = started.elapsed().as_secs_f64() * 1e3;
        (pane.stats().prepare_ns as f64 / 1e6, wall)
    };
    frame(screen(cols, rows, &base, (0, 0)), vec![true; rows]);
    type Step = Box<dyn Fn(usize, &mut Vec<Cell>) -> ((usize, usize), Vec<bool>)>;
    let workloads: Vec<(&str, Step)> = vec![
        (
            "single-cell echo",
            Box::new(move |i, cells| {
                let (col, row) = (i % cols, rows / 2);
                cells[row * cols + col].c = if i % 2 == 0 { 'x' } else { 'y' };
                let mut dirty = vec![false; rows];
                dirty[row] = true;
                (((col + 1) % cols, row), dirty)
            }),
        ),
        (
            "cursor only",
            Box::new(move |i, _| ((i % cols, rows / 2), vec![false; rows])),
        ),
        (
            "row echo",
            Box::new(move |i, cells| {
                let row = rows / 2;
                for cell in &mut cells[row * cols..(row + 1) * cols] {
                    cell.c = if i % 2 == 0 { 'm' } else { 'w' };
                }
                let mut dirty = vec![false; rows];
                dirty[row] = true;
                ((0, row), dirty)
            }),
        ),
        (
            "full viewport scroll",
            Box::new(move |_, cells| {
                cells.rotate_left(cols);
                ((0, rows - 1), vec![true; rows])
            }),
        ),
    ];
    for (label, step) in workloads {
        let mut cells = base.clone();
        let mut prepare = Vec::new();
        let mut wall = Vec::new();
        for i in 0..220 {
            let (cursor, dirty) = step(i, &mut cells);
            let (p, w) = frame(screen(cols, rows, &cells, cursor), dirty);
            if i >= 20 {
                prepare.push(p);
                wall.push(w);
            }
        }
        summary(&format!("{label} prepare"), prepare);
        summary(&format!("{label} frame incl. software GPU"), wall);
    }
}
