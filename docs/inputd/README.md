---
title: The input broker
description: How inputd turns keys into Bus verbs below the compositor, keeps the user keys for the person, and holds its keyboard grab only while its own VT is active.
---

# The input broker

`inputd` reads the keyboard below the compositor, where Left Ctrl and Right
Ctrl are still different keys. A key the person has bound fires a Bus verb
and never reaches an app. Everything else is passed on unchanged.

It is the successor of AmigaOS's `input.device`: keys become verbs, and
bindings are data that a person, an agent or a Bus message can change.

## The user keys

These strokes belong to the person, never to an app or a system default:

- **bare F1 to F12;**
- **Right Ctrl, Right Alt or Right Shift with an arrow** (any of the three
  right-hand modifiers, and no other modifier).

inputd swallows every user key on its press, repeats and release unless the
person has bound it to a verb or marked it passthrough. The shipped keymap
binds none of them. `Ctrl+F3`, bare arrows and Left-hand modifier arrows
stay with the apps.

The one shipped binding is the system-reserved recovery chord `Ctrl+Alt+P`
(Left or Right Ctrl with Left Alt), which toggles the scene editor.

## How a key is decided

Each key is decided **once, at its press**, and keeps that decision until
it is released:

- **fire:** a binding matched, so its verb is sent and the key is
  swallowed. A row with `repeat: "allow"` fires again on each repeat.
- **swallow:** an unbound user key.
- **pass:** anything else, or a passthrough row. The key is re-emitted on
  the `inputd virtual keyboard` device.

Releasing Right Ctrl before → still swallows the → release. A binding
changed while a key is held applies from that key's next press.

## Grab and VTs

With `--grab DEVICE` or `--grab-name NAME`, inputd grabs the keyboard
(`EVIOCGRAB`) and re-emits what it passes. With `--vt N[,M...]` it holds the
grab only while one of those VTs is the active console:

- **On switch-away,** it releases on the re-emitter every key it had passed
  and is still held, then drops the grab. The next VT's compositor reads the
  keyboard directly.
- **On switch-to,** it takes the grab again, but only once **no key is
  down**. A key held across the grab would stay down in the compositor,
  which saw its press but would never see its release. So while the
  switch chord (Ctrl+Alt+Fn) is still held, inputd waits ungrabbed and
  tries again at each report from the keyboard. While another holder still
  has the device, it retries with a backoff from 50 ms up to 2 s.
- **If the kernel drops events** (`SYN_DROPPED`), inputd re-reads which
  keys are down, releases every key it had passed on, and swallows any key
  still held until it is released.

It watches `/sys/class/tty/tty0/active` with `poll(2)`, so a switch wakes it
at once and nothing is polled on a timer. The re-emitter is made once, at
start, and kept: a compositor in a container cannot see an input device that
appears after it started.

## Running it

| flag | what it does |
|---|---|
| (none) | the Bus service only; no keyboard |
| `--observe DEVICE` | read a keyboard without grabbing it, and log what each key would do |
| `--grab DEVICE` | grab this event node |
| `--grab-name NAME` | grab the node whose name in `/proc/bus/input/devices` is NAME |
| `--vt N[,M...]` | hold the grab only on these VTs |
| `--grab-timeout SECONDS` | exit after this long, which releases the grab |
| `--keymap PATH` | the keymap file (default: `keymap.json` in the state directory) |

The unit `inputd.service` runs the Bus service only. An operator turns the
grab on with a drop-in, after a supervised run with `--grab-timeout` from a
shell that can kill it. Any exit of inputd, SIGKILL included, releases the
grab.

## The keymap file

`/var/lib/mixos/inputd/keymap.json` (the unit's state directory), in the
keymap.v1 format:

```json
{"version": 1, "physical": [
  {"stroke": {"code": 106, "modifiers": {"right_ctrl": true}},
   "action": "comp.workspace.switch", "args": {"index": "next"},
   "service": "comp", "repeat": "allow"}
]}
```

`code` is the Linux key code. `service` names the Bus service the verb goes
to; without it, the verb's first dot-segment does. `args` is the verb's
body. A row with `"passthrough": true` re-emits its stroke instead.

## Verbs

| verb | what it does |
|---|---|
| `input.query` | the rows, the mode and the generation |
| `input.bind` | add or replace a row (`{"layer":"physical", ...row}`) |
| `input.unbind` | remove the row on a stroke (`{"code":..., "modifiers":{...}}`) |
| `input.mode` | `{"mode":"transparent"}` passes every key unchanged; `normal`; empty toggles |
| `input.reload` | re-read the keymap file |
| `input.key` | inject a key press, release or tap |
| `input.pointer.move`, `.button`, `.scroll` | inject pointer input |

Bindings are written through to the keymap file. `MIXOS_MESH_OPEN=0`
limits the keymap changes to node-local callers.
