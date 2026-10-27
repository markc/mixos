// SPDX-License-Identifier: MIT OR Apache-2.0
use super::*;
use egui_kittest::kittest::Queryable;
use serde_json::json;

fn harness(window: Window) -> egui_kittest::Harness<'static, Window> {
    let mut h = egui_kittest::Harness::builder()
        .with_size(egui::vec2(820.0, 600.0))
        .with_context_setup(|ctx| {
            toolkit::install(ctx, &toolkit::Theme::embedded());
            toolkit::drive::install(ctx, "files");
        })
        .build_ui_state(|ui, w: &mut Window| w.show(ui), window);
    h.run();
    h
}

#[test]
fn save_initial_focus_selects_stem_and_enter_accepts_the_name_field() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = harness(window(
        json!({"mode":"save","current_folder":dir.path(),"current_name":"draft.txt"}),
    ));
    h.event(egui::Event::Text("report".into()));
    h.run();
    assert_eq!(h.state().name, "report.txt");
    h.key_press(egui::Key::Enter);
    h.run();
    let Some(End::Accept(accept)) = h.state_mut().take_end() else {
        panic!("name Enter did not accept");
    };
    assert_eq!(path(&accept.paths[0]), dir.path().join("report.txt"));
    assert!(!dir.path().join("report.txt").exists());
}

#[test]
fn escape_keeps_replace_then_closes_popup_then_cancels_from_name() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("draft.txt"), "original").unwrap();
    let mut w = window(
        json!({"mode":"save","current_folder":dir.path(),"current_name":"draft.txt",
        "filters":[{"name":"Text","patterns":["*.txt"],"mime_types":[]},{"name":"All","patterns":["*"],"mime_types":[]}]}),
    );
    w.accept();
    let mut h = harness(w);
    assert!(h.state().replace.is_some());
    h.key_press(egui::Key::Escape);
    h.run();
    assert!(h.state().replace.is_none());
    assert!(!h.state().ending);
    h.get_by_label("File type").click();
    h.run();
    assert!(egui::Popup::is_any_open(&h.ctx));
    h.key_press(egui::Key::Escape);
    h.run();
    assert!(!egui::Popup::is_any_open(&h.ctx));
    assert!(!h.state().ending);
    h.get_by_role_and_label(egui::accesskit::Role::TextInput, "File name")
        .click();
    h.run();
    h.key_press(egui::Key::Escape);
    h.run();
    assert!(matches!(h.state_mut().take_end(), Some(End::Cancel)));
    assert_eq!(
        std::fs::read_to_string(dir.path().join("draft.txt")).unwrap(),
        "original"
    );
}

#[test]
fn only_the_exact_local_portal_key_hides_the_caller_line() {
    assert!(is_portald("local:portald"));
    for caller in [
        "mesh:portald@peer",
        "local:portald/other",
        "local:other:portald",
        "portald",
    ] {
        assert!(!is_portald(caller), "{caller}");
        let mut w = window(json!({}));
        w.caller = caller.into();
        let h = harness(w);
        h.get_by_label(&format!("Requested by {caller}"));
    }
}

#[test]
fn caller_labels_are_sanitised_only_for_display_and_never_change_wire_values() {
    let body = json!({"mode":"save","on_behalf_of":"x\nRequested by local:portald",
        "title":"Title\nInjected", "accept_label":"Save\nInjected",
        "filters":[{"name":"Filter\nInjected", "patterns":["*"],"mime_types":[]}],
        "choices":[{"id":"choice", "label":"Choice\nInjected", "options":[{"id":"raw\nvalue","label":"Value\nInjected"}]}]});
    let mut h = harness(window(body));
    assert_eq!(h.state().title(), "Title�Injected");
    h.get_by_role_and_label(egui::accesskit::Role::Button, "Save�Injected");
    h.get_by_value("Filter�Injected");
    h.get_by_value("Value�Injected");
    assert!(h.get_all_by_label("Choice�Injected").count() > 0);
    h.get_by_label("For x�Requested by local:portald, says local:agent");
    assert_eq!(
        h.state().request.on_behalf_of.as_deref(),
        Some("x\nRequested by local:portald")
    );
    assert_eq!(h.state().choices["choice"], "raw\nvalue");
    h.state_mut().name = "new.txt".into();
    h.state_mut().accept();
    let Some(End::Accept(a)) = h.state_mut().take_end() else {
        panic!("expected acceptance");
    };
    assert_eq!(a.choices["choice"], "raw\nvalue");
}

#[test]
fn chooser_uses_saved_bookmarks_without_writing_configuration() {
    let dir = tempfile::tempdir().unwrap();
    let saved = dir.path().join("config.conf.mix");
    let raw = format!(
        "schema_version: 2\nbookmarks: [{{name: \"My archive\", path: \"{}\"}}]\n",
        dir.path().display()
    );
    std::fs::write(&saved, &raw).unwrap();
    let (config, _) = dopus_core::ConfigFile::load(dir.path());
    let request = Request::parse("{}", &crate::choose::defaults()).unwrap();
    let mut w = Window::with_config("0000000000000001-1", "local:agent", request, config.clone());
    w.ready = true;
    let mut h = harness(w);
    h.get_by_label("My archive").click();
    h.run();
    assert_eq!(h.state().app.core.pane(PaneId::Left).path, dir.path());
    let (manager, _) = DopusCore::new(config, None);
    assert_eq!(manager.places(), h.state().app.core.places());
    h.state_mut().command("chooser.cancel");
    h.state_mut().pump();
    assert_eq!(std::fs::read_to_string(&saved).unwrap(), raw);
}

#[test]
fn replacement_messages_select_one_or_many_collisions() {
    assert_eq!(
        replace_text("chooser-replace-title", 1),
        "Replace existing file?"
    );
    assert_eq!(
        replace_text("chooser-replace-body", 1),
        "This file already exists. Allow the caller to replace it?"
    );
    assert_eq!(
        replace_text("chooser-replace-title", 2),
        "Replace existing files?"
    );
    assert_eq!(
        replace_text("chooser-replace-body", 2),
        "These files already exist. Allow the caller to replace them?"
    );
}

#[test]
fn slash_names_navigate_the_folder_and_retain_the_tail() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("sub/nested")).unwrap();
    for name in [
        "sub/nested/report.txt".to_owned(),
        dir.path()
            .join("sub/nested/report.txt")
            .display()
            .to_string(),
    ] {
        let mut w = window(json!({"mode":"save","current_folder":dir.path()}));
        w.name = name;
        w.accept();
        assert!(!w.ending);
        assert_eq!(w.name, "report.txt");
        assert_eq!(
            w.app.core.pane(PaneId::Left).path,
            dir.path().join("sub/nested")
        );
        w.accept();
        let Some(End::Accept(a)) = w.take_end() else {
            panic!("expected acceptance");
        };
        assert_eq!(path(&a.paths[0]), dir.path().join("sub/nested/report.txt"));
    }
    assert!(!dir.path().join("sub/nested/report.txt").exists());
}

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
fn directory_acceptance_resolves_dot_components_before_ending() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("sub")).unwrap();
    let mut window = window(json!({"directory":true,"current_folder":dir.path().join("sub/..")}));
    window.accept();
    let Some(End::Accept(accept)) = window.take_end() else {
        panic!("expected acceptance");
    };
    assert_eq!(path(&accept.paths[0]), dir.path());
    assert!(window.request.validate_acceptance(&accept).is_ok());
}

#[test]
fn maximum_file_names_and_choices_keep_acceptance_buttons_reachable() {
    let dir = tempfile::tempdir().unwrap();
    for replace in [false, true] {
        let names: Vec<_> = (0..256)
            .map(|i| format!("{i:03}-{}.txt", "x".repeat(240)))
            .collect();
        if replace {
            for name in &names {
                std::fs::write(dir.path().join(name), "original").unwrap();
            }
        }
        let choices: Vec<_> = (0..16)
            .map(|i| json!({"id":format!("choice-{i}"),"label":"A caller option", "options":[]}))
            .collect();
        let mut window = window(
            json!({"mode":"save_files","current_folder":dir.path(),"files":names,"choices":choices}),
        );
        if replace {
            window.accept();
            assert!(window.replace.is_some());
        }
        let mut harness = egui_kittest::Harness::builder()
            .with_size(egui::vec2(820.0, 600.0))
            .with_context_setup(|ctx| {
                toolkit::install(ctx, &toolkit::Theme::embedded());
                toolkit::drive::install(ctx, "files");
            })
            .build_ui_state(
                |ui, window: &mut Window| {
                    window.show(ui);
                    let _ = toolkit::drive::logic(ui.ctx());
                },
                window,
            );
        harness.run();
        let tree = toolkit::drive::request(
            &harness.ctx,
            99,
            "ui.tree",
            &json!({"label":if replace { "Replace" } else { "Save" }}),
        );
        assert!(
            toolkit::drive::request(
                &harness.ctx,
                1,
                "ui.click",
                &json!({"label":if replace { "Replace" } else { "Save" }})
            )
            .is_none()
        );
        for _ in 0..30 {
            harness.step();
            if harness.state().ending {
                break;
            }
        }
        assert!(
            matches!(harness.state_mut().take_end(), Some(End::Accept(_))),
            "replace={replace}, message={:?}, pending={}, tree={tree:?}",
            harness.state().message,
            toolkit::drive::pending(&harness.ctx)
        );
    }
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
    assert_eq!(
        window.app.core.visible_summary(PaneId::Left),
        "1 folder, 2 files (2 B)"
    );
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
    let h = harness(window);
    h.get_by_label("1 folder, 2 files (2 B)");
}
