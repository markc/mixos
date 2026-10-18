// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The table's Bus verbs: one set any app mounts under its own prefix
//! (`<app>.table.*`), so every table is readable and drivable by agents
//! with no app code beyond the mount.
//!
//! [`request`] turns a verb and its arguments into the same
//! [`TableEvent`]s a person's clicks and keys raise, checked against the
//! table's columns, state and rows; the app applies them through the path
//! its UI events take. `table.state` asks the app to answer with
//! [`TableState::describe`] and the rows ([`super::rows_json`]).

use serde_json::{Value, json};

use super::model::{Column, Density, Order, PageMove, Pick, Sort, TableEvent, TableState, Total};

/// The verbs, after the app's prefix.
pub const VERBS: [&str; 9] = [
    "table.state",
    "table.sort",
    "table.page",
    "table.page_size",
    "table.select",
    "table.activate",
    "table.context",
    "table.column",
    "table.density",
];

/// Every table verb under `app`, as `HELP` and `app.describe` list them:
/// name, args, description and `read_only`.
pub fn describe(app: &str) -> Vec<Value> {
    let verbs = [
        (
            "table.state",
            json!({}),
            "The table: columns (id, label key, sortable, width, visible), sort, order, page, page size, pages, total, shown range, selection, focus, density, and the shown rows (key, cells by column id)",
            true,
        ),
        (
            "table.sort",
            json!({"column":"column id (null: unsorted, where the table allows it)","order":"optional asc|desc (default the column's first order)"}),
            "Sort by a column, as its header click does; the page goes back to the first",
            false,
        ),
        (
            "table.page",
            json!({"index":"page, 0-based (or first|previous|next|last)"}),
            "Show a page",
            false,
        ),
        (
            "table.page_size",
            json!({"n":"rows per page, 1 or more"}),
            "Set the page size; the page goes back to the first",
            false,
        ),
        (
            "table.select",
            json!({"rows":"row keys (or 0-based indices on the page), the first replacing the selection","all":"or true: every row of the page (Ctrl+A); or a scope name (the app's, e.g. \"folder\"): every row of the whole set"}),
            "Select rows, as clicks (and Ctrl-clicks, Ctrl+A, the footer's select-all link) do",
            false,
        ),
        (
            "table.activate",
            json!({"row":"row key or 0-based index"}),
            "Activate a row, as a double-click or Enter does",
            false,
        ),
        (
            "table.context",
            json!({"row":"row key or 0-based index"}),
            "Select a row for its context menu, as a right-click does (the menu's commands run through <app>.execute)",
            false,
        ),
        (
            "table.column",
            json!({"id":"column id","width":"optional points (resizable columns)","visible":"optional bool"}),
            "Resize, show or hide a column",
            false,
        ),
        (
            "table.density",
            json!({"density":"compact|comfortable"}),
            "Set the row height",
            false,
        ),
    ];
    verbs
        .into_iter()
        .map(|(verb, args, description, read_only)| {
            json!({"name":format!("{app}.{verb}"),"args":args,"description":description,"read_only":read_only})
        })
        .collect()
}

/// The verb that raises the same change as `event` (`None`: a drag, which
/// agents do with the app's own move verb).
pub fn verb_for(event: &TableEvent) -> Option<&'static str> {
    Some(match event {
        TableEvent::SortChanged(_) => "table.sort",
        TableEvent::PageChanged(_) => "table.page",
        TableEvent::PageSizeChanged(_) => "table.page_size",
        TableEvent::Selected { .. }
        | TableEvent::SelectionChanged { .. }
        | TableEvent::SelectEverything => "table.select",
        TableEvent::Activated { .. } => "table.activate",
        TableEvent::ContextMenu { .. } => "table.context",
        TableEvent::ColumnResized { .. } | TableEvent::ColumnShown { .. } => "table.column",
        TableEvent::DensityChanged(_) => "table.density",
        TableEvent::DragStarted { .. } => return None,
    })
}

/// Why a table verb was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// A malformed or out-of-range argument.
    Invalid(String),
    /// No such column or row.
    NotFound(String),
}

/// What a table verb asks of the app.
#[derive(Clone, Debug, PartialEq)]
pub enum Request {
    /// Answer with the table's description and rows.
    State,
    /// Apply these, in order, as if the person had raised them; then
    /// answer with the table's state.
    Events(Vec<TableEvent>),
}

/// The table the verbs act on, as the app shows it.
pub struct Table<'a> {
    pub columns: &'a [Column],
    pub state: &'a TableState,
    pub total: Total,
    /// The shown page's row keys, in order.
    pub keys: &'a [String],
    pub allow_unsorted: bool,
}

/// Verb `verb` (after the app's prefix) with `args` on `table`.
pub fn request(verb: &str, args: &Value, table: &Table) -> Result<Request, Refusal> {
    let invalid = |m: &str| Refusal::Invalid(m.to_owned());
    let state = table.state;
    let events = match verb {
        "table.state" => return Ok(Request::State),
        "table.sort" => {
            let sort = match args.get("column") {
                None => return Err(invalid("column is required")),
                Some(Value::Null) if table.allow_unsorted => None,
                Some(Value::Null) => return Err(invalid("this table is always sorted")),
                Some(Value::String(id)) => {
                    let column = table
                        .columns
                        .iter()
                        .find(|c| c.id == id)
                        .ok_or_else(|| Refusal::NotFound(format!("no column {id}")))?;
                    if !column.sortable {
                        return Err(invalid("that column does not sort"));
                    }
                    let order = match args.get("order").and_then(Value::as_str) {
                        None => column.first_order,
                        Some(name) => Order::from_name(name)
                            .ok_or_else(|| invalid("order must be asc or desc"))?,
                    };
                    Some(Sort::new(column.id, order))
                }
                Some(_) => return Err(invalid("column must be a string or null")),
            };
            vec![TableEvent::SortChanged(sort)]
        }
        "table.page" => {
            let to = match args.get("index") {
                Some(Value::String(name)) => match name.as_str() {
                    "first" => PageMove::First,
                    "previous" => PageMove::Previous,
                    "next" => PageMove::Next,
                    "last" => PageMove::Last,
                    _ => {
                        return Err(invalid(
                            "index must be a number or first|previous|next|last",
                        ));
                    }
                },
                Some(value) => PageMove::To(
                    whole(value).ok_or_else(|| invalid("index must be a whole number"))?,
                ),
                None => return Err(invalid("index is required")),
            };
            if let PageMove::To(page) = to
                && state.last_page(table.total).is_some_and(|last| page > last)
            {
                return Err(invalid("past the last page"));
            }
            // Already there is no change, and no refusal.
            state.page_event(to, table.total).into_iter().collect()
        }
        "table.page_size" => {
            let n = args
                .get("n")
                .and_then(whole)
                .filter(|n| *n >= 1)
                .and_then(|n| u32::try_from(n).ok())
                .ok_or_else(|| invalid("n must be a whole number, 1 or more"))?;
            vec![TableEvent::PageSizeChanged(n)]
        }
        "table.select" if args.get("all").is_some() => match args.get("all") {
            // Every row of the page, as Ctrl+A.
            Some(Value::Bool(true)) => state.select_all(table.keys).into_iter().collect(),
            // Every row of the whole set, as the footer's link.
            Some(Value::String(_)) => vec![TableEvent::SelectEverything],
            _ => {
                return Err(invalid(
                    "all must be true (the page) or a scope name (every row)",
                ));
            }
        },
        "table.select" => {
            let rows = match args.get("rows") {
                Some(Value::Array(rows)) if !rows.is_empty() => rows.clone(),
                Some(row @ (Value::String(_) | Value::Number(_))) => vec![row.clone()],
                _ => return Err(invalid("rows must be a key, an index or a list of them")),
            };
            let mut events = Vec::new();
            for (n, row) in rows.iter().enumerate() {
                let (index, key) = locate(row, table.keys)?;
                let pick = if n == 0 { Pick::Only } else { Pick::Toggle };
                events.push(TableEvent::Selected { index, key, pick });
            }
            events
        }
        "table.activate" | "table.context" => {
            let row = args.get("row").ok_or_else(|| invalid("row is required"))?;
            let (index, key) = locate(row, table.keys)?;
            if verb == "table.activate" {
                vec![TableEvent::Activated { index, key }]
            } else {
                vec![TableEvent::ContextMenu { index, key }]
            }
        }
        "table.column" => {
            let id = args
                .get("id")
                .and_then(Value::as_str)
                .ok_or_else(|| invalid("id is required"))?;
            let column = table
                .columns
                .iter()
                .find(|c| c.id == id)
                .ok_or_else(|| Refusal::NotFound(format!("no column {id}")))?;
            let mut events = Vec::new();
            match args.get("width") {
                None | Some(Value::Null) => {}
                Some(width) => {
                    let width = width
                        .as_f64()
                        .filter(|w| w.is_finite() && *w >= 1.0)
                        .ok_or_else(|| invalid("width must be a length of 1 point or more"))?;
                    if !column.width.resizable() {
                        return Err(invalid("that column's width is not the person's"));
                    }
                    events.push(TableEvent::ColumnResized {
                        column: id.to_owned(),
                        width: width as f32,
                    });
                }
            }
            match args.get("visible") {
                None | Some(Value::Null) => {}
                Some(Value::Bool(visible)) => events.push(TableEvent::ColumnShown {
                    column: id.to_owned(),
                    visible: *visible,
                }),
                Some(_) => return Err(invalid("visible must be true or false")),
            }
            if events.is_empty() {
                return Err(invalid("give width or visible"));
            }
            events
        }
        "table.density" => {
            let density = match args.get("density").and_then(Value::as_str) {
                Some("compact") => Density::Compact,
                Some("comfortable") => Density::Comfortable,
                _ => return Err(invalid("density must be compact or comfortable")),
            };
            vec![TableEvent::DensityChanged(density)]
        }
        other => return Err(Refusal::NotFound(format!("no table verb {other}"))),
    };
    Ok(Request::Events(events))
}

/// A whole number (Mix sends 3 or 3.0).
fn whole(value: &Value) -> Option<u64> {
    value.as_u64().or_else(|| {
        value
            .as_f64()
            .filter(|f| f.fract() == 0.0 && *f >= 0.0)
            .map(|f| f as u64)
    })
}

/// A row named by key or by index on the page.
fn locate(row: &Value, keys: &[String]) -> Result<(usize, String), Refusal> {
    let index = match row {
        Value::String(key) => keys.iter().position(|k| k == key),
        other => whole(other)
            .and_then(|i| usize::try_from(i).ok())
            .filter(|i| *i < keys.len()),
    };
    index
        .map(|i| (i, keys[i].clone()))
        .ok_or_else(|| Refusal::NotFound(format!("no row {row} on the shown page")))
}

#[cfg(test)]
mod tests {
    use super::super::model::Width;
    use super::*;

    fn columns() -> Vec<Column> {
        vec![
            Column::new("from", "from")
                .width(Width::Share {
                    share: 0.3,
                    min: 80.0,
                })
                .sortable(Order::Ascending),
            Column::new("date", "date")
                .width(Width::Fixed(80.0))
                .sortable(Order::Descending),
            Column::new("folder", "folder"),
        ]
    }

    fn ask(verb: &str, args: Value) -> Result<Request, Refusal> {
        let columns = columns();
        let state = TableState {
            page_size: 10,
            ..TableState::default()
        };
        let keys: Vec<String> = ["a", "b", "c"].map(String::from).to_vec();
        request(
            verb,
            &args,
            &Table {
                columns: &columns,
                state: &state,
                total: Total::Known(25),
                keys: &keys,
                allow_unsorted: false,
            },
        )
    }

    fn events(verb: &str, args: Value) -> Vec<TableEvent> {
        match ask(verb, args) {
            Ok(Request::Events(events)) => events,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn each_verb_raises_the_event_its_control_does() {
        assert_eq!(ask("table.state", json!({})), Ok(Request::State));
        assert_eq!(
            events("table.sort", json!({"column": "date"})),
            [TableEvent::SortChanged(Some(Sort::new(
                "date",
                Order::Descending
            )))]
        );
        assert_eq!(
            events("table.sort", json!({"column": "from", "order": "desc"})),
            [TableEvent::SortChanged(Some(Sort::new(
                "from",
                Order::Descending
            )))]
        );
        assert_eq!(
            events("table.page", json!({"index": 2})),
            [TableEvent::PageChanged(2)]
        );
        assert_eq!(
            events("table.page", json!({"index": "last"})),
            [TableEvent::PageChanged(2)]
        );
        assert!(
            events("table.page", json!({"index": 0})).is_empty(),
            "already there"
        );
        assert_eq!(
            events("table.page_size", json!({"n": 25.0})),
            [TableEvent::PageSizeChanged(25)]
        );
        assert_eq!(
            events("table.select", json!({"rows": ["b", 2]})),
            [
                TableEvent::Selected {
                    index: 1,
                    key: "b".into(),
                    pick: Pick::Only
                },
                TableEvent::Selected {
                    index: 2,
                    key: "c".into(),
                    pick: Pick::Toggle
                },
            ]
        );
        assert_eq!(
            events("table.activate", json!({"row": "a"})),
            [TableEvent::Activated {
                index: 0,
                key: "a".into()
            }]
        );
        assert_eq!(
            events("table.context", json!({"row": 1})),
            [TableEvent::ContextMenu {
                index: 1,
                key: "b".into()
            }]
        );
        assert_eq!(
            events(
                "table.column",
                json!({"id": "from", "width": 200, "visible": false})
            ),
            [
                TableEvent::ColumnResized {
                    column: "from".into(),
                    width: 200.0
                },
                TableEvent::ColumnShown {
                    column: "from".into(),
                    visible: false
                },
            ]
        );
        assert_eq!(
            events("table.density", json!({"density": "comfortable"})),
            [TableEvent::DensityChanged(Density::Comfortable)]
        );
    }

    #[test]
    fn bad_arguments_are_refused_by_kind() {
        let invalid = |verb: &str, args: Value| matches!(ask(verb, args), Err(Refusal::Invalid(_)));
        let missing =
            |verb: &str, args: Value| matches!(ask(verb, args), Err(Refusal::NotFound(_)));
        assert!(
            invalid("table.sort", json!({"column": "folder"})),
            "unsortable"
        );
        assert!(
            invalid("table.sort", json!({"column": null})),
            "always sorted"
        );
        assert!(invalid(
            "table.sort",
            json!({"column": "date", "order": "up"})
        ));
        assert!(missing("table.sort", json!({"column": "size"})));
        assert!(invalid("table.page", json!({"index": 3})), "past the last");
        assert!(invalid("table.page_size", json!({"n": 0})));
        assert!(missing("table.select", json!({"rows": ["z"]})));
        assert!(invalid("table.select", json!({"all": 3})));
        assert_eq!(
            events("table.select", json!({"all": true})),
            [TableEvent::SelectionChanged {
                keys: vec!["a".into(), "b".into(), "c".into()],
                focus: Some(0),
            }]
        );
        assert_eq!(
            events("table.select", json!({"all": "folder"})),
            [TableEvent::SelectEverything]
        );
        assert!(missing("table.activate", json!({"row": 3})));
        assert!(
            invalid("table.column", json!({"id": "date", "width": 90})),
            "fixed width"
        );
        assert!(invalid("table.column", json!({"id": "from"})));
        assert!(invalid("table.density", json!({"density": "roomy"})));
        assert!(missing("table.nope", json!({})));
    }

    #[test]
    fn every_event_but_a_drag_has_a_verb_and_every_verb_is_described() {
        let names: Vec<String> = describe("app")
            .iter()
            .map(|v| v["name"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(names, VERBS.map(|v| format!("app.{v}")));
        let all = [
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
        ];
        for event in &all {
            assert!(VERBS.contains(&verb_for(event).unwrap()), "{event:?}");
        }
        assert_eq!(
            verb_for(&TableEvent::DragStarted { keys: Vec::new() }),
            None
        );
    }
}
