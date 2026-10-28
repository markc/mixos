---
title: editor-model
description: The headless text-editing model shared by MixOS editors, turning commands and motions into edits and keeping a local mirror of an edit-service buffer in step with the server, plus highlighting and diagnostics state.
---

# editor-model

`editor-model` is the frontend side of the [edit service](../editd/README.md):
everything an editor needs that is not drawing. It has no UI toolkit, needs
no async runtime and does no Bus I/O. The transport stays in the consumer;
the mirror only produces requests (`types::Outgoing`) and consumes replies
and events.

Used by:

- [editor](../editor/README.md), the shared egui editor widget, which turns
  keys and pointer events into `EditCommand`s;
- [ced](../ced/README.md) and its `documents` crate, including
  `ced --headless`, which drive the mirror against `editd`.

It builds on the [edit](../edit/README.md) library (`Text`, `ot`, `view`,
`wire`, origins) and the [`syntax`](../syntax/README.md) crate (lsh highlighting).

The source is [`libs/editor-model`](https://github.com/markc/mixos/tree/main/libs/editor-model).

## Modules

| Module | What it holds |
|---|---|
| `model` | `EditorModel`: selection, preferred column, scroll, overwrite, remote carets, change markers and the input-method composition range. `command()` turns an `EditCommand` into a `LocalEdit`. |
| `mirror` | `Mirror`: one buffer's view text (confirmed rev + in-flight + queue) with local echo and single-authority OT rebase against `editd`. Pure: no Bus, no clock; every method returns a `Step`. |
| `highlight` | `Highlight`: per-view highlighting state, lsh languages through `syntax::cache`, Mix through the Mix lexer. |
| `diag` | `Diagnostics`: per-source diagnostic sets (`mix lint --json` output, or external sets) mapped through edits. |
| `types` | Shared types every frontend speaks: `Intent`, `LocalEdit`, `ViewDelta`, `Outgoing`, `Incoming`, `Conflict`, `Notice`, `OpIdGen`, `bus_lane_label`. |
| `fake` | `FakeEditd`, a fake `edit` daemon over the real `edit::Buffer`, for tests (feature `fake`). |

## The editor model

`EditorModel::command(&text, &cfg, command)` applies an `EditCommand` to the
current view text and returns the `LocalEdit` to send, or `None` for a pure
motion. Commands: insert, newline, backspace, delete, delete word left or
right, tab, outdent, duplicate line, delete line, move line up or down,
toggle comment, `Move { to, extend }`, select all, select word
(double-click), select line (triple-click) and set selection. Motions are
left and right, word left and right, up and down, smart Home, End, page up
and down, document start and end, and `To(offset)`.

The contracts, from the module docs:

- Every motion is grapheme-correct through `edit::view`; Up and Down keep
  `preferred_cells`.
- A multi-line Tab or Outdent is one multi-item `LocalEdit`: one
  `edit.apply`, one undo group. `coalesce` is set only for single-grapheme
  typing and backspace/delete runs.
- Newline copies the line's leading whitespace and uses the buffer's line
  ending. ToggleComment uses the language's line-comment token
  (`line_comment_for`: `--` for Mix, `//` for Rust, C, C++, Go and
  JavaScript, `#` for shell, Python, TOML and YAML; other languages get
  nothing).
- `apply_delta` maps the selection, remote carets and markers through a
  `ViewDelta`. A delta that overlaps the composition range cancels it.
- Lines are 1-based, cells 0-based.

```rust
use edit::text::Text;
use edit::view::MeasureCfg;
use editor_model::model::{EditCfg, EditCommand, EditorModel};

let cfg = EditCfg {
    measure: MeasureCfg { tab_size: 4, ambiguous_wide: false },
    insert_spaces: true,
    eol: "\n",
    line_comment: Some("--"),
};
let text = Text::from_text("hello\n").unwrap();
let mut m = EditorModel::default();
let edit = m.command(&text, &cfg, EditCommand::Insert("> ".into()));
// `edit` is Some(LocalEdit { items, coalesce, caret_after }) for the mirror.
```

## The mirror

`Mirror::bootstrap` starts from an `edit.open` reply. The host then feeds it
`local_edit`, `server_op` (undo, redo, save, reload, apply at a revision),
`on_event` (`edit.changed`), `on_reply`, `on_deadline`, `on_page` and
`on_history`, and after every call drains `next_outgoing`,
`take_retry_timer`, `take_echo_timer` and `take_outcomes`. The deltas it
produces are `ViewDelta`s, the one thing the editor side consumes: how the
view text changed, with its kind (`Local`, `Remote`, `Undo`, `Redo`,
`Reload`, `Revert`, `Resync`), revision and view generation.

The rules are binding and written out in the module docs of
[`src/mirror.rs`](https://github.com/markc/mixos/blob/main/libs/editor-model/src/mirror.rs):
one request in flight per buffer; how a server edit is folded (duplicate,
gap, own ack, or a remote fold that rebases pending ops and reports a
`Conflict`); how replies, `busy` backoff and refusals are handled; what
happens at a deadline; recovery from an event gap, a reconnect or an epoch
change (history first, then a paged snapshot); and the keep-mine transfer
of a detached copy, staged over 1 MiB.

## Highlighting and diagnostics

- **Highlight**: lsh languages are checkpointed every 1024 lines and seek
  under a per-frame `SliceBudget`; a line not reached yet draws plain. Mix
  languages (`mix`, `scene`, `mix-data`) are relexed whole off the UI
  thread, 150 ms after the last delta (the caller debounces and runs the
  worker); buffers over 2 MiB stay plain. A result tagged for another
  epoch, buffer, language or config is discarded; an older result is used
  only above the first line touched since.
- **Diagnostics**: one set per source. `LINT_SOURCE` is the frontend's own
  `mix lint --json -` run (schema version 2 only); any other source is an
  external set (`ced.diagnostics`). A diagnostic whose line was touched
  since its tagged generation is dropped; the rest map through the deltas.

## Features

- `mix` (the crate default; the workspace dependency turns default
  features off, and ced and editor enable it): Mix highlighting through the
  Mix lexer. Without it, Mix buffers draw plain.
- `fake`: the `fake` module, for consumers' tests. ced's tests enable it.

## Testing

Unit tests sit beside each module (`src/model/tests.rs`,
`src/highlight/tests.rs`, `src/diag/tests.rs`). The mirror is tested end to
end against `FakeEditd`. Like editd, the fake reports pathless buffers,
including empty scratches, as dirty until saved. Disk-backed buffers start
clean:

- `tests/fixture_server_side.rs` runs the server side of every scripted
  scenario in
  [`tests/fixtures/mirror/`](https://github.com/markc/mixos/tree/main/libs/editor-model/tests/fixtures/mirror)
  (format in its README), so a contradictory fixture fails there first.
- `tests/mirror_convergence.rs` replays every fixture through a real
  `Mirror`, checking the exact requests sent and the declared client
  expectations, then runs proptest properties over random interleavings of
  typing, agent edits, undo, reordered or dropped events and replies, lost
  requests, deadlines and dedup eviction. The emitted deltas are replayed
  onto a shadow string that must match the mirror's text.

On an authorised build worker:

```
cargo test --locked -p editor-model
```

A vertical-motion benchmark is `#[ignore]`d; run it in release on a build
node.
