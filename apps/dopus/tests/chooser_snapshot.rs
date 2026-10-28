// SPDX-License-Identifier: MIT OR Apache-2.0
//! Chooser snapshots use the real one-pane core and shared DOpus view.
use design::{DesignContext, Mode, Scheme};
use dopus::choose::{defaults, window::Window};
use dopus_core::chooser::Request;
use egui_kittest::{Harness, kittest::Queryable};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime};

fn home() -> &'static Path {
    static HOME: OnceLock<PathBuf> = OnceLock::new();
    HOME.get_or_init(|| {
        let home = std::env::temp_dir().join("dopus-chooser-snapshots/home");
        std::fs::create_dir_all(home.join("Documents")).unwrap();
        let app_home = home.parent().unwrap().join("dopus-state");
        std::fs::create_dir_all(app_home.join("config")).unwrap();
        std::fs::write(
            app_home.join("config/config.conf.mix"),
            format!(
                "schema_version: 2\nbookmarks: [{{name: \"My archive\", path: \"{}\"}}]\n",
                home.join("Documents").display()
            ),
        )
        .unwrap();
        for (name, contents) in [
            ("notes.txt", "Notes"),
            ("report.pdf", "Report"),
            ("photo.png", "Picture"),
        ] {
            std::fs::write(home.join(name), contents).unwrap();
        }
        let when = SystemTime::UNIX_EPOCH + Duration::from_secs(1_791_711_000);
        for entry in std::fs::read_dir(&home).unwrap() {
            std::fs::File::open(entry.unwrap().path())
                .unwrap()
                .set_modified(when)
                .unwrap();
        }
        // SAFETY: set once before the first core is made; every test uses it.
        unsafe {
            std::env::set_var("HOME", &home);
            std::env::set_var("TZ", "UTC");
            std::env::set_var("MIXOS_APP_HOME", &app_home);
        }
        home
    })
}

fn snapshot(kind: &str, mode: Mode) {
    snapshot_at(kind, mode, false);
}

fn snapshot_at(kind: &str, mode: Mode, minimum: bool) {
    let folder = home();
    let mut body = match kind {
        "open" => json!({}),
        "multiple" => json!({"multiple":true}),
        "directory" => json!({"directory":true}),
        "save" => json!({"mode":"save", "current_name":"drawing", "filters":[
            {"name":"PNG images", "patterns":["*.png"], "mime_types":[]},
            {"name":"All files", "patterns":["*"], "mime_types":[]}
        ], "choices":[
            {"id":"encoding", "label":"Encoding", "options":[{"id":"utf8","label":"UTF-8"},{"id":"latin1","label":"Latin-1"}]},
            {"id":"preview", "label":"Include preview", "options":[], "default":"true"}
        ]}),
        "save_files" => json!({"mode":"save_files","files":["one.txt","two.txt"]}),
        "replace" => json!({"mode":"save_files","files":["notes.txt","report.pdf"]}),
        "replace_one" => json!({"mode":"save","current_name":"notes.txt"}),
        _ => unreachable!(),
    };
    body["current_folder"] = json!(folder);
    body["on_behalf_of"] = json!("fakeapp");
    let request = Request::parse(&body.to_string(), &defaults()).unwrap();
    let caller = if kind == "open" {
        "local:agent"
    } else {
        "local:portald"
    };
    let mut window = Window::for_request("0123456789abcdef-1", caller, request);
    window.ready = true;
    for _ in 0..50 {
        window.pump();
        std::thread::sleep(Duration::from_millis(10));
    }
    if kind == "multiple" {
        for name in ["notes.txt", "report.pdf"] {
            window.app.core.select_modified(
                dopus_core::PaneId::Left,
                folder.join(name),
                true,
                false,
            );
        }
    }
    if kind == "replace" || kind == "replace_one" {
        window.accept();
        assert!(window.replace.is_some());
    }
    let theme = toolkit::Theme::for_context(DesignContext {
        scheme: Scheme::Studio,
        mode,
        ..Default::default()
    });
    let mut harness = Harness::builder()
        .with_context_setup(move |ctx| toolkit::install(ctx, &theme))
        .with_size(if minimum {
            egui::Vec2::from(toolkit::window::MIN_SIZE)
        } else {
            egui::vec2(820.0, 600.0)
        })
        .wgpu()
        .build_ui(move |ui| window.show(ui));
    harness.run();
    if minimum {
        for _ in 0..4 {
            harness.run();
        }
        let bounds = egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::Vec2::from(toolkit::window::MIN_SIZE),
        );
        if kind != "replace" {
            assert!(bounds.contains_rect(harness.get_by_label("Cancel").rect()));
        }
        if kind == "save" {
            let name = harness.get_by_role_and_label(egui::accesskit::Role::TextInput, "File name");
            assert!(bounds.contains_rect(name.rect()));
        }
    }
    harness.snapshot(format!(
        "{}chooser_{kind}_{}",
        if minimum { "minimum_window_" } else { "" },
        if mode == Mode::Dark { "dark" } else { "light" }
    ));
}

#[test]
fn minimum_window_open() {
    snapshot_at("open", Mode::Dark, true);
}

#[test]
fn minimum_window_save() {
    snapshot_at("save", Mode::Dark, true);
}

#[test]
fn minimum_window_replace() {
    snapshot_at("replace", Mode::Dark, true);
}

macro_rules! snapshots {
    ($( $name:ident, $kind:literal, $mode:ident; )*) => { $(
        #[test]
        fn $name() { snapshot($kind, Mode::$mode); }
    )* };
}

snapshots! {
    open_dark, "open", Dark;
    open_light, "open", Light;
    multiple_dark, "multiple", Dark;
    multiple_light, "multiple", Light;
    directory_dark, "directory", Dark;
    directory_light, "directory", Light;
    save_dark, "save", Dark;
    save_light, "save", Light;
    save_files_dark, "save_files", Dark;
    save_files_light, "save_files", Light;
    replace_dark, "replace", Dark;
    replace_light, "replace", Light;
    replace_one_dark, "replace_one", Dark;
    replace_one_light, "replace_one", Light;
}
