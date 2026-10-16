---
title: inputd — changelog
description: Release notes of the inputd component, mirrored from services/inputd/CHANGELOG.md.
generated: by docs/_build/gen-site.mix from services/inputd/CHANGELOG.md; do not edit
---

# inputd contract changes

> Mirrored from [`services/inputd/CHANGELOG.md`](https://github.com/markc/mixos/blob/main/services/inputd/CHANGELOG.md), where releases update it. The component's page is [inputd](README.md). This page describes the current `main` branch of MixOS.

## 0.5.0 (draft)

The broker enters MixOS. The wire is unchanged: Bus service `inputd`, the
`input.*` verbs and their bodies, and the keymap.v1 file
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
- The re-emitter is `inputd virtual keyboard`, created once at start; the
  injector is `inputd virtual pointer`.
- The keymap is `keymap.json` in the state directory
  (`/var/lib/mixos/inputd/`), or `--keymap PATH`.
- The mesh lock is `MIXOS_MESH_OPEN=0`.
