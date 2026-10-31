# term

The MixOS terminal (`dev.mixos.term`): tabs, split panes, and a grid drawn on the GPU.

Documentation: <https://mixos.dev/term/> — source [`docs/term/README.md`](../../docs/term/README.md).

`term.dialog {}` reads the open modal as `{dialog: {kind, title,
buttons: [{id, label, role}], body?, path?, status?} | null}`.
`{open: null}` closes it as Escape does; `{answer: <button id>}` presses
a listed button. No modal returns `NO_DIALOG`; a bad button returns `ARGUMENT`.
Help opens with `{open: "keys"}` or `{open: "about"}` and closes with
`{answer: "done"}`.
