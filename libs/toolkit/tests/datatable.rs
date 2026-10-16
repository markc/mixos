// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The data table driven through kittest: a header click sorts, a row
//! click selects and hands the table the keyboard, the arrows move the
//! selection and Enter activates, the pager pages, and the rows are
//! `Row` nodes agents can find. Then offscreen snapshots of a sample table
//! (long cells elided, an unread row strong, markers, the pager) in a light
//! and a dark theme. Regenerate the images with `UPDATE_SNAPSHOTS=1` and
//! look at them before committing.

#[path = "support/fixture.rs"]
mod fixture;

use design::{Mode, Scheme};
use egui::accesskit::Role;
use egui::{Key, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use toolkit::Icon;
use toolkit::datatable::{
    Align, Cell, Column, DataTable, Order, RowStyle, SelectionMode, Sort, TableEvent, TableState,
    TextRow, TextRows, Total, Width, compare_text, page_of, sort_rows,
};

/// One sample message.
#[derive(Clone)]
struct Message {
    id: &'static str,
    from: &'static str,
    subject: &'static str,
    /// Sorts as text: ISO dates.
    date: &'static str,
    unread: bool,
    flagged: bool,
    attachment: bool,
}

const MESSAGES: [Message; 7] = [
    Message {
        id: "m1",
        from: "Ann Lee",
        subject: "Lunch on Friday?",
        date: "2026-10-16",
        unread: true,
        flagged: false,
        attachment: false,
    },
    Message {
        id: "m2",
        from: "Bob Rivera-Montgomery of the Extremely Long Display Name Society",
        subject: "Quarterly report: the full numbers, every appendix, and a summary of the summary",
        date: "2026-10-15",
        unread: false,
        flagged: true,
        attachment: true,
    },
    Message {
        id: "m3",
        from: "cy@example.invalid",
        subject: "Minutes",
        date: "2026-10-14",
        unread: false,
        flagged: false,
        attachment: false,
    },
    Message {
        id: "m4",
        from: "Dee",
        subject: "Re: build cluster",
        date: "2026-10-13",
        unread: true,
        flagged: true,
        attachment: false,
    },
    Message {
        id: "m5",
        from: "Eve",
        subject: "Photos",
        date: "2026-10-12",
        unread: false,
        flagged: false,
        attachment: true,
    },
    Message {
        id: "m6",
        from: "Fay",
        subject: "Invoice 1043",
        date: "2026-09-30",
        unread: false,
        flagged: false,
        attachment: true,
    },
    Message {
        id: "m7",
        from: "Gus",
        subject: "Old thread",
        date: "2025-12-01",
        unread: false,
        flagged: false,
        attachment: false,
    },
];

fn columns() -> Vec<Column> {
    vec![
        Column::new("from", "From")
            .width(Width::Share {
                share: 0.3,
                min: 80.0,
            })
            .sortable(Order::Ascending),
        Column::new("subject", "Subject")
            .width(Width::Remainder { min: 120.0 })
            .sortable(Order::Ascending),
        Column::new("date", "Date")
            .width(Width::Fixed(84.0))
            .align(Align::End)
            .sortable(Order::Descending),
    ]
}

/// A table app with every row in memory: it sorts and pages client side.
struct Sample {
    columns: Vec<Column>,
    state: TableState,
    seen: Vec<TableEvent>,
    pages: bool,
}

impl Sample {
    fn new(page_size: u32) -> Self {
        Self {
            columns: columns(),
            state: TableState {
                page_size,
                ..TableState::sorted(Sort::new("date", Order::Descending))
            },
            seen: Vec::new(),
            pages: true,
        }
    }

    fn rows(&self) -> TextRows {
        let mut all = MESSAGES.to_vec();
        sort_rows(&mut all, self.state.sort.as_ref(), |a, b, column| match column {
            "from" => compare_text(a.from, b.from),
            "subject" => compare_text(a.subject, b.subject),
            _ => a.date.cmp(b.date),
        });
        let rows = page_of(&all, &self.state)
            .iter()
            .map(|m| {
                let mut subject = Cell::new(m.subject);
                if m.flagged {
                    subject = subject.icon(Icon::Flag);
                }
                if m.attachment {
                    subject = subject.icon(Icon::Paperclip);
                }
                TextRow {
                    key: m.id.into(),
                    cells: vec![Cell::new(m.from), subject, Cell::new(m.date).weak(!m.unread)],
                    style: RowStyle {
                        strong: m.unread,
                        weak: false,
                    },
                }
            })
            .collect();
        TextRows {
            columns: vec!["from", "subject", "date"],
            rows,
        }
    }
}

fn harness(sample: Sample, scheme: Scheme, mode: Mode) -> Harness<'static, Sample> {
    let mut h = Harness::builder()
        .with_size(vec2(720.0, 260.0))
        .wgpu()
        .build_ui_state(
            |ui, s: &mut Sample| {
                let rows = s.rows();
                let events = DataTable::new("messages", &s.columns, &s.state)
                    .total(Total::Known(MESSAGES.len() as u64))
                    .selection(SelectionMode::Single)
                    .page_sizes(&[5, 10, 25])
                    .pager(s.pages)
                    .empty_text("Nothing here")
                    .show(ui, &rows);
                for event in events {
                    s.state.apply(&event, SelectionMode::Single);
                    s.seen.push(event);
                }
                let keys: Vec<String> = s.rows().rows.into_iter().map(|r| r.key).collect();
                s.state.rows_changed(&keys);
            },
            sample,
        );
    toolkit::install(&h.ctx, &fixture::theme(scheme, mode));
    h.run();
    h
}

fn light(sample: Sample) -> Harness<'static, Sample> {
    harness(sample, Scheme::Pro, Mode::Light)
}

/// The rows' labels, top to bottom.
fn row_labels(h: &Harness<'static, Sample>) -> Vec<String> {
    let mut rows: Vec<(f32, String)> = h
        .query_all_by_role(Role::Row)
        .map(|n| {
            (
                n.rect().top(),
                n.accesskit_node().label().unwrap_or_default().to_owned(),
            )
        })
        .collect();
    rows.sort_by(|a, b| a.0.total_cmp(&b.0));
    rows.into_iter().map(|(_, label)| label).collect()
}

#[test]
fn rows_are_row_nodes_labelled_with_their_cells() {
    let h = light(Sample::new(5));
    let labels = row_labels(&h);
    assert_eq!(labels.len(), 5, "{labels:?}");
    assert_eq!(labels[0], "Ann Lee, Lunch on Friday?, 2026-10-16");
    assert!(h.query_by_role(Role::Table).is_some());
}

#[test]
fn a_header_click_sorts_and_a_second_reverses() {
    let mut h = light(Sample::new(10));
    h.get_by_label("From").click();
    h.run();
    assert_eq!(
        h.state().state.sort,
        Some(Sort::new("from", Order::Ascending))
    );
    assert!(row_labels(&h)[0].starts_with("Ann Lee"));
    h.get_by_label("From").click();
    h.run();
    assert_eq!(
        h.state().state.sort,
        Some(Sort::new("from", Order::Descending))
    );
    assert!(row_labels(&h)[0].starts_with("Gus"));
    assert!(
        h.state()
            .seen
            .iter()
            .all(|e| matches!(e, TableEvent::SortChanged(Some(_))))
    );
}

#[test]
fn a_click_selects_then_the_keyboard_moves_and_enter_activates() {
    let mut h = light(Sample::new(10));
    h.get_by_label_contains("Minutes").click();
    h.run();
    assert_eq!(h.state().state.selection, vec!["m3".to_owned()]);
    h.key_press(Key::ArrowDown);
    h.run();
    assert_eq!(h.state().state.selection, vec!["m4".to_owned()]);
    h.key_press(Key::End);
    h.run();
    assert_eq!(h.state().state.selection, vec!["m7".to_owned()]);
    h.key_press(Key::Enter);
    h.run();
    assert!(matches!(
        h.state().seen.last(),
        Some(TableEvent::Activated { key, .. }) if key == "m7"
    ));
    let selected: Vec<_> = h
        .query_all_by_role(Role::Row)
        .filter(|n| n.accesskit_node().is_selected() == Some(true))
        .collect();
    assert_eq!(selected.len(), 1);
}

#[test]
fn a_double_click_activates() {
    let mut h = light(Sample::new(10));
    let row = h.get_by_label_contains("Photos").rect().center();
    h.hover_at(row);
    for _ in 0..2 {
        for pressed in [true, false] {
            h.event(egui::Event::PointerButton {
                pos: row,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            });
        }
    }
    // One pass: a kittest step is a quarter second, past the double-click
    // window.
    h.run();
    assert!(
        h.state()
            .seen
            .iter()
            .any(|e| matches!(e, TableEvent::Activated { key, .. } if key == "m5"))
    );
}

#[test]
fn the_pager_moves_between_pages() {
    let mut h = light(Sample::new(5));
    assert!(h.query_by_label("1–5 of 7").is_some());
    assert!(h.get_by_label("Previous page").accesskit_node().is_disabled());
    h.get_by_label("Next page").click();
    h.run();
    assert_eq!(h.state().state.page, 1);
    assert!(h.query_by_label("6–7 of 7").is_some());
    assert_eq!(row_labels(&h).len(), 2);
    assert!(h.get_by_label("Next page").accesskit_node().is_disabled());
    h.get_by_label("First page").click();
    h.run();
    assert_eq!(h.state().state.page, 0);
}

fn snapshot_sample() -> Sample {
    let mut sample = Sample::new(5);
    sample.state.selection = vec!["m3".into()];
    sample.state.focus = Some(2);
    sample
}

#[test]
fn a_sample_table_in_a_light_and_a_dark_theme() {
    let mut results = egui_kittest::SnapshotResults::new();
    for (id, scheme, mode) in [
        ("pro_light", Scheme::Pro, Mode::Light),
        ("studio_dark", Scheme::Studio, Mode::Dark),
    ] {
        let mut h = harness(snapshot_sample(), scheme, mode);
        h.run();
        results.add(h.try_snapshot(format!("datatable_{id}")));
    }
}

#[test]
fn an_empty_table_says_so() {
    let mut sample = Sample::new(5);
    sample.state.page = 9;
    let mut h = light(sample);
    h.run();
    assert!(row_labels(&h).is_empty());
    assert!(h.query_by_label("No rows").is_some());
}
