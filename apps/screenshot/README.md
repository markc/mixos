# screenshot

Native screenshots and editable annotations for MixOS (`dev.mixos.screenshot`).

Documentation: <https://mixos.dev/screenshot/> — source [`docs/screenshot/README.md`](../../docs/screenshot/README.md).

## Parameterised document edits

The headless `annotation` engine owns `edit.annotate` (the Shape body),
`edit.move {id, dx, dy}`, `edit.delete {id}`, `edit.crop {crop}` (null resets),
and nullary `edit.undo` / `edit.redo`. Dispatch through
`screenshot.execute {id, params}`; its reply includes the command's `result`
alongside status, dialog and job. Annotate returns document info plus the
new object's id; the other edits return document info.

Pointer strokes, text, number markers, completed object drags, crops, menus,
shortcuts and Bus calls all reach the same `command::Registry::execute`.
A drag commits on release, making one undo step and one executed event.
Delete selection is the nullary shell command `edit.delete_selected`; it
dispatches `edit.delete` with the selected id. Uncrop dispatches `edit.crop`.
The shell's dialog/job checks compose with the engine's enablement.

The six `screenshot.annotate`, `move`, `delete`, `crop`, `undo` and `redo`
verbs remain deprecated aliases of execute and preserve their old reply
bodies. `screenshot.commands` reports parameter schemas. Every successful
edit publishes `screenshot.command.executed` v2 with accepted params and
result, whether reached by hand or over the Bus. Undo/redo use the shared
`command::History`; raster workers receive a fresh history without old edits.