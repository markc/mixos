---
title: Data table
description: The toolkit's data table — sortable, resizable columns over virtualised one-line rows, a pager, keyboard navigation and a drive surface, with the engine kept apart from the drawing.
---

# Data table

`toolkit::datatable` draws tabular data: a header of sortable, resizable
columns over virtualised one-line rows, with a pager footer. It is built
on `egui_extras`' table for layout, scrolling and column resizing, and it
follows the engine-first rule of the
[egui applications decision](../docs/decisions/2026-10-09-egui-applications.md):
the table only reports what the person did, and the app decides what that
means.

The first user is [Email](../email/README.md)'s message list, which sorts
and pages on the server. The source is
[`libs/toolkit/src/datatable.rs`](https://github.com/markc/mixos/blob/main/libs/toolkit/src/datatable.rs)
and its engine,
[`datatable/model.rs`](https://github.com/markc/mixos/blob/main/libs/toolkit/src/datatable/model.rs).

## The pieces

| Piece | What it is |
|---|---|
| `Column` | id, Fluent label key, `Width`, `Align`, `sortable` (with the first click's `Order`), `elide` |
| `Width` | `Fixed(pt)`, `Resizable { initial, min }`, `Share { share, min }` (a fraction of the table, resizable), `Remainder { min }` |
| `Rows` | the shown page: `len`, `key(row)`, `cell(row, column)`, optional `style(row)` and `custom(ui, row, column)` |
| `Cell` | text, marker icons drawn at the cell's end, `weak` |
| `RowStyle` | `strong` (the medium face, for unread mail), `weak` |
| `TextRows` | rows held as cells, for tests and small tables |
| `TableState` | sort, page, page size, selection (row keys), focus, dragged widths, density; serialisable |
| `TableEvent` | `SortChanged`, `PageChanged`, `PageSizeChanged`, `Selected`, `Activated`, `ColumnResized` |
| `Total` | `Known(n)`, or `Unknown { more }` when the size of the set is not known |
| `DataTable` | the widget: `DataTable::new(id, &columns, &state)`, its options, then `.show(ui, &rows)` |

## Using it

```rust
use toolkit::datatable::{Column, DataTable, Order, TableState, Total, Width, Align};

let columns = vec![
    Column::new("name", "column-name")
        .width(Width::Share { share: 0.3, min: 80.0 })
        .sortable(Order::Ascending),
    Column::new("detail", "column-detail").width(Width::Remainder { min: 120.0 }),
    Column::new("size", "column-size")
        .width(Width::Fixed(72.0))
        .align(Align::End)
        .sortable(Order::Descending),
];

// each frame, from the app's state:
let events = DataTable::new("files", &columns, &state.table)
    .total(Total::Known(state.total))
    .strings(&strings)
    .page_sizes(&[50, 100, 200])
    .empty_text(strings.get("empty"))
    .show(ui, &rows);
for event in events {
    state.table.apply(&event, SelectionMode::Single);
    // then: re-sort in memory, or ask the server for the new page
}
```

Drawing never changes the app's state: `show` returns events, and the app
applies them after the frame, like every other view event.

### Sorting and paging

A header click on a sortable column reports `SortChanged`: another column
starts in its first order (a date's newest first), the sorted column
reverses, and with `allow_unsorted(true)` a third click goes back to no
sort. A sort or page change moves the page back to the first.

The table never reorders or slices rows itself. An app holding every row
in memory uses `sort_rows(&mut rows, state.sort.as_ref(), compare)` (a
stable sort; `compare_text` folds case) and `page_of(&rows, &state)`. An
app whose rows live on a server sends the sort and `state.offset()` with
its query, and keeps only the newest answer.

The footer shows "x–y of N" (or "x–y" for an unknown total), the optional
page size picker, and first, previous, next and last. `TableState::can`
and `page_event` answer the same questions for commands, so an app's
"Next Page" menu entry and its footer button agree.

### Rows, selection and the keyboard

Rows are one line at the design's row height: the chrome's menu-row
height in `Density::Compact`, an item gap more above and below in
`Comfortable`. Long cells are cut to the column with "…" and show the
whole text in a tooltip.

A click selects a row (Ctrl-click toggles one in `SelectionMode::Multi`),
a double-click activates it, and the click gives the table the keyboard.
Then Up, Down, Page Up, Page Down, Home and End move the selection and
scroll it into view, and Enter activates it. The selection is held as row
keys, so `TableState::rows_changed(&keys)` keeps it across a refresh and
drops what is gone.

### Widths and persistence

`TableState` is one serialisable struct. Persist the parts that are a
preference (`sort`, `widths`, `density`) in the app's config, and keep the
page and selection for the session. A dragged column reports
`ColumnResized` once the drag ends; the next launch starts the column at
that width.

## Agents

The table is an AccessKit `Table` node labelled with its target id. Each
row is a `Row` node labelled with its cells' text, comma separated, and
marked selected; each header is a button carrying its sort direction; the
pager's buttons are labelled "First page", "Previous page", "Next page"
and "Last page". So the [drive verbs](../docs/drive.md) work on it as a
hand does:

```mix
send email email.ui.tree role="Row"
send email email.ui.click label="Subject"
send email email.ui.click label="Next page"
```

For the data itself an app reports `TableState::describe(&columns, total,
shown)` (columns, sort, order, page, pages, total, range, selection,
focus) and `datatable::rows_json(&columns, &rows)` in its own state verb.
Email puts the first under `mail.table` in `email.info`.

## Tests

The engine's rules (sort cycling, page arithmetic, keyboard moves,
selection clamping, the client-side helpers) are unit tests in
`datatable/model.rs`. `libs/toolkit/tests/datatable.rs` drives a sample
table through kittest (header sorts, row selection and keys, double-click,
the pager, an empty page) and snapshots it in Pro light and Studio dark.
