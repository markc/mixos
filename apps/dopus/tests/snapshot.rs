// SPDX-License-Identifier: MIT OR Apache-2.0
//! Offscreen snapshots of dopus driven through its real core and view, over
//! a fixed scratch home: Places and Properties open, a folder expanded in
//! the tree, a file selected, the right pane in Documents; the sidebars
//! closed; and each dialog. Regenerate with `UPDATE_SNAPSHOTS=1` and look
//! at the images before committing them.
//!
//! The pictures depend on the home directory (Places, `~` in paths), the
//! time zone and file times, so the first test to run pins them all:
//! `HOME` is the scratch home, `TZ` is UTC, and every entry is dated
//! 2026-10-11 09:30.
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant, SystemTime};

use design::{DesignContext, Mode, Scheme};
use dopus::app::{App, Dialog};
use dopus::{commands, strings, view::view};
use dopus_core::{DOpusConfig, DopusCore, PaneId, PromptKind};
use egui_kittest::Harness;
use toolkit::{Theme, icons};

/// 2026-10-11 09:30 UTC.
const WHEN: u64 = 1_791_711_000;

/// The scratch home, built once; `HOME` and `TZ` point at the fixture.
fn home() -> &'static Path {
    static HOME: OnceLock<PathBuf> = OnceLock::new();
    HOME.get_or_init(|| {
        let home = std::env::temp_dir().join("dopus-snapshots").join("home");
        let _ = std::fs::remove_dir_all(&home);
        let files: &[(&str, usize)] = &[
            ("notes.md", 2_400),
            ("report.pdf", 182_000),
            ("photo.png", 1_250_000),
            ("song.flac", 24_000_000),
            ("build.rs", 640),
            ("archive.tar.gz", 3_400_000),
            ("run.sh", 120),
            (".profile", 80),
            ("Documents/plan.odt", 18_000),
            ("Documents/budget.ods", 9_500),
            ("Documents/slides.odp", 410_000),
            ("Documents/readme.txt", 300),
            ("Documents/Letters/to-the-council.md", 1_900),
            ("Downloads/installer.iso", 70_000),
            ("Music/track.mp3", 5_000_000),
            ("Pictures/holiday.jpg", 2_100_000),
        ];
        for (name, size) in files {
            let path = home.join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, vec![b'x'; *size]).unwrap();
        }
        let when = SystemTime::UNIX_EPOCH + Duration::from_secs(WHEN);
        let mut all = vec![home.clone()];
        let mut at = 0;
        while at < all.len() {
            if all[at].is_dir() {
                for entry in std::fs::read_dir(&all[at]).unwrap() {
                    all.push(entry.unwrap().path());
                }
            }
            at += 1;
        }
        for path in all.iter().rev() {
            std::fs::File::open(path)
                .and_then(|f| f.set_modified(when))
                .unwrap();
        }
        // SAFETY: set once, before any core reads them, by the first test
        // to get here; every test sets the same values.
        unsafe {
            std::env::set_var("HOME", &home);
            std::env::set_var("TZ", "UTC");
        }
        home
    })
}

/// Feed the core every event until none arrives for a while.
fn settle(app: &mut App, events: &std::sync::mpsc::Receiver<dopus_core::CoreEvent>) {
    while let Ok(event) = events.recv_timeout(Duration::from_millis(400)) {
        let derived = app.core.on_event(event);
        app.absorb(derived);
    }
    let derived = app.core.tick(Instant::now());
    app.absorb(derived);
}

/// The app over the fixture: left in the home with Documents expanded and
/// notes.md selected, right in Documents, both sidebars as `sidebars`.
fn app(sidebars: bool) -> App {
    let home = home();
    let mut config = DOpusConfig::default();
    config.left.path = home.to_owned();
    config.right.path = home.join("Documents");
    config.places.open = sidebars;
    config.properties.open = sidebars;
    let (core, events) = DopusCore::new(config, None);
    let mut app = App::new(core);
    settle(&mut app, &events);
    app.core.toggle_expand(PaneId::Left, &home.join("Documents"));
    settle(&mut app, &events);
    app.core
        .select_modified(PaneId::Left, home.join("notes.md"), false, false);
    settle(&mut app, &events);
    app
}

fn window(theme: Theme, name: &str, sidebars: bool, dialog: Option<Dialog>) {
    let mut app = app(sidebars);
    if let Some(dialog) = dialog {
        app.open(dialog);
    }
    let registry = commands::registry();
    let strings = strings();
    let stroke = icons::stroke_width(&theme);
    let mut harness = Harness::builder()
        .with_size(egui::vec2(980.0, 640.0))
        .wgpu()
        .build_ui(move |ui| {
            let _ = view(ui, &app, &registry, &strings, stroke);
        });
    toolkit::install(&harness.ctx, &theme);
    harness.run();
    harness.snapshot(name);
}

fn chrome(scheme: Scheme, mode: Mode) -> Theme {
    Theme::for_context(DesignContext {
        scheme,
        mode,
        ..DesignContext::default()
    })
}

/// The default look: Studio dark.
#[test]
fn window_studio_dark() {
    window(
        chrome(Scheme::Studio, Mode::Dark),
        "window_studio_dark",
        true,
        None,
    );
}

#[test]
fn window_studio_light() {
    window(
        chrome(Scheme::Studio, Mode::Light),
        "window_studio_light",
        true,
        None,
    );
}

#[test]
fn window_pro_light() {
    window(
        chrome(Scheme::Pro, Mode::Light),
        "window_pro_light",
        true,
        None,
    );
}

/// A hue scheme.
#[test]
fn window_ocean_dark() {
    window(
        chrome(Scheme::Ocean, Mode::Dark),
        "window_ocean_dark",
        true,
        None,
    );
}

/// Both sidebars closed: the panes take the width; the toggles stay beside
/// the path fields.
#[test]
fn sidebars_closed_studio_dark() {
    window(
        chrome(Scheme::Studio, Mode::Dark),
        "sidebars_closed_studio_dark",
        false,
        None,
    );
}

#[test]
fn confirm_delete_studio_light() {
    window(
        chrome(Scheme::Studio, Mode::Light),
        "confirm_delete_studio_light",
        true,
        Some(Dialog::Confirm {
            token: 1,
            message: "Delete notes.md?".into(),
        }),
    );
}

#[test]
fn prompt_new_folder_studio_dark() {
    window(
        chrome(Scheme::Studio, Mode::Dark),
        "prompt_new_folder_studio_dark",
        true,
        Some(Dialog::Prompt {
            token: 1,
            kind: PromptKind::NewFolder,
            text: "New Folder".into(),
        }),
    );
}

#[test]
fn drop_choice_studio_dark() {
    let home = home();
    window(
        chrome(Scheme::Studio, Mode::Dark),
        "drop_choice_studio_dark",
        true,
        Some(Dialog::Drop {
            source: PaneId::Left,
            paths: vec![home.join("notes.md"), home.join("report.pdf")],
            destination: home.join("Documents"),
        }),
    );
}

#[test]
fn shortcuts_studio_light() {
    window(
        chrome(Scheme::Studio, Mode::Light),
        "shortcuts_studio_light",
        true,
        Some(Dialog::Shortcuts),
    );
}
