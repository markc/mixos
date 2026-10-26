# inputd contract changes

## 0.5.0 (draft)

The broker enters MixOS. The wire is unchanged: Bus service `inputd`, the
`input.*` commands and their bodies, and the keymap.v1 file
(`{"version":1,"physical":[...]}`); a keymap written by the earlier broker
loads unchanged.

- **The user keys are the user's.** Bare F1–F12, and Right Ctrl, Right Alt
  or Right Shift (any of them, nothing else) with an arrow, are swallowed
  on press, repeat and release unless the person binds them or marks them
  passthrough. The shipped keymap binds none of them: its only rows are the
  system-reserved `Ctrl+Alt+P` scene-editor chord. The earlier shipped
  F-key, Right Ctrl arrow and Right Shift passthrough rows are gone; a
  person who wants them binds them.
- **Press-latched disposition.** Each key is decided once, at its press,
  and holds that decision to its release, whatever the modifiers or the
  keymap do meanwhile. A repeat or release whose press was not seen under
  the current grab is dropped.
- **`--vt N[,M...]`.** The grab is held only while one of those VTs is
  active, followed by `poll(2)` on `/sys/class/tty/tty0/active`. On
  switch-away every key held on the re-emitter is released there, then the
  grab is dropped; on switch-to the grab is retaken, with bounded backoff
  while another holder has it. Without `--vt` the grab is held always.
- **The grab is taken only with no key down,** so no key is left held in
  the compositor; inputd waits ungrabbed until the keyboard is quiet.
- **Dropped events (`SYN_DROPPED`) are recovered:** the key state is
  re-read, every key passed on is released, and keys still held are
  swallowed until their release. While a modifier is held that way, keys
  that would go to the apps are held back too, so none goes out without
  its modifier.
- A frame written to the re-emitter is always ended with a report, on
  recovery and on switch-away, so no release is left buffered.
- While waiting for a quiet keyboard, inputd retries on the grab backoff
  schedule (to 2 s) as well as at each report.
- `input.mode` refuses a malformed body; only an empty body or `{}`
  toggles.
- The re-emitter is `inputd virtual keyboard`, created once at start; the
  injector is `inputd virtual pointer`.
- The keymap is `keymap.json` in the state directory
  (`/var/lib/mixos/inputd/`), or `--keymap PATH`.
