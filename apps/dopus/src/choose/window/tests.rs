// SPDX-License-Identifier: MIT OR Apache-2.0
use super::*;
use serde_json::json;

fn window(body: serde_json::Value) -> Window {
    let request = Request::parse(&body.to_string(), &crate::choose::defaults()).unwrap();
    let mut window = Window::for_request("0000000000000001-1", "local:agent", request);
    window.ready = true;
    window
}

#[test]
fn accept_cancel_close_failure_and_first_end_wins() {
    let dir = tempfile::tempdir().unwrap();
    for end in [End::Cancel, End::Caller, End::Failed("lost surface".into())] {
        let mut window = window(json!({"directory":true,"current_folder":dir.path()}));
        window.finish(end.clone());
        window.accept();
        window.finish(End::Cancel);
        assert_eq!(
            format!("{:?}", window.take_end().unwrap()),
            format!("{end:?}")
        );
        assert!(window.take_end().is_none());
        assert!(window.ending);
    }
    let mut window = window(json!({"directory":true,"current_folder":dir.path()}));
    window.accept();
    window.finish(End::Cancel);
    assert!(matches!(window.take_end(), Some(End::Accept(_))));
}

#[test]
fn no_input_before_the_immediate_reply() {
    let mut window = window(json!({"directory":true}));
    window.ready = false;
    window.finish(End::Cancel);
    window.accept();
    assert!(!window.ending);
    assert!(window.take_end().is_none());
}

#[test]
fn native_close_and_escape_are_cancel() {
    for close in [true, false] {
        let mut window = window(json!({"directory":true}));
        let ctx = egui::Context::default();
        toolkit::install(&ctx, &toolkit::Theme::embedded());
        let mut input = egui::RawInput::default();
        if close {
            input
                .viewports
                .get_mut(&egui::ViewportId::ROOT)
                .unwrap()
                .events
                .push(egui::ViewportEvent::Close);
        } else {
            input.events.push(egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            });
        }
        ctx.run_ui(input, |ui| window.show(ui))
            .drop_without_applying_deltas();
        assert!(matches!(window.take_end(), Some(End::Cancel)));
    }
}

#[test]
fn save_name_extension_and_slash_navigation_never_create_files() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("sub")).unwrap();
    let mut window = window(
        json!({"mode":"save","current_folder":dir.path(),"current_name":"first",
        "current_filter":{"name":"Text","patterns":["*.txt"],"mime_types":[]}}),
    );
    window.name = "sub/".into();
    window.accept();
    assert!(!window.ending);
    assert!(window.name.is_empty());
    assert_eq!(
        window.app.core.pane(PaneId::Left).path,
        dir.path().join("sub")
    );
    window.name = "absent/folder".into();
    window.accept();
    assert!(window.message.is_some());
    window.name = "second".into();
    window.accept();
    let Some(End::Accept(accept)) = window.take_end() else {
        panic!("expected acceptance");
    };
    assert_eq!(path(&accept.paths[0]), dir.path().join("sub/second.txt"));
    assert!(!dir.path().join("sub/second.txt").exists());
}

#[test]
fn save_files_confirms_collisions_once_and_preserves_all_names_and_order() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), "original").unwrap();
    std::fs::write(dir.path().join("c.txt"), "other").unwrap();
    let mut window = window(
        json!({"mode":"save_files","current_folder":dir.path(),"files":["c.txt","b.txt","a.txt"]}),
    );
    window.accept();
    assert!(!window.ending);
    assert!(window.replace.is_some());
    window.command("chooser.keep");
    assert!(window.replace.is_none());
    window.accept();
    window.command("chooser.replace");
    let Some(End::Accept(accept)) = window.take_end() else {
        panic!("expected acceptance");
    };
    assert_eq!(
        accept.paths.iter().map(|p| path(p)).collect::<Vec<_>>(),
        ["c.txt", "b.txt", "a.txt"].map(|n| dir.path().join(n))
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
        "original"
    );
    assert!(!dir.path().join("b.txt").exists());
}

#[test]
fn original_non_utf8_name_survives_without_lossy_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let mut window = window(
        json!({"mode":"save","current_folder":dir.path(),"current_name":{"bytes":"raw%FF.txt"}}),
    );
    window.accept();
    let Some(End::Accept(accept)) = window.take_end() else {
        panic!("expected acceptance");
    };
    assert_eq!(
        path(&accept.paths[0]).file_name().unwrap().as_bytes(),
        b"raw\xff.txt"
    );
}

#[test]
fn filtered_rows_and_keyboard_selection_share_the_same_projection() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("folder")).unwrap();
    for name in ["one.txt", "two.png", "three.txt"] {
        std::fs::write(dir.path().join(name), "x").unwrap();
    }
    let mut window = window(
        json!({"current_folder":dir.path(),"current_filter":{"name":"Text","patterns":["*.txt"],"mime_types":[]}}),
    );
    for _ in 0..100 {
        window.pump();
        if !window.app.core.pane(PaneId::Left).listing {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let rows = window.app.core.visible_rows(PaneId::Left);
    assert_eq!(rows.len(), 3);
    assert!(
        rows.iter()
            .all(|r| r.entry.is_dir || r.entry.name.ends_with(".txt"))
    );
    window.command("selection.last");
    assert!(
        window
            .app
            .core
            .pane(PaneId::Left)
            .selected
            .as_ref()
            .unwrap()
            .extension()
            .unwrap()
            == "txt"
    );
}
