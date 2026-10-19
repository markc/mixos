// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! A weight-only settings change replaces the raster and its cached glyphs.

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use term::app::App;
use term_core::config::{Config, Cursor, Settings};
use term_core::font::FontSize;
use term_core::panes::Geometry;
use term_core::raster::Raster;
use term_core::tabs::{Cleanup, TabSet};

#[test]
fn weight_only_settings_change_rebuilds_the_raster() {
    let tabs = Arc::new(Mutex::new(
        TabSet::with_session(
            Settings {
                config: Config::default(),
                term: "xterm-256color".into(),
            },
            None,
        )
        .expect("a Mix PTY"),
    ));
    let (cleanup, reaper) = Cleanup::start().expect("cleanup worker");
    let raster = Raster::with_face(1.0, 13.0, Cursor::Underline, None, 400).unwrap();
    let mut app = App::new(
        tabs.clone(),
        cleanup.clone(),
        None,
        Arc::new(AtomicBool::new(true)),
        raster,
        FontSize::new(13.0),
    );
    let mut settings = settings::terminal::Terminal::default();
    settings.font.size = 13.0;
    app.apply_terminal(&settings, None, false);
    app.sync();
    app.layout(Geometry {
        x: 0.0,
        y: 0.0,
        w: 640.0,
        h: 240.0,
    });
    for weight in [300, 400, 500] {
        app.paint();
        assert!(
            !app.grids.is_empty(),
            "paint populated the old raster's grids"
        );
        let before = app.raster.lock().unwrap().identity();
        let size = app.font().current();
        settings.font.weight = weight;
        app.apply_terminal(&settings, None, false);
        assert!(!app.raster.lock().unwrap().same_identity(&before));
        assert_eq!(app.font().current(), size);
        assert!(
            !app.grids.is_empty(),
            "the replacement relayout resizes the PTY"
        );
        let after = app.raster.lock().unwrap().identity();
        app.apply_terminal(&settings, None, false);
        assert!(app.raster.lock().unwrap().same_identity(&after));
    }
    drop(app);
    let removed = tabs.lock().unwrap().shutdown();
    cleanup.submit(removed);
    drop(cleanup);
    reaper.join().unwrap();
}
