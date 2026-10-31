// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>

use super::*;

#[test]
fn canonical_body_round_trips_all_modes_without_losing_bytes_or_defaults() {
    for body in [
        json!({"mode":"open","multiple":true,"directory":true,"correlation":"compose-3",
            "current_filter":{"name":"Text","patterns":["*.md"],"mime_types":["text/*"]},
            "choices":[{"id":"check","label":"Check","options":[]}]}),
        json!({"mode":"save","current_file":"file:///tmp/%FF.md","accept_label":"_Write",
            "filters":[{"name":"Text","patterns":["*.md"],"mime_types":[]}],
            "current_filter":{"name":"Other","patterns":["*.txt"],"mime_types":[]}}),
        json!({"mode":"save_files","current_folder":"file:///tmp/%FE",
            "files":["one.md",{"bytes":"%FF.md"}], "on_behalf_of":"test app"}),
    ] {
        let request = Request::parse(&body.to_string(), &defaults()).unwrap();
        let reparsed = Request::parse(&request.body().unwrap().to_string(), &defaults()).unwrap();
        assert_eq!(request, reparsed);
    }
}

#[test]
fn inserted_current_filter_stays_within_the_round_trip_limit() {
    let filters = (0..64)
        .map(|i| json!({"name":format!("Filter {i}"),"patterns":["*.md"],"mime_types":[]}))
        .collect::<Vec<_>>();
    let extra = json!({"name":"Extra","patterns":["*.txt"],"mime_types":[]});
    assert!(
        parse(json!({"filters":filters,"current_filter":extra}))
            .unwrap_err()
            .message
            .contains("64-filter")
    );
    let request = parse(json!({"filters":&filters[..63],"current_filter":extra})).unwrap();
    assert_eq!(request.filters.len(), 64);
    assert_eq!(
        Request::parse(&request.body().unwrap().to_string(), &defaults()).unwrap(),
        request
    );
}

fn defaults() -> Defaults {
    Defaults {
        open_title: "Open".into(),
        save_title: "Save".into(),
        save_files_title: "Save files".into(),
        open_accept: "Open".into(),
        save_accept: "Save".into(),
    }
}

#[test]
fn edited_locations_are_refused_without_panicking_in_body() {
    let mut req = Request::parse("{}", &defaults()).unwrap();
    req.current_folder = Some(b"relative\0".to_vec());
    let result = std::panic::catch_unwind(|| req.body());
    assert!(
        result.is_ok(),
        "a public Request edit must not kill its worker"
    );
    assert!(result.unwrap().is_err());
}

#[derive(Default)]
struct TestPresenter {
    opened: Vec<OpenRequest>,
    ready: Vec<String>,
    closed: Vec<String>,
    unavailable: bool,
}

impl Presenter for TestPresenter {
    fn open(&mut self, request: &OpenRequest) -> std::result::Result<(), Unavailable> {
        if self.unavailable {
            return Err(Unavailable("no display".into()));
        }
        assert!(!request.ready);
        self.opened.push(request.clone());
        Ok(())
    }
    fn input_ready(&mut self, id: &str) {
        self.ready.push(id.to_owned());
    }
    fn close(&mut self, id: &str) {
        self.closed.push(id.to_owned());
    }
}

fn engine() -> Engine {
    Engine::new(0x9f2c41d07ab3e655, defaults(), b"/home/user".to_vec()).unwrap()
}
fn body(id: &str) -> String {
    json!({"request":id}).to_string()
}
fn parse(value: Value) -> Result<Request> {
    Request::parse(&value.to_string(), &defaults())
}
fn filter() -> Value {
    json!({"name":"Images","patterns":["*.png"],"mime_types":[]})
}
fn option() -> Value {
    json!({"id":"utf8","label":"UTF-8"})
}
fn choice() -> Value {
    json!({"id":"encoding","label":"Encoding","options":[option()]})
}
fn checkbox() -> Value {
    json!({"id":"compress","label":"Compress","options":[]})
}
fn accepted_path(path: &[u8]) -> End {
    End::Accept(Acceptance {
        paths: vec![path.to_vec()],
        folder: None,
        current_filter: None,
        choices: BTreeMap::new(),
    })
}
fn open(e: &mut Engine, p: &mut TestPresenter, request: Value, caller: &str, now: u64) -> String {
    let reply = e.choose(&request.to_string(), caller, now, p).unwrap();
    let id = reply["request"].as_str().unwrap().to_owned();
    let mut expected = json!({"request":id,"topic":"files.chosen"});
    if let Some(correlation) = request.get("correlation") {
        expected["correlation"] = correlation.clone();
    }
    assert_eq!(reply, expected);
    e.reply_sent(&id, p);
    id
}
fn argument(value: Value) {
    assert_eq!(
        parse(value.clone()).unwrap_err().error_code,
        "ARGUMENT",
        "{value}"
    );
}

#[test]
fn every_top_level_type_range_and_wrong_mode_refuses() {
    for value in [json!(null), json!([]), json!(1), json!("x")] {
        argument(value);
    }
    for bad in ["{", "true", "[]"] {
        assert!(Request::parse(bad, &defaults()).is_err());
    }
    for (field, bad_values) in [
        (
            "mode",
            vec![json!(null), json!(true), json!(""), json!("other")],
        ),
        ("title", vec![json!(1), json!(""), json!("é".repeat(129))]),
        (
            "accept_label",
            vec![json!(false), json!(""), json!("x".repeat(65))],
        ),
        ("multiple", vec![json!(1), json!("false"), json!(null)]),
        ("directory", vec![json!([]), json!("true"), json!(null)]),
        (
            "filters",
            vec![json!({}), json!(null), json!(vec![filter(); 65])],
        ),
        ("current_filter", vec![json!([]), json!(null)]),
        (
            "choices",
            vec![json!(false), json!(null), json!(vec![checkbox(); 17])],
        ),
        ("parent_window", vec![json!(1), json!("x".repeat(257))]),
        (
            "correlation",
            vec![json!(null), json!(""), json!("x".repeat(257))],
        ),
        (
            "on_behalf_of",
            vec![json!(3), json!(""), json!("x".repeat(129))],
        ),
        (
            "current_folder",
            vec![
                json!(null),
                json!(false),
                json!("relative"),
                json!("/a\0b"),
                json!("/".to_owned() + &"x".repeat(4096)),
            ],
        ),
    ] {
        for bad in bad_values {
            argument(json!({field:bad}));
        }
    }
    for mode in ["open", "save", "save_files"] {
        for field in [
            "multiple",
            "directory",
            "filters",
            "current_filter",
            "current_name",
            "current_file",
            "files",
        ] {
            let allowed = match field {
                "multiple" | "directory" => mode == "open",
                "filters" | "current_filter" => mode != "save_files",
                "files" => mode == "save_files",
                _ => mode == "save",
            };
            if !allowed {
                let mut request = json!({"mode":mode});
                if mode == "save_files" {
                    request["files"] = json!(["a"]);
                }
                request[field] = Value::Null;
                argument(request);
            }
        }
    }
    for bad in [
        json!(null),
        json!(false),
        json!("relative"),
        json!("/a\0b"),
        json!("/"),
        json!("/a/"),
        json!("/a/."),
        json!("/a/.."),
    ] {
        argument(json!({"mode":"save", "current_file":bad}));
    }
    for bad in [
        json!(null),
        json!(false),
        json!(""),
        json!("a/b"),
        json!("."),
        json!(".."),
        json!("\0"),
        json!("x".repeat(256)),
        json!({"bytes":1}),
    ] {
        argument(json!({"mode":"save", "current_name":bad}));
    }
    for value in [
        json!({"mode":"save_files"}),
        json!({"mode":"save_files","files":[]}),
        json!({"mode":"save_files","files":null}),
        json!({"mode":"save_files","files":"a"}),
        json!({"mode":"save_files","files":["a", {"bytes":"%61"}]}),
        json!({"mode":"save_files","files":vec!["a";257]}),
        json!({"mode":"save_files","files":["a/b"]}),
    ] {
        argument(value);
    }
}

#[test]
fn every_filter_refusal_and_large_valid_filter() {
    for bad in [
        json!(null),
        json!({}),
        json!({"name":"a","patterns":[]}),
        json!({"name":1,"patterns":["*"],"mime_types":[]}),
        json!({"name":"","patterns":["*"],"mime_types":[]}),
        json!({"name":"x".repeat(129),"patterns":["*"],"mime_types":[]}),
        json!({"name":"a","patterns":"*","mime_types":[]}),
        json!({"name":"a","patterns":[],"mime_types":null}),
        json!({"name":"a","patterns":[],"mime_types":[]}),
        json!({"name":"a","patterns":[1],"mime_types":[]}),
        json!({"name":"a","patterns":[""],"mime_types":[]}),
        json!({"name":"a","patterns":["[abc"],"mime_types":[]}),
        json!({"name":"a","patterns":[],"mime_types":["x".repeat(257)]}),
        json!({"name":"a","patterns":vec!["*";512],"mime_types":["image/png"]}),
    ] {
        argument(json!({"filters":[bad.clone()]}));
        argument(json!({"current_filter":bad}));
    }
    let large = json!({"name":"x".repeat(128),"patterns":vec!["x".repeat(256);300],"mime_types":vec!["image/*";212]});
    assert!(parse(json!({"filters":[large]})).is_ok());
}

#[test]
fn every_choice_and_option_refusal() {
    for bad in [
        json!(null),
        json!({}),
        json!({"id":"a","label":"b"}),
        json!({"id":1,"label":"b","options":[]}),
        json!({"id":"","label":"b","options":[]}),
        json!({"id":"x".repeat(65),"label":"b","options":[]}),
        json!({"id":"a","label":null,"options":[]}),
        json!({"id":"a","label":"","options":[]}),
        json!({"id":"a","label":"x".repeat(129),"options":[]}),
        json!({"id":"a","label":"b","options":null}),
        json!({"id":"a","label":"b","options":vec![option();33]}),
        json!({"id":"a","label":"b","options":[],"default":true}),
        json!({"id":"a","label":"b","options":[],"default":""}),
        json!({"id":"a","label":"b","options":[],"default":"yes"}),
        json!({"id":"a","label":"b","options":[option()],"default":"missing"}),
        json!({"id":"a","label":"b","options":[option(),option()]}),
    ] {
        argument(json!({"choices":[bad]}));
    }
    for bad in [
        json!(null),
        json!({}),
        json!({"id":"a"}),
        json!({"id":1,"label":"a"}),
        json!({"id":"","label":"a"}),
        json!({"id":"x".repeat(65),"label":"a"}),
        json!({"id":"a","label":false}),
        json!({"id":"a","label":""}),
        json!({"id":"a","label":"x".repeat(129)}),
    ] {
        argument(json!({"choices":[{"id":"a","label":"b","options":[bad]}]}));
    }
    argument(json!({"choices":[checkbox(),checkbox()]}));
}

#[test]
fn boundaries_unknown_fields_and_precedence() {
    assert!(parse(json!({"title":"é".repeat(128),"accept_label":"_O_pen", "parent_window":"", "unknown":null})).is_ok());
    assert_eq!(
        parse(json!({"accept_label":"_O_pen"}))
            .unwrap()
            .accept_label,
        "Open"
    );
    for label in ["_", "___", &"_".repeat(64)] {
        for (mode, files, expected) in [
            ("open", None, "Open"),
            ("save", None, "Save"),
            ("save_files", Some(json!(["a"])), "Save"),
        ] {
            let mut request = json!({"mode":mode,"accept_label":label});
            if let Some(files) = files {
                request["files"] = files;
            }
            assert_eq!(parse(request).unwrap().accept_label, expected);
        }
    }
    let mut raw = "{}".to_owned();
    raw.push_str(&" ".repeat(MAX_BODY - 2));
    assert!(Request::parse(&raw, &defaults()).is_ok());
    raw.push(' ');
    assert_eq!(
        Request::parse(&raw, &defaults()).unwrap_err().error_code,
        "ARGUMENT"
    );
    let req = parse(json!({"mode":"save","current_file":"file:///a/%FF", "current_folder":"/other","current_name":"other"})).unwrap();
    assert_eq!(req.current_folder.unwrap(), b"/a");
    assert_eq!(req.current_name.unwrap(), b"\xff");
    argument(json!({"mode":"save","current_file":"/good/file","current_folder":false}));
    let files: Vec<_> = (0..256).map(|i| format!("file-{i}")).collect();
    assert_eq!(
        parse(json!({"mode":"save_files","files":files}))
            .unwrap()
            .files
            .len(),
        256
    );
    assert!(name(&json!("é".repeat(127))).is_ok());
    assert!(name(&json!({"bytes":"%FF".repeat(255),"ignored":true})).is_ok());
    assert!(location(&json!("/".to_owned() + &"x".repeat(4095))).is_ok());
    assert!(location(&json!("file:///".to_owned() + &"%FF".repeat(4095))).is_ok());
}

#[test]
fn locations_names_and_exact_byte_uri_round_trip() {
    for invalid in [
        "http:///tmp",
        "file:/tmp",
        "file://example.org/tmp",
        "file://user@localhost/tmp",
        "file://localhost:80/tmp",
        "file://",
        "file://localhost",
        "file:///a%00",
        "file:///%",
        "file:///%0",
        "file:///%GG",
        "file:///a?query",
        "file:///a#fragment",
        "file:///tmp/é",
    ] {
        assert!(location(&json!(invalid)).is_err(), "{invalid}");
    }
    for invalid in ["", ".", "..", "%2E%2E", "a%2Fb", "%00", "%", "%0Z"] {
        assert!(name(&json!({"bytes":invalid})).is_err(), "{invalid}");
    }
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    let bytes = b"/tmp/\xff space#?%\xc3\xa9".to_vec();
    let path = std::path::PathBuf::from(std::ffi::OsString::from_vec(bytes.clone()));
    let uri = file_uri(path.as_os_str().as_bytes()).unwrap();
    assert_eq!(uri, "file:///tmp/%FF%20space%23%3F%25%C3%A9");
    assert_eq!(location(&json!(uri)).unwrap(), bytes);
    assert_eq!(
        location(&json!("file://localhost/tmp/%FF")).unwrap(),
        b"/tmp/\xff"
    );
    assert_eq!(
        location(&json!("FILE://LOCALHOST/tmp/%FF")).unwrap(),
        b"/tmp/\xff"
    );
    assert_eq!(location(&json!("/tmp/a#?%")).unwrap(), b"/tmp/a#?%");
    assert_eq!(
        location(&json!("file:///tmp/a+;=b")).unwrap(),
        b"/tmp/a+;=b"
    );
    assert_eq!(
        location(&json!(file_uri(b"/tmp/a+;=b").unwrap())).unwrap(),
        b"/tmp/a+;=b"
    );
    let n = name(&json!({"bytes":"%FF%20%25"})).unwrap();
    assert_eq!(n, b"\xff %");
}

#[test]
fn filters_select_standalone_match_or_insert_and_match_files() {
    let a = filter();
    let b = json!({"name":"Text","patterns":["*.txt"],"mime_types":["text/*"]});
    let standalone = parse(json!({"current_filter":a})).unwrap();
    assert!(!standalone.filter_menu);
    assert_eq!(standalone.current_filter, Some(parse_filter(&a).unwrap()));
    let default = parse(json!({"filters":[a.clone(),b.clone()]})).unwrap();
    assert_eq!(default.current_filter, Some(parse_filter(&a).unwrap()));
    let matching = parse(json!({"filters":[a.clone(),b.clone()],"current_filter":b})).unwrap();
    assert!(matching.filter_menu);
    assert_eq!(matching.filters.len(), 2);
    assert_eq!(matching.current_filter, Some(parse_filter(&b).unwrap()));
    let added = parse(json!({"filters":[a],"current_filter":b})).unwrap();
    assert_eq!(
        added.filters,
        vec![parse_filter(&b).unwrap(), parse_filter(&a).unwrap()]
    );
    let f = parse_filter(
        &json!({"name":"Test","patterns":["a[0-9]?.png"],"mime_types":["text/*","image/jpeg"]}),
    )
    .unwrap();
    for (name, directory, mime, matches) in [
        ("a2b.png", false, "image/png", true),
        ("A2b.png", false, "image/png", false),
        ("other", false, "text/plain", true),
        ("other", false, "image/jpeg", true),
        ("other", false, "image/png", false),
        ("folder", true, "inode/directory", true),
    ] {
        assert_eq!(f.matches(name, directory, mime), matches);
    }
    assert_eq!(f.patterns(), &["a[0-9]?.png"]);
    assert_eq!(f.compiled.len(), 1);
    assert_eq!(json!(f).as_object().unwrap().len(), 3);
}

#[test]
fn single_pattern_extension_is_save_only_and_preserves_existing_extensions() {
    let req = parse(json!({"mode":"save","current_filter":filter()})).unwrap();
    for (typed, expected) in [
        (b"image".as_slice(), b"image.png".as_slice()),
        (b"photo.jpg", b"photo.jpg"),
        (b"image.PNG", b"image.PNG"),
        (b"image.png", b"image.png"),
        (b"name.", b"name.png"),
        (b".bashrc", b".bashrc.png"),
        (b".config.json", b".config.json"),
        (b"name.ext.", b"name.ext."),
        (b"\xff", b"\xff.png"),
        (b"\xff.jpg", b"\xff.jpg"),
    ] {
        assert_eq!(req.save_name(typed), expected);
    }
    let compound = parse(json!({"mode":"save","current_filter":{
        "name":"Archive","patterns":["*.tar.gz"],"mime_types":[]
    }}))
    .unwrap();
    for (typed, expected) in [
        ("archive", "archive.tar.gz"),
        ("archive.tar", "archive.tar"),
        ("archive.tar.gz", "archive.tar.gz"),
        ("name.", "name.tar.gz"),
        (".bashrc", ".bashrc.tar.gz"),
    ] {
        assert_eq!(compound.save_name(typed.as_bytes()), expected.as_bytes());
    }
    for patterns in [
        vec!["*.p?g"],
        vec!["*.png", "*.jpg"],
        vec!["*."],
        vec!["*.[ab]"],
    ] {
        let req = parse(json!({"mode":"save","current_filter":{
            "name":"Other","patterns":patterns,"mime_types":[]
        }}))
        .unwrap();
        assert_eq!(req.save_name(b"image"), b"image");
    }
    for req in [
        json!({"current_filter":filter()}),
        json!({"mode":"save_files","files":["image"]}),
        json!({"mode":"save"}),
    ] {
        assert_eq!(parse(req).unwrap().save_name(b"image"), b"image");
    }
}

#[test]
fn filters_without_current_filter_publish_the_first_filter() {
    let mut e = engine();
    let mut p = TestPresenter::default();
    let id = open(
        &mut e,
        &mut p,
        json!({"filters":[filter(),{
            "name":"Text","patterns":["*.txt"],"mime_types":[]
        }]}),
        "caller",
        1,
    );
    let outcome = e
        .end(&id, accepted_path(b"/tmp/a.png"), 2, &mut p)
        .unwrap()
        .unwrap();
    assert_eq!(json!(outcome)["current_filter"], filter());
}

#[test]
fn choice_defaults_and_selected_values_are_complete() {
    let req = parse(json!({"choices":[checkbox(),choice()]})).unwrap();
    assert_eq!(
        req.choice_defaults(),
        BTreeMap::from([
            ("compress".into(), "false".into()),
            ("encoding".into(), "utf8".into())
        ])
    );
    assert_eq!(
        parse(json!({"choices":[{"id":"a","label":"a","options":[],"default":"true"}]}))
            .unwrap()
            .choice_defaults()["a"],
        "true"
    );
    let mut e = engine();
    let mut p = TestPresenter::default();
    let id = open(
        &mut e,
        &mut p,
        json!({"choices":[checkbox(),choice()],"current_filter":filter()}),
        "caller",
        1,
    );
    let outcome = e
        .end(&id, accepted_path(b"/tmp/a.png"), 2, &mut p)
        .unwrap()
        .unwrap();
    assert_eq!(outcome.choices, Some(req.choice_defaults()));
    assert_eq!(
        outcome.current_filter,
        Some(parse_filter(&filter()).unwrap())
    );
}

#[test]
fn desk_and_caller_limits_and_per_app_keying() {
    let mut e = engine();
    let mut p = TestPresenter::default();
    for app in ["app-a", "app-b", "app-c", "app-d"] {
        for _ in 0..2 {
            open(
                &mut e,
                &mut p,
                json!({"on_behalf_of":app}),
                "local:portald",
                1,
            );
        }
        let error = e
            .choose(
                &json!({"on_behalf_of":app}).to_string(),
                "local:portald",
                1,
                &mut p,
            )
            .unwrap_err();
        assert_eq!(error.error_code, "BUSY");
    }
    assert_eq!(e.list()["requests"].as_array().unwrap().len(), 8);
    assert_eq!(
        e.choose("{}", "fresh-caller", 1, &mut p)
            .unwrap_err()
            .error_code,
        "BUSY"
    );
    assert_eq!(p.opened.len(), 8);
    let id = p.opened[0].id.clone();
    e.cancel(&body(&id), 2, &mut p).unwrap();
    open(&mut e, &mut p, json!({}), "fresh-caller", 3);

    let mut e = engine();
    let mut p = TestPresenter::default();
    for caller in ["local:connection:1", "local:connection:2"] {
        for app in [None, Some("app")] {
            for _ in 0..2 {
                open(
                    &mut e,
                    &mut p,
                    app.map(|app| json!({"on_behalf_of":app}))
                        .unwrap_or(json!({})),
                    caller,
                    1,
                );
            }
        }
    }
    assert_eq!(p.opened.len(), 8);
}

#[test]
fn rate_refuses_the_eleventh_within_ten_seconds_independently_for_forwarded_apps() {
    let mut e = engine();
    let mut p = TestPresenter::default();
    for _ in 0..10 {
        let id = open(&mut e, &mut p, json!({"on_behalf_of":"a"}), "portald", 0);
        e.end(&id, End::Cancel, 0, &mut p).unwrap();
    }
    assert_eq!(p.opened.len(), 10);
    let error = e
        .choose(r#"{"on_behalf_of":"a"}"#, "portald", 9999, &mut p)
        .unwrap_err();
    assert_eq!(error.error_code, "BUSY");
    assert_eq!(error.message, "chooser rate limit reached");
    assert_eq!(p.opened.len(), 10);
    assert!(e.open.is_empty());
    open(&mut e, &mut p, json!({"on_behalf_of":"b"}), "portald", 9999);
    open(
        &mut e,
        &mut p,
        json!({"on_behalf_of":"a"}),
        "other-connection",
        9999,
    );
    open(
        &mut e,
        &mut p,
        json!({"on_behalf_of":"a"}),
        "portald",
        10000,
    );
}

#[test]
fn rate_counts_argument_refusals_before_field_and_filter_validation() {
    for invalid in [
        "{".to_owned(),
        "[]".to_owned(),
        json!({"mode":"bad"}).to_string(),
        json!({"on_behalf_of":false}).to_string(),
        " ".repeat(MAX_BODY + 1),
        json!({"on_behalf_of":"app","filters":[{
            "name":"Broken","patterns":["[abc"],"mime_types":[]
        }]})
        .to_string(),
    ] {
        let mut e = engine();
        let mut p = TestPresenter::default();
        for _ in 0..10 {
            assert_eq!(
                e.choose(&invalid, "caller", 0, &mut p)
                    .unwrap_err()
                    .error_code,
                "ARGUMENT"
            );
        }
        let error = e.choose(&invalid, "caller", 9999, &mut p).unwrap_err();
        assert_eq!(error.error_code, "BUSY");
        assert_eq!(error.message, "chooser rate limit reached");
        assert!(p.opened.is_empty());
        assert!(e.open.is_empty());
        assert!(e.done.is_empty());
        assert_eq!(e.recent.values().next().unwrap().len(), 10);
        // The failed 11th attempt is also counted; no unbounded queue.
        assert_eq!(
            e.choose(&invalid, "caller", 10000, &mut p)
                .unwrap_err()
                .error_code,
            "ARGUMENT"
        );
    }
}

#[test]
fn rate_caller_table_is_capped_and_old_entries_are_evicted() {
    let mut e = engine();
    let mut p = TestPresenter::default();
    for i in 0..MAX_RATE_CALLERS {
        let invalid = json!({"on_behalf_of":format!("app-{i}"),"mode":"bad"});
        assert_eq!(
            e.choose(&invalid.to_string(), "portald", 0, &mut p)
                .unwrap_err()
                .error_code,
            "ARGUMENT"
        );
    }
    assert_eq!(e.recent.len(), MAX_RATE_CALLERS);
    let error = e
        .choose(r#"{"on_behalf_of":"fresh"}"#, "portald", 9999, &mut p)
        .unwrap_err();
    assert_eq!(error.error_code, "BUSY");
    assert_eq!(error.message, "chooser rate caller table full");
    assert_eq!(e.recent.len(), MAX_RATE_CALLERS);
    assert!(p.opened.is_empty());
    open(
        &mut e,
        &mut p,
        json!({"on_behalf_of":"fresh"}),
        "portald",
        10000,
    );
    assert_eq!(e.recent.len(), 1);
}

#[test]
fn incarnations_sequences_refusals_and_restart() {
    let mut e = engine();
    let mut p = TestPresenter::default();
    let first = open(&mut e, &mut p, json!({}), "caller", 1);
    assert_eq!(first, "9f2c41d07ab3e655-1");
    e.end(&first, End::Cancel, 2, &mut p).unwrap();
    let second = open(&mut e, &mut p, json!({}), "caller", 3);
    assert_eq!(second, "9f2c41d07ab3e655-2");
    let mut restarted = Engine::new(0x1234, defaults(), b"/home/user".to_vec()).unwrap();
    let third = open(&mut restarted, &mut p, json!({}), "caller", 4);
    assert_eq!(third, "0000000000001234-1");
    assert_eq!(
        restarted.result(&body(&first), 4).unwrap_err().reason,
        Some("unknown")
    );
    assert!(
        restarted
            .cancel(&body(&first), 4, &mut p)
            .unwrap()
            .is_none()
    );
    e.sequence = u64::MAX;
    assert_eq!(
        e.choose("{}", "new", 10, &mut p).unwrap_err().error_code,
        "UNAVAILABLE"
    );
}

#[test]
fn first_end_wins_outcome_shapes_results_and_publication_once() {
    for (end, response, reason) in [
        (accepted_path(b"/tmp/a"), 0, "chosen"),
        (End::Cancel, 1, "cancelled"),
        (End::Caller, 1, "caller"),
        (End::Failed("broken".into()), 2, "failed"),
    ] {
        let mut e = engine();
        let mut p = TestPresenter::default();
        let id = open(&mut e, &mut p, json!({"correlation":"corr"}), "caller", 1);
        assert_eq!(e.result(&body(&id), 2).unwrap(), json!({"state":"open"}));
        let outcome = e.end(&id, end, 3, &mut p).unwrap().unwrap();
        assert_eq!((outcome.response, outcome.reason), (response, reason));
        let value = json!(outcome);
        assert_eq!(value["correlation"], "corr");
        for key in ["uris", "choices"] {
            assert_eq!(value.get(key).is_some(), response == 0);
        }
        assert!(value.get("current_filter").is_none());
        assert_eq!(value.get("message").is_some(), response == 2);
        for later in [
            End::Cancel,
            End::Caller,
            End::Failed("late".into()),
            accepted_path(b"/tmp/later"),
        ] {
            assert!(e.end(&id, later, 4, &mut p).unwrap().is_none());
        }
        for _ in 0..2 {
            assert_eq!(
                e.result(&body(&id), 5).unwrap(),
                json!({"state":"done","outcome":outcome})
            );
        }
        assert_eq!(p.closed.len(), 1);
    }
}

#[test]
fn retention_eviction_unknown_expired_and_cancel_is_open_and_idempotent() {
    let mut e = engine();
    let mut p = TestPresenter::default();
    let mut ids = Vec::new();
    for i in 0..65 {
        let id = open(&mut e, &mut p, json!({}), &format!("caller-{i}"), i);
        e.cancel(&body(&id), i, &mut p).unwrap();
        ids.push(id);
    }
    assert_eq!(e.done.len(), 64);
    assert_eq!(
        e.result(&body(&ids[0]), 66).unwrap_err().reason,
        Some("expired")
    );
    assert!(e.result(&body(&ids[1]), 600000).is_ok());
    assert_eq!(
        e.result(&body(&ids[1]), 600001).unwrap_err().reason,
        Some("expired")
    );
    let future = "9f2c41d07ab3e655-999";
    assert_eq!(
        e.result(&body(future), 600001).unwrap_err().reason,
        Some("unknown")
    );
    for id in [&ids[0], &ids[1], &ids[64], future, "0000000000000000-1"] {
        assert!(e.cancel(&body(id), 600001, &mut p).unwrap().is_none());
    }
    let id = open(&mut e, &mut p, json!({}), "someone-else", 600002);
    // cancel accepts no caller identity: any caller may cancel this one.
    let outcome = e.cancel(&body(&id), 600003, &mut p).unwrap().unwrap();
    assert_eq!((outcome.response, outcome.reason), (1, "caller"));
    assert!(e.cancel(&body(&id), 600004, &mut p).unwrap().is_none());
    for bad in [
        json!({}),
        json!({"request":null}),
        json!({"request":1}),
        json!({"request":""}),
        json!({"request":"bogus"}),
        json!({"request":"9F2C41D07AB3E655-1"}),
        json!({"request":"9f2c41d07ab3e655-0"}),
        json!({"request":"9f2c41d07ab3e655-01"}),
        json!({"request":"9f2c41d07ab3e655-18446744073709551616"}),
    ] {
        assert_eq!(
            e.cancel(&bad.to_string(), 600004, &mut p)
                .unwrap_err()
                .error_code,
            "ARGUMENT"
        );
        assert_eq!(
            e.result(&bad.to_string(), 600004).unwrap_err().error_code,
            "ARGUMENT"
        );
    }
}

#[test]
fn save_files_containment_order_and_non_utf8_names() {
    let mut e = engine();
    let mut p = TestPresenter::default();
    let id = open(
        &mut e,
        &mut p,
        json!({"mode":"save_files","files":[{"bytes":"%FF"},"normal"]}),
        "caller",
        1,
    );
    for paths in [
        vec![b"/chosen/../escape".to_vec(), b"/chosen/normal".to_vec()],
        vec![b"/elsewhere/\xff".to_vec(), b"/chosen/normal".to_vec()],
        vec![b"/chosen/normal".to_vec(), b"/chosen/\xff".to_vec()],
        vec![b"/chosen/\xff".to_vec()],
    ] {
        let end = End::Accept(Acceptance {
            paths,
            folder: Some(b"/chosen".to_vec()),
            current_filter: None,
            choices: BTreeMap::new(),
        });
        assert_eq!(
            e.end(&id, end, 2, &mut p).unwrap_err().error_code,
            "ARGUMENT"
        );
    }
    let end = End::Accept(Acceptance {
        paths: vec![b"/chosen/\xff".to_vec(), b"/chosen/normal".to_vec()],
        folder: Some(b"/chosen".to_vec()),
        current_filter: None,
        choices: BTreeMap::new(),
    });
    assert_eq!(
        e.end(&id, end, 3, &mut p).unwrap().unwrap().uris.unwrap(),
        ["file:///chosen/%FF", "file:///chosen/normal"]
    );
    let id = open(
        &mut e,
        &mut p,
        json!({"mode":"save_files","files":["a"]}),
        "caller",
        4,
    );
    let end = End::Accept(Acceptance {
        paths: vec![b"/a".to_vec()],
        folder: Some(b"/".to_vec()),
        current_filter: None,
        choices: BTreeMap::new(),
    });
    assert_eq!(
        e.end(&id, end, 5, &mut p).unwrap().unwrap().uris.unwrap(),
        ["file:///a"]
    );
}

#[test]
fn acceptance_cardinality_paths_filters_and_choices_are_rechecked() {
    for (req, paths) in [
        (json!({}), vec![]),
        (json!({}), vec![b"/a".to_vec(), b"/b".to_vec()]),
        (json!({"mode":"save"}), vec![b"/a".to_vec(), b"/b".to_vec()]),
        (json!({}), vec![b"relative".to_vec()]),
        (json!({}), vec![b"/a\0b".to_vec()]),
        (json!({}), vec![b"/tmp/..".to_vec()]),
        (json!({}), vec![b"/tmp/".to_vec()]),
        (json!({}), vec![b"/tmp/.".to_vec()]),
        (json!({}), vec![b"/".to_vec()]),
        (json!({"mode":"save"}), vec![b"/tmp/..".to_vec()]),
        (json!({"mode":"save"}), vec![b"/tmp/".to_vec()]),
        (json!({"directory":true}), vec![b"/tmp/..".to_vec()]),
        (json!({"directory":true}), vec![b"/tmp/.".to_vec()]),
        (json!({"directory":true}), vec![b"/tmp/../folder".to_vec()]),
        (json!({"directory":true}), vec![b"/tmp/./folder".to_vec()]),
        (
            json!({"multiple":true}),
            vec![b"/tmp/a".to_vec(), b"/tmp/".to_vec()],
        ),
        (
            json!({}),
            vec![format!("/tmp/{}", "a".repeat(256)).into_bytes()],
        ),
    ] {
        let mut e = engine();
        let mut p = TestPresenter::default();
        let id = open(&mut e, &mut p, req, "caller", 1);
        assert!(
            e.end(
                &id,
                End::Accept(Acceptance {
                    paths,
                    folder: None,
                    current_filter: None,
                    choices: BTreeMap::new()
                }),
                2,
                &mut p
            )
            .is_err()
        );
        assert_eq!(e.list()["requests"].as_array().unwrap().len(), 1);
    }
    let mut e = engine();
    let mut p = TestPresenter::default();
    let id = open(&mut e, &mut p, json!({"multiple":true}), "caller", 1);
    assert!(
        e.end(
            &id,
            End::Accept(Acceptance {
                paths: vec![b"/a".to_vec(), b"/b".to_vec()],
                folder: None,
                current_filter: None,
                choices: BTreeMap::new()
            }),
            2,
            &mut p
        )
        .unwrap()
        .is_some()
    );
    let id = open(
        &mut e,
        &mut p,
        json!({"choices":[choice(),checkbox()]}),
        "caller",
        3,
    );
    for (id_value, value) in [
        ("encoding", "bad"),
        ("compress", "yes"),
        ("unknown", "true"),
    ] {
        assert!(
            e.end(
                &id,
                End::Accept(Acceptance {
                    paths: vec![b"/a".to_vec()],
                    folder: None,
                    current_filter: None,
                    choices: BTreeMap::from([(id_value.into(), value.into())])
                }),
                4,
                &mut p
            )
            .is_err()
        );
    }
}

#[test]
fn directories_and_multiple_directories_accept_and_remember_the_folder() {
    for (request, paths, remembered) in [
        (
            json!({"directory":true}),
            vec![b"/".to_vec()],
            b"/".to_vec(),
        ),
        (
            json!({"directory":true}),
            vec![b"/tmp/folder/".to_vec()],
            b"/tmp/folder/".to_vec(),
        ),
        (
            json!({"directory":true,"multiple":true}),
            vec![b"/tmp/one".to_vec(), b"/tmp/two".to_vec()],
            b"/tmp/one".to_vec(),
        ),
    ] {
        let mut e = engine();
        let mut p = TestPresenter::default();
        let id = open(&mut e, &mut p, request, "caller", 1);
        let expected: Vec<_> = paths.iter().map(|path| file_uri(path).unwrap()).collect();
        let outcome = e
            .end(
                &id,
                End::Accept(Acceptance {
                    paths,
                    folder: None,
                    current_filter: None,
                    choices: BTreeMap::new(),
                }),
                2,
                &mut p,
            )
            .unwrap()
            .unwrap();
        assert_eq!(outcome.response, 0);
        assert_eq!(outcome.uris.unwrap(), expected);
        open(&mut e, &mut p, json!({}), "caller", 3);
        assert_eq!(
            p.opened.last().unwrap().request.current_folder.as_ref(),
            Some(&remembered)
        );
    }
}

#[test]
fn list_order_defaults_remembered_folder_and_refusals_keep_nothing() {
    let mut e = engine();
    let mut p = TestPresenter::default();
    let a = open(
        &mut e,
        &mut p,
        json!({"correlation":"a","on_behalf_of":"app"}),
        "portald",
        100,
    );
    let b = open(
        &mut e,
        &mut p,
        json!({"mode":"save","title":"Second"}),
        "agent",
        101,
    );
    let rows = e.list()["requests"].as_array().unwrap().clone();
    assert_eq!(
        rows[0],
        json!({"request":a,"mode":"open","title":"Open","caller":"portald","on_behalf_of":"app","correlation":"a","opened_ms":100})
    );
    assert_eq!(rows[1]["request"], b);
    assert!(rows[1].get("on_behalf_of").is_none());
    assert!(rows[1].get("correlation").is_none());
    assert_eq!(
        p.opened[0].request.current_folder.as_deref(),
        Some(b"/home/user".as_slice())
    );
    e.end(&a, accepted_path(b"/picked/\xff"), 102, &mut p)
        .unwrap();
    open(
        &mut e,
        &mut p,
        json!({"on_behalf_of":"app"}),
        "new-portald",
        103,
    );
    assert_eq!(
        p.opened.last().unwrap().request.current_folder.as_deref(),
        Some(b"/picked".as_slice())
    );
    let before = e.list();
    p.unavailable = true;
    for expected in ["UNAVAILABLE"; 10].into_iter().chain(["BUSY"; 10]) {
        assert_eq!(
            e.choose("{}", "no-window", 104, &mut p)
                .unwrap_err()
                .error_code,
            expected
        );
    }
    assert_eq!(e.list(), before);
    let recent = e.recent.get(&Caller("no-window".into(), None)).unwrap();
    assert_eq!(recent.len(), 10);
}

#[test]
fn remembered_folders_cap_at_128_and_evict_least_recent_reads_or_acceptances() {
    let mut e = engine();
    let mut p = TestPresenter::default();
    // Equal timestamps make the order depend on use, not clock resolution.
    for i in 0..128 {
        let id = open(
            &mut e,
            &mut p,
            json!({"on_behalf_of":format!("app-{i}")}),
            "portald",
            0,
        );
        e.end(
            &id,
            accepted_path(format!("/folder-{i}/file").as_bytes()),
            0,
            &mut p,
        )
        .unwrap();
        assert_eq!(e.remembered.len(), i + 1);
    }
    // Reusing a folder refreshes it even if the chooser is then cancelled.
    let id = open(
        &mut e,
        &mut p,
        json!({"on_behalf_of":"app-0"}),
        "new-portald",
        0,
    );
    assert_eq!(
        p.opened.last().unwrap().request.current_folder.as_deref(),
        Some(b"/folder-0".as_slice())
    );
    e.end(&id, End::Cancel, 0, &mut p).unwrap();
    // An acceptance updates an existing entry without increasing the count.
    let id = open(
        &mut e,
        &mut p,
        json!({"on_behalf_of":"app-1","current_folder":"/override"}),
        "portald",
        0,
    );
    e.end(&id, accepted_path(b"/updated/file"), 0, &mut p)
        .unwrap();
    assert_eq!(e.remembered.len(), 128);
    // A failed presentation does not refresh the least recently used entry.
    p.unavailable = true;
    assert_eq!(
        e.choose(r#"{"on_behalf_of":"app-2"}"#, "portald", 0, &mut p)
            .unwrap_err()
            .error_code,
        "UNAVAILABLE"
    );
    p.unavailable = false;
    for i in 128..130 {
        let id = open(
            &mut e,
            &mut p,
            json!({"on_behalf_of":format!("app-{i}")}),
            "portald",
            0,
        );
        e.end(&id, accepted_path(b"/new/file"), 0, &mut p).unwrap();
        assert_eq!(e.remembered.len(), 128);
        assert!(
            !e.remembered
                .iter()
                .any(|(key, _)| key == &(true, format!("app-{}", i - 126)))
        );
    }
    let expected: Vec<_> = (4..128)
        .chain([0, 1, 128, 129])
        .map(|i| (true, format!("app-{i}")))
        .collect();
    assert_eq!(
        e.remembered
            .iter()
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>(),
        expected
    );
    for (app, folder) in [
        ("app-0", "/folder-0"),
        ("app-1", "/updated"),
        ("app-2", "/home/user"),
        ("app-3", "/home/user"),
    ] {
        let id = open(&mut e, &mut p, json!({"on_behalf_of":app}), "portald", 0);
        assert_eq!(
            p.opened.last().unwrap().request.current_folder.as_deref(),
            Some(folder.as_bytes())
        );
        e.end(&id, End::Cancel, 0, &mut p).unwrap();
    }
    assert_eq!(e.remembered.len(), 128);
}

#[test]
fn rate_counts_window_limit_refusals_before_a_slot_is_freed() {
    let mut e = engine();
    let mut p = TestPresenter::default();
    let first = open(&mut e, &mut p, json!({}), "busy", 0);
    open(&mut e, &mut p, json!({}), "busy", 0);
    for _ in 0..8 {
        assert_eq!(
            e.choose("{}", "busy", 0, &mut p).unwrap_err().message,
            "chooser limit reached"
        );
    }
    e.cancel(&body(&first), 1, &mut p).unwrap();
    assert_eq!(
        e.choose("{}", "busy", 1, &mut p).unwrap_err().message,
        "chooser rate limit reached"
    );
    open(&mut e, &mut p, json!({}), "busy", 10_000);
}

#[test]
fn reply_precedes_input_and_first_outcome_failed_reply_closes() {
    let mut e = engine();
    let mut p = TestPresenter::default();
    let reply = e
        .choose(r#"{"correlation":"corr"}"#, "caller", 1, &mut p)
        .unwrap();
    assert_eq!(
        reply,
        json!({"request":"9f2c41d07ab3e655-1","topic":"files.chosen","correlation":"corr"})
    );
    let id = reply["request"].as_str().unwrap();
    assert!(p.ready.is_empty());
    assert_eq!(
        e.end(id, End::Cancel, 2, &mut p).unwrap_err().error_code,
        "BUSY"
    );
    assert!(e.done.is_empty());
    // The host has now sent the reply. Only now may the presenter take input.
    e.reply_sent(id, &mut p);
    e.reply_sent(id, &mut p);
    assert_eq!(p.ready, [id]);
    assert!(e.end(id, End::Cancel, 3, &mut p).unwrap().is_some());
    assert!(e.end(id, End::Cancel, 4, &mut p).unwrap().is_none());
    let reply = e.choose("{}", "caller", 5, &mut p).unwrap();
    assert_eq!(
        reply,
        json!({"request":"9f2c41d07ab3e655-2","topic":"files.chosen"})
    );
    let id = reply["request"].as_str().unwrap();
    e.reply_failed(id, &mut p);
    assert!(e.open.is_empty());
    assert_eq!(p.closed.last().unwrap(), id);
    assert_eq!(e.result(&body(id), 6).unwrap_err().reason, Some("unknown"));
}

#[test]
fn inclusive_filter_choice_and_option_limits_and_explicit_answers() {
    let filters = vec![filter(); 64];
    let options: Vec<_> = (0..32)
        .map(|i| json!({"id":format!("option-{i}"),"label":"x".repeat(128)}))
        .collect();
    let choices: Vec<_> = (0..16).map(|i| json!({"id":format!("choice-{i}"),"label":"x".repeat(128),"options":options,"default":"option-31"})).collect();
    let request = parse(json!({"filters":filters,"choices":choices})).unwrap();
    assert_eq!(request.filters.len(), 64);
    assert_eq!(request.choices.len(), 16);
    assert_eq!(request.choices[0].options.len(), 32);
    assert!(
        request
            .choice_defaults()
            .values()
            .all(|value| value == "option-31")
    );

    let text_filter = json!({"name":"Text","patterns":["*.txt"],"mime_types":[]});
    let mut e = engine();
    let mut p = TestPresenter::default();
    let id = open(
        &mut e,
        &mut p,
        json!({"filters":[filter(),text_filter],"choices":[checkbox(),choice()]}),
        "caller",
        1,
    );
    let mut answer = Acceptance {
        paths: vec![b"/chosen/text.txt".to_vec()],
        folder: None,
        current_filter: Some(
            parse_filter(&json!({"name":"Unlisted","patterns":["*"],"mime_types":[]})).unwrap(),
        ),
        choices: BTreeMap::from([
            ("compress".into(), "true".into()),
            ("encoding".into(), "utf8".into()),
        ]),
    };
    assert_eq!(
        e.end(&id, End::Accept(answer.clone()), 2, &mut p)
            .unwrap_err()
            .error_code,
        "ARGUMENT"
    );
    answer.current_filter = Some(parse_filter(&text_filter).unwrap());
    let outcome = e.end(&id, End::Accept(answer), 3, &mut p).unwrap().unwrap();
    assert_eq!(
        outcome.current_filter,
        Some(parse_filter(&text_filter).unwrap())
    );
    assert_eq!(outcome.choices.unwrap()["compress"], "true");
}

#[test]
fn eviction_uses_decision_order_and_open_requests_never_expire() {
    let mut e = engine();
    let mut p = TestPresenter::default();
    let long_open = open(&mut e, &mut p, json!({}), "patient", 0);
    let mut oldest_done = String::new();
    for i in 1..=64 {
        let id = open(&mut e, &mut p, json!({}), &format!("caller-{i}"), i);
        if i == 1 {
            oldest_done = id.clone();
        }
        e.end(&id, End::Cancel, i, &mut p).unwrap();
    }
    assert_eq!(
        e.result(&body(&long_open), 65).unwrap(),
        json!({"state":"open"})
    );
    e.end(&long_open, End::Cancel, 66, &mut p).unwrap();
    assert_eq!(
        e.result(&body(&oldest_done), 67).unwrap_err().reason,
        Some("expired")
    );
    assert_eq!(e.result(&body(&long_open), 67).unwrap()["state"], "done");
    let waiting = open(&mut e, &mut p, json!({}), "patient", 1000);
    assert_eq!(
        e.result(&body(&waiting), 1_000_000).unwrap(),
        json!({"state":"open"})
    );
}

/// A fake filesystem for `files.choose.accept`: `/home/user` and `/home/user/docs`
/// are folders, `/home/user/a.txt` and `/home/user/b.txt` are files.
fn disk(path: &[u8]) -> Entry {
    match path {
        b"/" | b"/home" | b"/home/user" | b"/home/user/docs" => Entry::Directory,
        b"/home/user/a.txt" | b"/home/user/b.txt" => Entry::File,
        _ => Entry::Missing,
    }
}

fn accept(e: &mut Engine, p: &mut TestPresenter, body: Value) -> Result<Outcome> {
    e.accept(&body.to_string(), 5, p, disk)
}

#[test]
fn accept_completes_open_like_the_accept_button() {
    let (mut e, mut p) = (engine(), TestPresenter::default());
    let id = open(
        &mut e,
        &mut p,
        json!({"mode":"open","multiple":true}),
        "c",
        1,
    );
    let outcome = accept(
        &mut e,
        &mut p,
        json!({"request":id,"paths":["/home/user/a.txt","file:///home/user/b.txt"]}),
    )
    .unwrap();
    assert_eq!((outcome.response, outcome.reason), (0, "chosen"));
    assert_eq!(
        outcome.uris.as_deref().unwrap(),
        ["file:///home/user/a.txt", "file:///home/user/b.txt"]
    );
    assert_eq!(p.closed, [id.clone()]);
    // The requester reads exactly the same outcome.
    assert_eq!(
        e.result(&body(&id), 6).unwrap(),
        json!({"state":"done","outcome":outcome})
    );
    // A single `path` works too, and a folder chooser takes folders.
    let id = open(&mut e, &mut p, json!({"directory":true}), "c", 2);
    let outcome = accept(
        &mut e,
        &mut p,
        json!({"request":id,"path":"/home/user/docs"}),
    )
    .unwrap();
    assert_eq!(outcome.uris.unwrap(), ["file:///home/user/docs"]);
}

#[test]
fn accept_saves_with_the_window_rules() {
    let (mut e, mut p) = (engine(), TestPresenter::default());
    let filter = json!({"name":"Text","patterns":["*.txt"],"mime_types":[]});
    let id = open(
        &mut e,
        &mut p,
        json!({"mode":"save","filters":[filter]}),
        "c",
        1,
    );
    // The single-pattern extension rule applies, as it does to a typed name.
    let outcome = accept(
        &mut e,
        &mut p,
        json!({"request":id,"path":"/home/user/new"}),
    )
    .unwrap();
    assert_eq!(outcome.uris.unwrap(), ["file:///home/user/new.txt"]);

    let id = open(&mut e, &mut p, json!({"mode":"save"}), "c", 2);
    let exists = accept(
        &mut e,
        &mut p,
        json!({"request":id,"path":"/home/user/a.txt"}),
    )
    .unwrap_err();
    assert_eq!(exists.error_code, "EXISTS");
    let replaced = accept(
        &mut e,
        &mut p,
        json!({"request":id,"path":"/home/user/a.txt","replace":true}),
    )
    .unwrap();
    assert_eq!(replaced.uris.unwrap(), ["file:///home/user/a.txt"]);

    let id = open(
        &mut e,
        &mut p,
        json!({"mode":"save_files","files":["x.md","y.md"]}),
        "c",
        3,
    );
    let outcome = accept(
        &mut e,
        &mut p,
        json!({"request":id,"folder":"/home/user/docs",
            "paths":["/home/user/docs/x.md","/home/user/docs/y.md"]}),
    )
    .unwrap();
    assert_eq!(outcome.reason, "chosen");
}

#[test]
fn accept_refuses_unknown_and_finished_requests() {
    let (mut e, mut p) = (engine(), TestPresenter::default());
    let path = json!("/home/user/a.txt");
    for (id, reason) in [
        ("9f2c41d07ab3e655-9", "unknown"),
        ("0000000000000000-1", "unknown"),
    ] {
        let refusal = accept(&mut e, &mut p, json!({"request":id,"path":path})).unwrap_err();
        assert_eq!(
            (refusal.error_code, refusal.reason),
            ("UNKNOWN_REQUEST", Some(reason))
        );
    }
    assert_eq!(
        accept(&mut e, &mut p, json!({"request":"bad","path":path}))
            .unwrap_err()
            .error_code,
        "ARGUMENT"
    );
    let id = open(&mut e, &mut p, json!({}), "c", 1);
    accept(&mut e, &mut p, json!({"request":id,"path":path})).unwrap();
    let again = accept(&mut e, &mut p, json!({"request":id,"path":path})).unwrap_err();
    assert_eq!(
        (again.error_code, again.reason),
        ("UNKNOWN_REQUEST", Some("done"))
    );
}

#[test]
fn accept_refuses_invalid_paths_and_leaves_the_chooser_open() {
    let (mut e, mut p) = (engine(), TestPresenter::default());
    let id = open(&mut e, &mut p, json!({}), "c", 1);
    for (body, code) in [
        (
            json!({"request":id,"path":"/home/user/missing.txt"}),
            "INVALID_PATH",
        ),
        (
            json!({"request":id,"path":"/home/user/docs"}),
            "INVALID_PATH",
        ),
        (json!({"request":id,"path":"relative.txt"}), "ARGUMENT"),
        (json!({"request":id}), "ARGUMENT"),
        (
            json!({"request":id,"path":"/home/user/a.txt","paths":["/home/user/a.txt"]}),
            "ARGUMENT",
        ),
        // Not multiple: one path only.
        (
            json!({"request":id,"paths":["/home/user/a.txt","/home/user/b.txt"]}),
            "ARGUMENT",
        ),
    ] {
        assert_eq!(
            accept(&mut e, &mut p, body.clone()).unwrap_err().error_code,
            code,
            "{body}"
        );
    }
    let folder = open(&mut e, &mut p, json!({"directory":true}), "d", 2);
    assert_eq!(
        accept(
            &mut e,
            &mut p,
            json!({"request":folder,"path":"/home/user/a.txt"})
        )
        .unwrap_err()
        .error_code,
        "INVALID_PATH"
    );
    let save = open(&mut e, &mut p, json!({"mode":"save"}), "s", 3);
    for path in ["/nowhere/new.txt", "/home/user/docs"] {
        assert_eq!(
            accept(&mut e, &mut p, json!({"request":save,"path":path}))
                .unwrap_err()
                .error_code,
            "INVALID_PATH",
            "{path}"
        );
    }
    assert!(p.closed.is_empty());
    for id in [&id, &folder, &save] {
        assert_eq!(e.result(&body(id), 6).unwrap(), json!({"state":"open"}));
    }
}
