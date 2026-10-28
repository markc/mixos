// SPDX-License-Identifier: MIT OR Apache-2.0
use super::*;
use egui_kittest::{Harness, kittest::Queryable};
use serde_json::json;

type State = (FileChooser, Option<Outcome>);

fn harness(mut chooser: FileChooser) -> Harness<'static, State> {
    chooser.started = true;
    let mut harness = Harness::builder().with_size(egui::vec2(900.0, 700.0))
        .build_ui_state(|ui, state: &mut State| { if state.1.is_none() { state.1 = state.0.show(ui.ctx()); } }, (chooser, None));
    crate::install(&harness.ctx, &crate::Theme::embedded());
    harness.run_steps(3);
    harness
}

fn settle(h: &mut Harness<'_, State>) {
    for _ in 0..200 {
        h.run();
        if h.state().0.pending.is_none() { return; }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("chooser worker did not settle");
}

fn parse(value: serde_json::Value) -> ChooseRequest { crate::choose::request(&value.to_string()).unwrap() }

#[test]
fn email_attach_multiple_and_filters_keep_folders() {
    let dir = tempfile::tempdir().unwrap();
    for name in ["one.md", "two.md", "photo.png"] { std::fs::write(dir.path().join(name), "").unwrap(); }
    std::fs::create_dir(dir.path().join("Reports")).unwrap();
    let request = parse(json!({"mode":"open","multiple":true,"title":"Attach files","accept_label":"Attach",
        "current_folder":dir.path().to_str().unwrap(),"filters":[{"name":"Markdown","patterns":["*.md"]}]}));
    let mut chooser = FileChooser::from_request(request.clone());
    let Answer::Listed(listing) = open_filtered(dir.path().into(), false, Some(&request)).unwrap() else {panic!()};
    assert_eq!(listing.entries.iter().map(|e|e.name.as_str()).collect::<Vec<_>>(), ["Reports", "one.md", "two.md"]);
    chooser.entries = listing.entries;
    chooser.selection.all(&chooser.entries);
    let mut h = harness(chooser);
    h.get_by_label("Attach").click();
    settle(&mut h);
    let Some(Outcome::Picked(paths)) = &h.state().1 else {panic!("not picked")};
    assert_eq!(paths, &[dir.path().join("one.md"), dir.path().join("two.md")]);
}

#[test]
fn email_save_suggested_name_appends_extension_and_round_trips_choices() {
    let dir = tempfile::tempdir().unwrap();
    let request = parse(json!({"mode":"save","title":"Save message","current_name":"message",
        "current_folder":dir.path().to_str().unwrap(),"filters":[{"name":"Markdown","patterns":["*.md"]}],
        "choices":[{"id":"encoding","label":"Encoding","options":[{"id":"utf8","label":"UTF-8"},{"id":"utf16","label":"UTF-16"}]},
        {"id":"readonly","label":"Read only","options":[]}]}));
    let mut h = harness(FileChooser::from_request(request));
    assert_eq!(h.state().0.name, "message");
    h.get_by_label("Read only").click(); h.run();
    h.get_by_label("UTF-8").click(); h.run();
    h.get_by_label("UTF-16").click(); h.run();
    h.snapshot("file_chooser_contract_save");
    h.get_by_label("Save").click(); settle(&mut h);
    let Some(Outcome::Save {path, ..}) = &h.state().1 else {panic!("not saved")};
    assert_eq!(path, &dir.path().join("message.md"));
    let crate::choose::Outcome::Chosen {choices, current_filter, ..} = h.state().0.chosen(vec![path.clone()]) else {panic!()};
    assert_eq!(choices["readonly"], "true"); assert_eq!(choices["encoding"], "utf16");
    assert_eq!(current_filter.unwrap().name, "Markdown");
    assert!(!path.exists());
}

#[test]
fn directory_and_save_files_keep_exact_names_and_confirm_replacement_once() {
    let dir = tempfile::tempdir().unwrap();
    let mut chooser = FileChooser::from_request(parse(json!({"directory":true,"current_folder":dir.path().to_str().unwrap()})));
    assert!(chooser.can_confirm());
    chooser.pick(&Context::default());
    let answer = chooser.pending.take().unwrap().recv_timeout(std::time::Duration::from_secs(5)).unwrap().unwrap();
    assert!(matches!(answer, Answer::Picked(paths) if paths == [dir.path()]));
    std::fs::write(dir.path().join("one.md"), "original").unwrap();
    let names = vec![b"one.md".to_vec(), b"\xff.md".to_vec()];
    let Answer::Batch(paths, replace) = check_batch(dir.path().into(), names.clone()).unwrap() else {panic!()};
    assert!(replace);
    assert_eq!(crate::choose::path_bytes(&paths[1]), crate::choose::path_bytes(&dir.path().join(crate::choose::path_from_bytes(&names[1]))));
    let mut chooser = FileChooser::from_request(parse(json!({"mode":"save_files","files":["one.md",{"bytes":"%FF.md"}],"current_folder":dir.path().to_str().unwrap()})));
    chooser.batch = Some(paths.clone()); chooser.replace = Some(paths[0].clone());
    let mut h = harness(chooser);
    h.get_by_label("Replace").click(); h.run();
    assert_eq!(h.state().1, Some(Outcome::Picked(paths)));
    assert_eq!(std::fs::read_to_string(dir.path().join("one.md")).unwrap(), "original");
}

#[test]
fn typed_save_paths_navigate_and_current_file_bytes_survive() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("Reports")).unwrap();
    let request = parse(json!({"mode":"save","current_file":format!("file://{}/%FF.md", dir.path().display())}));
    let chooser = FileChooser::from_request(request.clone());
    assert_eq!(chooser.directory, dir.path());
    let Answer::Save(path, false) = check_typed(dir.path().into(), chooser.name, &request).unwrap() else {panic!()};
    assert_eq!(path.file_name().unwrap().as_encoded_bytes(), b"\xff.md");
    assert!(matches!(check_typed(dir.path().into(), "Reports/letter".into(), &request), Ok(Answer::Navigate(_, tail)) if tail == "letter"));
    assert!(check_typed(dir.path().into(), "missing/letter".into(), &request).is_err());
}
