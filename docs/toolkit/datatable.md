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

### Context menus and focus

`.context_menu(&mut |ui, row| …)` gives rows a right-click menu. A
right-click on a row outside the selection selects it first (`Selected`),
then reports `ContextMenu { index, key }` and opens a popup that the closure
fills. Apps fill it from their command registry with
`Registry::context_menu(ui, state, strings, &ids, &why)`, which draws the
commands `ids` with their labels, shortcuts, ticks and enablement, and
returns the one chosen for the app to run with `Origin::Menu`.
`Registry::ids_in(menu)` lists a menu's commands.

`datatable::is_focused(ctx, id)` says whether the table has the keyboard,
and `datatable::focus(ctx, id)` gives it the keyboard. Focus-scoped
shortcuts, such as single keys that must never fire in a text field, use
these.

### Dragging rows

`.draggable(&|count| label)` makes rows a drag source. A drag that starts
on a selected row carries the whole selection, in page order. One that
starts on an unselected row selects that row and carries it alone. The
table reports `DragStarted { keys }`, puts a `RowsDrag { table, keys }`
payload on egui's `DragAndDrop`, and shows `label(count)`, for example "3
messages", in a badge by the pointer. A drop target reads the payload with
`DragAndDrop::payload::<RowsDrag>(ctx)`, checks `table`, and takes it on
release. Email's folder tree works this way. Agents never drag: the app's
own verb, `email.move` in Email, takes the same state path.

### Columns shown and density

`TableState::hidden` holds the columns the person hid. `ColumnShown`
toggles one, and `DensityChanged` switches between compact and
comfortable. Both are preferences, so an app keeps them with the widths.

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

### The table verbs

Every table can be read and driven through the Bus with no extra app
code. `toolkit::datatable::verbs` is a standard verb set that an app mounts
under its own prefix:

| Verb | Arguments | Does |
|---|---|---|
| `<app>.table.state` | — | columns (id, label key, sortable, width, visible), sort, order, page, page size, pages, total, range, selection, focus, density and the shown rows |
| `<app>.table.sort` | `column`, optional `order` (`asc`/`desc`) | sorts, as a header click does |
| `<app>.table.page` | `index` (0-based, or `first`/`previous`/`next`/`last`) | shows a page |
| `<app>.table.page_size` | `n` | sets the page size |
| `<app>.table.select` | `rows` (keys or indices) | selects rows, the first replacing the selection |
| `<app>.table.activate` | `row` | activates a row, as Enter or a double-click does |
| `<app>.table.context` | `row` | selects a row for its context menu (the menu's commands run through `<app>.execute`) |
| `<app>.table.column` | `id`, optional `width` and `visible` | resizes, shows or hides a column |
| `<app>.table.density` | `compact` or `comfortable` | sets the row height |

To mount the set:

1. List `verbs::describe(app)` in the app's `app.describe`.
2. Register the names in `verbs.conf.mix`, with `table.state` as a query
   and the rest as intents.
3. When one arrives, call
   `verbs::request(verb, args, &verbs::Table { columns, state, total, keys, allow_unsorted })`.
   It checks the arguments against the table. It returns either
   `Request::State` or `Request::Events(events)`, which are the same
   `TableEvent`s a person's clicks raise. A bad argument is refused as
   `Refusal::Invalid` or `Refusal::NotFound`.
4. Apply the events through the path the UI's events take, then answer
   with `TableState::describe` plus `rows_json`.

`verbs::verb_for(&event)` names the verb for each event, so an app's
registry test can check that every event its table raises has a verb.
Email mounts the set as `email.table.*` for its message list. There
`table.page_size` is refused, because the page size is the
`email.mail.page_size` setting.

## Tests

The engine's rules (sort cycling, page arithmetic, keyboard moves,
selection clamping, the client-side helpers) are unit tests in
`datatable/model.rs`. `libs/toolkit/tests/datatable.rs` drives a sample
table through kittest (header sorts, row selection and keys, double-click,
the pager, an empty page) and snapshots it in Pro light and Studio dark.
