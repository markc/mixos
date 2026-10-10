# editor

The shared plain-text editor for MixOS egui applications: a monospace,
virtualised view over the headless editing model (`editor-model`, `edit`).
ced is built on it, and so is any application that needs a text editor: a
Markdown editor, a notes pane, a config field.

- **Only visible rows are measured and drawn.** A multi-megabyte file, or a
  multi-megabyte line, costs about the same per frame as a short one.
- **Soft wrap is a view option** (`Wrap::Words`): rows break after
  whitespace, inside a word only when it is longer than a row.
- **The widget never edits text.** It turns keys, pointer, clipboard and
  input-method events into `Event`s (mostly `EditCommand`s) for the
  document's owner to apply.
- **Colours come from the theme**, through the toolkit's chrome roles, with
  every highlight class kept at 3:1 contrast or better.

## A local document

`local::Document` is the editor without a daemon: an `edit::buffer::Buffer`
with open and save (byte-order mark and line endings kept), undo and redo,
and highlighting for the 25 lsh languages, Markdown included.

```rust
let mut doc = editor::local::Document::open("notes.md")?;
let palette = editor::Palette::from_theme(&theme);
let view = editor::View::prose(); // soft wrap, no line numbers

// each frame
doc.show(ui, &palette, &view);
if doc.is_dirty() { /* offer to save: doc.save()? */ }
```

## A shared buffer

ced applies the same events to a buffer shared through the edit service
(`editor_model::mirror`) and passes other origins' carets, selections and
change markers through the same `Doc`. Any application can move from a local
document to a shared one without changing its UI code.

## Tests

`cargo test -p editor`: measurement, rows and wrapping, geometry, keys, the
input-method guard, palettes in every scheme, the local document, and
`tests/snapshot.rs`, which drives a real document through egui (typing,
selection, undo, read-only) and keeps offscreen snapshots. Regenerate the
snapshots with `UPDATE_SNAPSHOTS=1` and look at them before committing.

## Saving

`save` writes a new temporary file beside the real one (created exclusively,
so no existing file or link is followed), gives it the original's
permissions, flushes it and renames it into place. Saving through a symbolic
link writes the link's target; the link stays a link. Edits the buffer
refuses (a paste over the 1 MiB request limit) are kept for
`take_refusal`. `set_contents` is a reload: it has no request limit, and
Undo does not take it back.

## Known limits

- One monospace grid: proportional text is a separate, larger piece of work.
- With soft wrap, the scrollbar counts visual rows for documents up to
  256 KiB and lines beyond that.
- In lines over 64 KiB, which break every row width, a tab wider than a
  whole row can leave an empty row before it.
- A text area under two cells wide wraps at two cells (the widest cluster
  but a tab), so its second cell is clipped.
- A row's end and the next row's start are the same offset, and a caret has
  no affinity yet: Up or Down onto a row that breaks inside a word stops
  before its last character, so the caret stays on that row.
- Glyphs come from the theme's faces; scripts they do not cover (CJK, for
  one) draw as boxes until the font set gains a fallback.
- Without wrap, Home and End are logical; with wrap, Up and Down move by
  rows and Home and End stay logical.
- Mix-family buffers are highlighted by the Mix lexer only when the `mix`
  feature is on and the owner runs the relex (ced does).
