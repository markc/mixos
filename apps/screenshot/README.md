# screenshot

Native screenshots and editable annotations for MixOS (`dev.mixos.screenshot`).
Print Screen starts it through compd; Ctrl+S saves into Pictures/Screenshots;
agents use the `screenshot.*` Bus verbs. The manual is
[`docs/screenshot/README.md`](../../docs/screenshot/README.md).

- `crates/annotation`: the headless engine (document, painter, viewport,
  screenshots folder, ledger).
- `src/`: the egui shell (`app` state and rules, `commands` registry,
  `capture` over compd, `jobs`, `view`, `shell`, `headless`).

Run it with `screenshot [IMAGE]`, `screenshot --capture region`, or
`screenshot --headless` (registered as `screenshot-headless`; another
compositor's instances are named after it, see `src/names.rs`).

Test it on a build worker:

```
cargo test -p screenshot -p annotation
```

`tests/native_bus.rs` runs the headless binary against an in-process
broker and a fixture compositor; `tests/snapshot.rs` renders the window
offscreen (regenerate with `UPDATE_SNAPSHOTS=1` and look at the images);
`tests/registry.rs` checks the verb registry.
