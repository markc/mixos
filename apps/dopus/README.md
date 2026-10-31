# dopus

MixOS's twin-pane file manager.

Documentation: <https://mixos.dev/dopus/> — source [`docs/dopus/README.md`](../../docs/dopus/README.md).

`dopus.dialog {}` reads the open modal as `{dialog: {kind, title,
buttons: [{id, label, role}], body?, path?, status?} | null}`.
`{open: null}` closes it as Escape does; `{answer: <button id>}` presses
a listed button. No modal returns `NO_DIALOG`; a bad button returns `ARGUMENT`.
Help opens with `{open: "shortcuts"}` or `{open: "about"}`. Delete uses
yes/no; name uses `{answer: "ok", name: "..."}` or cancel; drop uses
move/copy/cancel. Headless instances retain delete and name prompts until
answered over the Bus, so file operations work without a window.
