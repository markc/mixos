// SPDX-License-Identifier: MIT OR Apache-2.0
use action::{Action, Class, Error, Origin, ParamSpec, Place, Registry, Scope, Strings};
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Default)]
struct Counter {
    n: u32,
    locked: bool,
}
const FTL: &str = "menu-edit = Edit\ncmd-bump = Bump\ncmd-reset = Reset\n";

fn registry() -> Registry<Counter> {
    let mut r = Registry::new();
    let mut bump = action::nullary!("count.bump", "cmd-bump", |s: &mut Counter| s.n += 1);
    bump.menu = Some("menu-edit");
    bump.shortcut = Some("Ctrl+B");
    bump.enabled = |s| if s.locked { Err("locked") } else { Ok(()) };
    r.add(bump).add(action::nullary!(
        "count.reset",
        "cmd-reset",
        |s: &mut Counter| s.n = 0
    ));
    r
}

#[test]
fn accepted_runs_are_recorded_in_order_and_refusals_are_not() {
    let r = registry();
    let mut s = Counter::default();
    r.execute("count.bump", &mut s, Value::Null, Origin::Menu)
        .unwrap();
    assert!(r.execute("nope", &mut s, Value::Null, Origin::Bus).is_err());
    s.locked = true;
    assert!(
        r.execute("count.bump", &mut s, Value::Null, Origin::Bus)
            .is_err()
    );
    r.execute("count.reset", &mut s, json!({}), Origin::Bus)
        .unwrap();
    r.note("view.mode", Origin::Ui);
    let (runs, lost) = r.take_executed();
    let got: Vec<_> = runs
        .iter()
        .map(|e| (e.id.as_str(), e.origin, e.seq))
        .collect();
    assert_eq!(
        got,
        [
            ("count.bump", Origin::Menu, 1),
            ("count.reset", Origin::Bus, 2),
            ("view.mode", Origin::Ui, 3)
        ]
    );
    assert_eq!(lost, 0);
    assert_eq!(r.take_executed(), (Vec::new(), 0));
}

#[test]
fn execute_dispatches_by_id_and_honours_enablement() {
    let r = registry();
    let mut s = Counter::default();
    assert_eq!(
        r.execute("count.bump", &mut s, Value::Null, Origin::Bus),
        Ok(Value::Null)
    );
    assert_eq!(s.n, 1);
    s.locked = true;
    assert_eq!(
        r.execute("count.bump", &mut s, Value::Null, Origin::Bus),
        Err(Error::Disabled {
            id: "count.bump",
            reason: "locked"
        })
    );
    assert_eq!(
        r.execute("nope", &mut s, Value::Null, Origin::Bus),
        Err(Error::Unknown("nope".into()))
    );
    r.execute("count.reset", &mut s, Value::Null, Origin::Bus)
        .unwrap();
    assert_eq!(s.n, 0);
}

#[test]
fn nullary_commands_refuse_parameters_without_running_or_journaling() {
    let r = registry();
    let mut s = Counter::default();
    for params in [
        json!({"n": 2}),
        json!([]),
        json!(false),
        json!(0),
        json!(""),
    ] {
        let error = r
            .execute("count.bump", &mut s, params, Origin::Bus)
            .unwrap_err();
        assert!(matches!(
            error,
            Error::BadParams {
                id: "count.bump",
                ..
            }
        ));
        assert!(!error.to_string().is_empty());
    }
    assert_eq!(s.n, 0);
    assert!(r.take_executed().0.is_empty());
}

#[test]
fn describe_is_localised_and_reports_enablement_reason_class_and_schema() {
    let d = registry().describe(
        &Counter {
            locked: true,
            ..Default::default()
        },
        &Strings::new(FTL),
    );
    assert_eq!(d[0].label, "Bump");
    assert_eq!(d[0].menu.as_deref(), Some("Edit"));
    assert_eq!(d[0].shortcut.as_deref(), Some("Ctrl+B"));
    assert!(!d[0].enabled);
    assert_eq!(d[0].reason, Some("locked"));
    assert_eq!(d[0].class, Class::Intent);
    assert_eq!(d[0].schema, None);
    assert!(d[1].enabled);
    assert_eq!(d[1].reason, None);
    assert_eq!(
        serde_json::to_value(&d[0]).unwrap(),
        json!({
            "id": "count.bump", "label": "Bump", "menu": "Edit", "shortcut": "Ctrl+B",
            "enabled": false, "checked": null, "reason": "locked", "class": "intent", "schema": null
        })
    );
}

#[derive(Deserialize, Serialize, JsonSchema, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
struct SetCount {
    n: u32,
}

fn typed() -> Action<Counter> {
    Action {
        id: "count.set",
        label: "Set",
        menu: None,
        submenu: None,
        group: 0,
        shortcut: None,
        icon: Some("hash"),
        enabled: action::always,
        params: Some(ParamSpec {
            type_name: "SetCount",
            schema: || schema_for!(SetCount),
        }),
        class: Class::Intent,
        run: |s, params| {
            let params: SetCount =
                serde_json::from_value(params.clone()).map_err(|e| Error::BadParams {
                    id: "count.set",
                    msg: e.to_string(),
                })?;
            s.n = params.n;
            Ok(json!(params))
        },
    }
}

#[test]
fn typed_parameters_round_trip_and_report_their_schema() {
    let mut r = Registry::new();
    r.add(typed());
    let mut s = Counter::default();
    assert_eq!(
        r.execute("count.set", &mut s, json!({"n": 42}), Origin::Bus)
            .unwrap(),
        json!({"n": 42})
    );
    assert_eq!(s.n, 42);
    for bad in [
        json!({"n": 1, "extra": true}),
        json!({"n": "wrong"}),
        Value::Null,
    ] {
        assert!(matches!(
            r.execute("count.set", &mut s, bad, Origin::Bus),
            Err(Error::BadParams {
                id: "count.set",
                ..
            })
        ));
    }
    assert_eq!(s.n, 42);
    let runs = r.take_executed().0;
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].params, json!({"n": 42}));
    assert_eq!(runs[0].result, json!({"n": 42}));
    let schema = serde_json::to_value(
        r.describe(&s, &Strings::new(FTL))[0]
            .schema
            .as_ref()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(schema["additionalProperties"], false);
    assert_eq!(schema["properties"]["n"]["type"], "integer");
}

#[test]
fn authorisation_precedes_enablement_and_sees_the_call() {
    let mut r = Registry::new();
    let mut c = typed();
    c.enabled = |_| panic!("authorisation must run first");
    r.add(c);
    r.authorize = Some(|call| {
        assert_eq!(call.id, "count.set");
        assert_eq!(call.origin, Origin::Bus);
        assert_eq!(call.params, &json!({"n": 5}));
        Err("refused")
    });
    assert_eq!(
        r.execute(
            "count.set",
            &mut Counter::default(),
            json!({"n": 5}),
            Origin::Bus
        ),
        Err(Error::Denied {
            id: "count.set",
            reason: "refused"
        })
    );
    assert!(r.take_executed().0.is_empty());
}

#[test]
fn failed_handlers_return_typed_errors_and_are_not_journaled() {
    let mut r = registry();
    let mut c = typed();
    c.run = |_, _| Err(Error::Failed("operation failed".into()));
    r.add(c);
    assert_eq!(
        r.execute(
            "count.set",
            &mut Counter::default(),
            json!({"n": 5}),
            Origin::Bus
        ),
        Err(Error::Failed("operation failed".into()))
    );
    assert!(r.take_executed().0.is_empty());
    for e in [
        Error::Unknown("absent".into()),
        Error::Disabled {
            id: "count.set",
            reason: "locked",
        },
        Error::BadParams {
            id: "count.set",
            msg: "invalid".into(),
        },
        Error::Failed("failed".into()),
    ] {
        let typed: &dyn std::error::Error = &e;
        assert!(!typed.to_string().is_empty());
    }
}

#[test]
fn journal_skips_queries_and_keeps_other_classes_gapless() {
    let mut r = Registry::new();
    let mut query = action::nullary!("count.get", "Get", |_: &mut Counter| {});
    query.class = Class::Query;
    r.add(query);
    for (id, class) in [
        ("intent", Class::Intent),
        ("input", Class::Input),
        ("system", Class::System),
    ] {
        let mut c = action::nullary!(id, id, |_: &mut Counter| {});
        c.class = class;
        r.add(c);
    }
    let mut s = Counter::default();
    r.execute("count.get", &mut s, Value::Null, Origin::Bus)
        .unwrap();
    r.note("count.get", Origin::Ui);
    for id in ["intent", "input", "system"] {
        r.execute(id, &mut s, Value::Null, Origin::Bus).unwrap();
    }
    let events = r.take_executed().0;
    assert_eq!(
        events
            .iter()
            .map(|e| (e.id.as_str(), e.seq))
            .collect::<Vec<_>>(),
        [("intent", 1), ("input", 2), ("system", 3)]
    );
}

#[test]
fn choices_toggles_scopes_and_labels_are_ui_free() {
    let mut r = registry();
    r.add_choice(
        Place {
            id: "choose",
            label: "cmd-bump",
            menu: None,
            submenu: None,
            group: 0,
        },
        |s| s.n = 3,
        |s| s.n == 3,
    );
    r.add_toggle(
        action::nullary!("toggle", "cmd-reset", |s: &mut Counter| s.locked =
            !s.locked),
        |s| s.locked,
    );
    r.scope("count.bump", Scope::Browsing);
    r.relabel(
        "choose",
        |s| if s.n == 3 { "cmd-reset" } else { "cmd-bump" },
    );
    let mut s = Counter::default();
    assert_eq!(r.scope_of("count.bump"), Scope::Browsing);
    assert_eq!(r.scope_of("choose"), Scope::Anywhere);
    assert_eq!(r.checked("choose", &s), Some(false));
    r.execute("choose", &mut s, Value::Null, Origin::Menu)
        .unwrap();
    r.execute("toggle", &mut s, json!({}), Origin::Menu)
        .unwrap();
    assert_eq!(r.checked("choose", &s), Some(true));
    assert_eq!(r.checked("toggle", &s), Some(true));
    assert_eq!(r.label_of("choose", &s), Some("cmd-reset"));
    assert_eq!(r.describe(&s, &Strings::new(FTL))[2].label, "Reset");
    assert!(matches!(
        r.execute("choose", &mut s, json!({"n": 8}), Origin::Bus),
        Err(Error::BadParams { .. })
    ));
}

#[test]
#[should_panic(expected = "duplicate command id")]
fn duplicate_ids_are_refused() {
    registry().add(action::nullary!("count.bump", "x", |_: &mut Counter| {}));
}

#[test]
fn bindings_move_unbind_and_restore_without_a_ui() {
    let mut r = registry();
    r.add_shortcuts("count.bump", ["Ctrl+U".into(), "Ctrl+B".into()]);
    assert_eq!(r.shortcuts_of("count.bump"), ["Ctrl+B", "Ctrl+U"]);
    let overrides = [("count.reset".to_owned(), vec!["Ctrl+B".to_owned()])].into();
    assert!(r.rebind(&overrides, |s| Some(s.to_owned())).is_empty());
    assert_eq!(r.shortcuts_of("count.bump"), ["Ctrl+U"]);
    assert_eq!(r.shortcuts_of("count.reset"), ["Ctrl+B"]);
    let overrides = [("count.bump".to_owned(), Vec::new())].into();
    r.rebind(&overrides, |s| Some(s.to_owned()));
    assert!(r.shortcuts_of("count.bump").is_empty());
    r.rebind(&Default::default(), |s| Some(s.to_owned()));
    assert_eq!(r.shortcuts_of("count.bump"), ["Ctrl+B", "Ctrl+U"]);
    assert!(r.shortcuts_of("count.reset").is_empty());
}

#[test]
fn extra_chords_never_become_menu_accelerators() {
    let mut r = registry();
    r.add_shortcuts("count.reset", ["Ctrl+U".into()]);
    r.rebind(&Default::default(), |s| Some(s.to_owned()));
    assert_eq!(r.bound_of("count.reset"), (None, vec!["Ctrl+U".into()]));
    let row =
        serde_json::to_value(&r.describe(&Counter::default(), &Strings::new(FTL))[1]).unwrap();
    assert_eq!(row["shortcut"], Value::Null);
    assert_eq!(row["more"], json!(["Ctrl+U"]));
}

#[test]
fn late_commands_survive_rebinding_and_accept_more_bindings() {
    let mut r = registry();
    r.rebind(&Default::default(), |s| Some(s.to_owned()));
    let mut late = action::nullary!("late", "Late", |_: &mut Counter| {});
    late.shortcut = Some("Ctrl+L");
    r.add(late);
    r.rebind(&Default::default(), |s| Some(s.to_owned()));
    assert_eq!(r.shortcuts_of("late"), ["Ctrl+L"]);
    r.set_shortcut("late", Some("Ctrl+X".into()));
    r.add_shortcuts("late", ["Ctrl+Y".into()]);
    assert_eq!(r.shortcuts_of("late"), ["Ctrl+X", "Ctrl+Y"]);
}

#[test]
fn disabled_precedes_bad_parameters() {
    let mut r = registry();
    let mut c = typed();
    c.enabled = |_| Err("locked");
    r.add(c);
    let mut s = Counter {
        locked: true,
        ..Default::default()
    };
    for id in ["count.bump", "count.set"] {
        assert!(matches!(
            r.execute(id, &mut s, json!({"bad": true}), Origin::Bus),
            Err(Error::Disabled {
                reason: "locked",
                ..
            })
        ));
    }
    assert!(r.take_executed().0.is_empty());
}

#[test]
#[allow(deprecated)] // Exercise the transitional API directly until step 7.
fn legacy_nullary_uses_its_enabled_and_run_callbacks() {
    let mut r = Registry::new();
    let mut c = typed();
    c.run = |_, _| panic!("legacy callback must run instead");
    r.add_nullary(c, |s| !s.locked, |s| s.n += 1);
    let mut s = Counter::default();
    assert_eq!(
        r.execute("count.set", &mut s, Value::Null, Origin::Bus),
        Ok(Value::Null)
    );
    assert_eq!(s.n, 1);
    s.locked = true;
    assert!(matches!(
        r.execute("count.set", &mut s, Value::Null, Origin::Bus),
        Err(Error::Disabled { .. })
    ));
    assert_eq!(r.take_executed().0.len(), 1);
    let c = r.get("count.set").unwrap();
    assert!(c.params.is_none());
    assert_eq!(c.class, Class::Intent);
}

#[test]
fn chords_are_stored_in_canonical_modifier_order() {
    let mut r = registry();
    let mut c = action::nullary!("canonical", "Canonical", |_: &mut Counter| {});
    c.shortcut = Some("Alt+Shift+Ctrl+C");
    r.add(c);
    r.add_shortcuts("canonical", ["Shift+Ctrl+Alt+C".into()]);
    assert_eq!(r.shortcuts_of("canonical"), ["Ctrl+Alt+Shift+C"]);
}

#[test]
fn denied_precedes_both_enabled_and_disabled_without_running() {
    for locked in [false, true] {
        let mut r = registry();
        r.authorize = Some(|_| Err("refused"));
        let mut s = Counter { locked, n: 0 };
        let error = r.execute("count.bump", &mut s, json!({"bad": true}), Origin::Bus);
        assert_eq!(
            error,
            Err(Error::Denied {
                id: "count.bump",
                reason: "refused"
            })
        );
        assert_eq!(
            error.unwrap_err().to_string(),
            "command \"count.bump\" is denied: refused"
        );
        assert_eq!(s.n, 0);
        assert!(r.take_executed().0.is_empty());
    }
}

#[test]
fn execute_request_is_strict_and_reports_its_schema() {
    use action::ExecuteRequest;
    for body in [json!({"id": "x"}), json!({"id": "x", "params": null})] {
        let request: ExecuteRequest = serde_json::from_value(body).unwrap();
        assert_eq!(request.id, "x");
        assert_eq!(request.params, None);
    }
    let body = json!({"id": "x", "params": {"n": 2}});
    let request: ExecuteRequest = serde_json::from_value(body.clone()).unwrap();
    assert_eq!(serde_json::to_value(request).unwrap(), body);
    for bad in [
        json!({}),
        json!({"id": 1}),
        json!({"id": "x", "extra": true}),
    ] {
        assert!(serde_json::from_value::<ExecuteRequest>(bad).is_err());
    }
    let schema = serde_json::to_value(schema_for!(ExecuteRequest)).unwrap();
    assert_eq!(schema["additionalProperties"], false);
    assert!(schema["properties"]["params"].is_object());
}

#[test]
fn journal_bounds_both_values_and_preserves_the_accepted_body() {
    let mut r = Registry::new();
    let mut c = typed();
    c.run = |_, _| Ok(json!({"returned": "y".repeat(5000)}));
    r.add(c);
    let params = json!({"accepted": "x".repeat(5000)});
    let result = r
        .execute(
            "count.set",
            &mut Counter::default(),
            params.clone(),
            Origin::Bus,
        )
        .unwrap();
    assert!(result["returned"].as_str().unwrap().len() > 4096);
    let event = r.take_executed().0.remove(0);
    assert_eq!(event.params, action::executed::bounded(params));
    assert_eq!(event.result, action::executed::bounded(result));
    let mut c = typed();
    c.id = "defaults";
    c.run = |_, _| Ok(json!({"n": 9}));
    r.add(c);
    for params in [Value::Null, json!({})] {
        r.execute(
            "defaults",
            &mut Counter::default(),
            params.clone(),
            Origin::Bus,
        )
        .unwrap();
        assert_eq!(r.take_executed().0[0].params, params);
    }
    let r = registry();
    r.execute(
        "count.bump",
        &mut Counter::default(),
        json!({}),
        Origin::Bus,
    )
    .unwrap();
    r.note("external", Origin::Ui);
    for run in r.take_executed().0 {
        assert_eq!(run.params, Value::Null);
        assert_eq!(run.result, Value::Null);
    }
}

#[test]
#[should_panic(expected = "duplicate shortcut")]
fn reordered_modifiers_are_duplicate_chords_at_add() {
    let mut r = Registry::new();
    for (id, shortcut) in [("one", "Shift+Ctrl+B"), ("two", "Ctrl+Shift+B")] {
        let mut c = action::nullary!(id, id, |_: &mut Counter| {});
        c.shortcut = Some(shortcut);
        r.add(c);
    }
}
