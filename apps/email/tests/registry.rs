// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The Bus verb registry (`docs/docs/spec/bus/verbs.conf.mix`, AGENTS.md §5)
//! lists exactly the verbs Email describes: its own (`app.describe` reports
//! them in this order) and the toolkit's drive verbs under its prefix, apart
//! from the protocol verbs every service answers.
use email::APP;
use email::verbs::{VERBS, describe};

const REGISTRY: &str = include_str!("../../../docs/docs/spec/bus/verbs.conf.mix");

/// The registry's names owned by `owner`, from its one-entry-per-line form.
fn registered(owner: &str) -> Vec<String> {
    let owned = format!("owner:\"{owner}\"");
    REGISTRY
        .lines()
        .filter(|line| line.contains(&owned))
        .filter_map(|line| {
            line.split("name:\"")
                .nth(1)?
                .split('"')
                .next()
                .map(str::to_owned)
        })
        .collect()
}

/// What `app.describe` lists, without the protocol verbs.
fn described() -> Vec<String> {
    describe()["verbs"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v["name"].as_str())
        .filter(|name| !matches!(*name, "HELP" | "app.describe"))
        .map(str::to_owned)
        .collect()
}

#[test]
fn every_described_verb_is_registered_and_nothing_else() {
    let described = described();
    assert!(!described.is_empty());
    assert_eq!(registered("email"), described);
    // The drive verbs are there under the app's prefix.
    for verb in toolkit::drive::VERBS {
        assert!(described.contains(&format!("{APP}.{verb}")), "{verb}");
    }
}

/// The registry's class for `name`, from its one-entry-per-line form.
fn class_of(name: &str) -> Option<&'static str> {
    let key = format!("name:\"{name}\"");
    REGISTRY
        .lines()
        .find(|line| line.contains(&key))?
        .split("class:\"")
        .nth(1)?
        .split('"')
        .next()
}

/// A verb the app describes as read-only is a query
/// (docs/docs/decisions/2026-10-11-verb-classes.md), and one that changes
/// something is not.
#[test]
fn read_only_verbs_are_queries() {
    for verb in describe()["verbs"].as_array().unwrap() {
        let name = verb["name"].as_str().unwrap();
        // The drive verbs are the toolkit's own family (a capture is a
        // query that writes its artefact).
        let drive = toolkit::drive::VERBS
            .iter()
            .any(|v| name == format!("{APP}.{v}"));
        if matches!(name, "HELP" | "app.describe") || drive {
            continue;
        }
        let class = class_of(name);
        if verb["read_only"] == true {
            assert_eq!(class, Some("query"), "{name}");
        } else {
            assert_ne!(class, Some("query"), "{name}");
        }
    }
}

/// Every change the message list's table can raise has an `email.table.*`
/// verb that raises it too (a drag's is `email.move`), and every verb is
/// registered.
#[test]
fn every_table_event_has_a_verb() {
    use toolkit::datatable::verbs::verb_for;
    use toolkit::datatable::{Density, Pick, TableEvent};
    let described = described();
    let events = [
        TableEvent::SortChanged(None),
        TableEvent::PageChanged(0),
        TableEvent::PageSizeChanged(1),
        TableEvent::Selected {
            index: 0,
            key: String::new(),
            pick: Pick::Only,
        },
        TableEvent::Activated {
            index: 0,
            key: String::new(),
        },
        TableEvent::ColumnResized {
            column: String::new(),
            width: 1.0,
        },
        TableEvent::ColumnShown {
            column: String::new(),
            visible: true,
        },
        TableEvent::DensityChanged(Density::Compact),
        TableEvent::ContextMenu {
            index: 0,
            key: String::new(),
        },
        TableEvent::DragStarted { keys: Vec::new() },
    ];
    for event in &events {
        let verb = match verb_for(event) {
            Some(verb) => format!("{APP}.{verb}"),
            None => format!("{APP}.move"),
        };
        assert!(described.contains(&verb), "{event:?}: {verb}");
    }
}

/// Every control of the list header (Compose, the sort headers, the pager,
/// the search and its scope) is a registry command, so `email.execute`
/// reaches it.
#[test]
fn every_list_header_control_is_a_command() {
    let registry = email::commands::registry();
    for id in [
        "mail.compose",
        "view.sort_from",
        "view.sort_subject",
        "view.sort_date",
        "view.sort_ascending",
        "view.sort_descending",
        "view.previous_page",
        "view.next_page",
        "view.search",
        "view.search_clear",
        "view.search_folder",
        "view.search_all",
    ] {
        assert!(registry.get(id).is_some(), "{id}");
    }
}

/// The registry's version for `name`, from its one-entry-per-line form.
fn version_of(name: &str) -> Option<&'static str> {
    let key = format!("name:\"{name}\"");
    REGISTRY
        .lines()
        .find(|line| line.contains(&key))?
        .split("version:\"")
        .nth(1)?
        .split('"')
        .next()
}

/// `email.list` took `sort`, `order` and `page` in 0.2.0, and `email.info`
/// the message list's table state: both payloads changed, so both moved
/// on from 0.1.0 (AGENTS.md §5).
#[test]
fn the_list_payload_change_is_versioned() {
    let list = VERBS.iter().find(|v| v.name == "email.list").unwrap();
    for arg in ["sort", "order", "page", "offset", "limit"] {
        assert!(list.args.contains(&format!("\"{arg}\"")), "{arg}");
    }
    assert_eq!(version_of("email.list"), Some("0.2.0"));
    assert_eq!(version_of("email.info"), Some("0.2.0"));
}
