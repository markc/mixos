// SPDX-License-Identifier: MIT OR Apache-2.0
//! The window offscreen, through real egui-wgpu with the GPU grid: two tabs,
//! the second split side by side, each pane a VT fixture so the image is
//! the same every run. Regenerate with `UPDATE_SNAPSHOTS=1` and look at the
//! image before committing.

use design::{DesignContext, Mode, Scheme};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use egui_kittest::wgpu::{WgpuTestRenderer, create_render_state, default_wgpu_setup};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use term::app::App;
use term::bus::WindowBus;
use term::clipboard::Clipboard;
use term::shell::Shell;
use term_core::config::{Config, Cursor, Settings};
use term_core::font::FontSize;
use term_core::panes::SplitDir;
use term_core::raster::Raster;
use term_core::tabs::{Cleanup, TabSet};
use term_core::terminal::Terminal;
use toolkit::Theme;

const LISTING: &str = "\x1b[1;32muser@host\x1b[0m:\x1b[1;34m~/src\x1b[0m$ ls\r\n\
\x1b[1;34mapps\x1b[0m  \x1b[1;34mlibs\x1b[0m  Cargo.toml  README.md\r\n\
\x1b[1;32muser@host\x1b[0m:\x1b[1;34m~/src\x1b[0m$ ";

const BUILD: &str = "$ cargo test -p term\r\n\
   \x1b[1;32mCompiling\x1b[0m term v0.4.0\r\n\
test result: \x1b[32mok\x1b[0m. 42 passed; 0 failed\r\n$ ";

const EDITOR: &str = "\x1b[7m  notes.md  \x1b[0m\r\n\
# Notes\r\n\
- split panes \u{2502} tabs\r\n\
- \u{2588}\u{2593}\u{2592}\u{2591} blocks, caf\u{e9}\r\n";

/// The window's state until its first frame builds the shell.
struct Holder {
    start: Option<(Theme, App)>,
    shell: Option<Shell>,
}

fn replace(tabs: &TabSet, id: u64, text: &str) {
    let terminal = tabs.pane_by_id(id).expect("a pane");
    *terminal.lock().unwrap() = Terminal::from_test_vt(40, 12, text.as_bytes());
}

#[test]
fn two_tabs_and_a_split_studio_dark() {
    let settings = Settings {
        config: Config::default(),
        term: "xterm-256color".into(),
    };
    let mut tabs = TabSet::with_session(settings, None).expect("a Mix PTY");
    let first_tab = tabs.active_id();
    replace(&tabs, tabs.active_tab().active_pane, EDITOR);
    tabs.set_title(first_tab, "notes".into()).unwrap();
    let second_tab = tabs.open().expect("a second tab");
    let left = tabs.active_tab().active_pane;
    let right = tabs.split_active(SplitDir::Vertical).expect("a split");
    replace(&tabs, left, LISTING);
    replace(&tabs, right, BUILD);
    tabs.set_title(second_tab, "src".into()).unwrap();
    let tabs = Arc::new(Mutex::new(tabs));
    let (cleanup, reaper) = Cleanup::start().expect("cleanup worker");
    let raster = Raster::for_test(1.0, 13.0, Cursor::Underline).expect("the test font");
    let app = App::new(
        tabs.clone(),
        cleanup.clone(),
        None,
        Arc::new(AtomicBool::new(true)),
        raster,
        FontSize::new(13.0),
    );
    let theme = Theme::for_context(DesignContext {
        scheme: Scheme::Studio,
        mode: Mode::Dark,
        ..DesignContext::default()
    });
    let state = create_render_state(
        default_wgpu_setup(),
        egui_wgpu::RendererOptions::PREDICTABLE,
    );
    term_render::install(&state);
    let holder = Holder {
        start: Some((theme.clone(), app)),
        shell: None,
    };
    let mut harness = Harness::builder()
        .with_size(egui::vec2(820.0, 420.0))
        .renderer(WgpuTestRenderer::from_render_state(state))
        .build_ui_state(
            |ui, h: &mut Holder| {
                if let Some((theme, app)) = h.start.take() {
                    h.shell = Some(Shell::new(
                        ui.ctx().clone(),
                        theme,
                        app,
                        WindowBus::offline(),
                        Clipboard::none(),
                    ));
                }
                let shell = h.shell.as_mut().unwrap();
                shell.logic(ui.ctx());
                shell.ui(ui);
            },
            holder,
        );
    toolkit::install(&harness.ctx, &theme);
    harness.run_steps(4);
    {
        let app = &harness.state().shell.as_ref().unwrap().app;
        assert_eq!(app.shape.tabs.len(), 2);
        assert_eq!(app.shape.visible(), vec![left, right]);
        assert!(
            app.shape.focus_ring(right),
            "the split shows its focus ring"
        );
        for id in [left, right] {
            assert!(
                app.painted.get(&id).is_some_and(|n| *n > 0),
                "pane {id} drawn"
            );
        }
        let (l, r) = (app.grids[&left], app.grids[&right]);
        assert_eq!(l.1, r.1, "side by side panes share their rows");
    }
    harness.snapshot("two_tabs_and_a_split_studio_dark");

    // Exercise the real offline debounce and show its persistent notice.
    harness
        .state_mut()
        .shell
        .as_mut()
        .unwrap()
        .app
        .zoom(FontSize::increase);
    harness.run_steps(2);
    std::thread::sleep(std::time::Duration::from_millis(410));
    harness.run_steps(2);
    harness.get_by_label("Zoom is not saved");
    harness.snapshot("unsaved_zoom_studio_dark");
    let zoom = harness.state().shell.as_ref().unwrap().app.font().current();
    harness.run_steps(2);
    assert_eq!(
        harness.state().shell.as_ref().unwrap().app.font().current(),
        zoom
    );
    harness.state_mut().shell.as_mut().unwrap().app.reset_font();
    harness.run_steps(2);

    // Selecting the first tab from the strip shows its one pane.
    harness
        .state_mut()
        .shell
        .as_mut()
        .unwrap()
        .app
        .select_tab(first_tab);
    tabs.lock().unwrap().refresh_titles();
    harness.state_mut().shell.as_mut().unwrap().app.sync();
    harness.run_steps(2);
    let app = &harness.state().shell.as_ref().unwrap().app;
    assert_eq!(app.shape.visible().len(), 1);
    assert!(!app.shape.focus_ring(app.shape.active_pane));

    drop(harness);
    let removed = tabs.lock().unwrap().shutdown();
    cleanup.submit(removed);
    drop(cleanup);
    reaper.join().unwrap();
}
