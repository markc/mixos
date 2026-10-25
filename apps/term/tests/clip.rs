// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! A pane narrower than its smallest grid (two columns) clips its grid at
//! its own border: no pixel of it lands in the neighbouring pane.

use egui_kittest::Harness;
use egui_kittest::wgpu::{WgpuTestRenderer, create_render_state, default_wgpu_setup};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use term::app::App;
use term_core::config::{Config, Cursor, Settings};
use term_core::font::FontSize;
use term_core::panes::{Geometry, SplitDir};
use term_core::raster::Raster;
use term_core::tabs::{Cleanup, TabSet};
use term_core::terminal::Terminal;

#[test]
fn a_tiny_pane_never_draws_over_its_neighbour() {
    if !::paths::root(::paths::Root::Prefix)
        .join("bin/mix")
        .is_file()
    {
        eprintln!("skipping: the PTY tests need Mix at /opt/mixos/bin/mix");
        return;
    }
    let settings = Settings {
        config: Config::default(),
        term: "xterm-256color".into(),
    };
    let mut tabs = TabSet::with_session(settings, None).expect("a Mix PTY");
    let left = tabs.active_tab().active_pane;
    let right = tabs.split_active(SplitDir::Vertical).expect("a split");
    // The left grid is solid red; the right one is the default background.
    for (id, text) in [(left, "\x1b[41m    "), (right, "")] {
        let terminal = tabs.pane_by_id(id).unwrap();
        *terminal.lock().unwrap() = Terminal::from_test_vt(2, 2, text.as_bytes());
    }
    let tabs = Arc::new(Mutex::new(tabs));
    let (cleanup, reaper) = Cleanup::start().expect("cleanup worker");
    let raster = Raster::for_test(1.0, 13.0, Cursor::Underline).expect("the test font");
    let cell = raster.width;
    let mut app = App::new(
        tabs.clone(),
        cleanup.clone(),
        None,
        Arc::new(AtomicBool::new(true)),
        raster,
        FontSize::new(13.0),
    );
    app.sync();
    // Each pane is narrower than two cells plus its padding.
    let inset = term::layout::inset(1.0, term::layout::Edges::default());
    let width = 2.0 * (cell as f32 + inset);
    app.layout(Geometry {
        x: 0.0,
        y: 0.0,
        w: width,
        h: 48.0,
    });
    let placed = app.placed();
    assert_eq!(placed.len(), 2);
    let right_pane = placed[1].1;
    assert!(
        right_pane.x < 2.0 * cell as f32 + inset,
        "the left grid would reach it"
    );
    app.paint();

    let state = create_render_state(
        default_wgpu_setup(),
        egui_wgpu::RendererOptions::PREDICTABLE,
    );
    term_render::install(&state);
    let mut harness = Harness::builder()
        .with_size(egui::vec2(width, 48.0))
        .renderer(WgpuTestRenderer::from_render_state(state))
        .build_ui_state(|ui, app: &mut App| term::view::panes(ui, app), app);
    harness.run_steps(2);
    let image = harness.render().expect("offscreen render");
    // The left grid's red, sampled inside it; the right pane must not
    // show it anywhere.
    let pixel = |x: u32, y: u32| image.get_pixel(x, y).0;
    let ink = pixel(inset as u32 + 1, inset as u32 + 3);
    let right_inside = pixel(right_pane.x as u32 + 3, 30);
    assert_ne!(
        ink, right_inside,
        "the left grid is drawn, in its own colour"
    );
    let red = |x: u32, y: u32| pixel(x, y) == ink;
    // Nothing red inside the right pane.
    for x in right_pane.x as u32..width as u32 {
        for y in 0..48 {
            assert!(
                !red(x, y),
                "left grid drawn at ({x}, {y}) in the right pane"
            );
        }
    }

    drop(harness);
    let removed = tabs.lock().unwrap().shutdown();
    cleanup.submit(removed);
    drop(cleanup);
    reaper.join().unwrap();
}
