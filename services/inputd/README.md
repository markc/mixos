# inputd

The input broker: keys become Bus commands, the user keys stay the user's, and the keyboard grab follows the VT.

Documentation: <https://mixos.dev/inputd/> — source [`docs/inputd/README.md`](../../docs/inputd/README.md).

- `src/`: the daemon (Bus service, keymap file, evdev and uinput, the VT-aware grab).
- `crates/inputd-schema`: the wire contract (keymap.v1 rows, `input.*` commands) and the one definition of the user keys.
- `crates/inputd-core`: the pure resolver, the shipped keymap and the press-latched interceptor.
- `units/`: `inputd.service` (Bus only) and an example drop-in that turns the grab on.

Tests: `cargo test -p inputd -p inputd-core -p inputd-schema`. None needs a device.
