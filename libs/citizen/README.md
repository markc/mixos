# citizen

An app's supervised native Bus connection, shared by the MixOS apps
(BusViewer, Prefs). Headless: no UI dependency.

- `start(app, service, url)`: register as `service` and receive
  `Delivery`s: commands for the app, `Changed` (service registration),
  `Theme`, `Connected` and `Disconnected`.
- `Handle`: `raw`/`call` other services (30 s), or `raw_within` with a
  longer limit for work that legitimately runs long; `reply` once to each
  command;
  `quit`, which answers every accepted command (`BUSY: <app> is closing`)
  before the connection closes; `wait_done`.
- `probe`/`forward`: single-instance launch through `<app>.ping` and
  `<app>.show`; `noded_url()`: the broker this session uses.
- `show(handle, comp, app_id)`: restore and focus this process's window
  through compd.
- With the `testing` feature: `Handle::sink()`, a handle with no connection
  that logs its calls, replies and quit.

Promoted from BusViewer's engine when Prefs became its second owner
(AGENTS.md §2.2). Test with `cargo test -p citizen`.
