---
title: citizen
description: An app's supervised native Bus connection, shared by the MixOS apps for registration, calls, replies, single-instance launch and window restore.
---

# citizen

`call_app_with_limit` accepts separate registration-wait and per-command
deadlines. The default `call_app` command limit is 30 seconds. A registration
wait expiring before another attempt reports that the app did not appear;
a deadline during a delivered call reports an uncertain outcome. Callers
must not retry that outcome. Ced uses a 120-second limit for `email.send`.

An app's supervised native Bus connection, shared by the MixOS apps
(Workshop, [Prefs](../prefs/README.md)). Headless: no UI dependency.

- `start(app, service, url)`: register as `service` and receive
  `Delivery`s: commands for the app, `Changed` (service registration),
  `Theme`, `Connected` and `Disconnected`.
- `Handle`: `raw`/`call` other services (30 s), or `raw_within` with a
  longer limit for work that legitimately runs long; `reply` once to each
  command; `quit`, which answers every accepted command
  (`BUSY: <app> is closing`) before the connection closes; `wait_done`.
- `probe`/`forward`: single-instance launch through `<app>.ping` and
  `<app>.show`; `noded_url()`: the broker this session uses.
- `Handle::caller_key(id)`: the pending command's broker-attested caller,
  `local:<from>`, `mesh:<service>@<peer>` or `anon`. It is retained until
  the app replies or disconnects. `caller_key(&IncomingCommand)` provides
  the same formatter for apps with their own native Bus lane.
- `Handle::subscribe(topic)` / `unsubscribe(topic)`: change subscriptions
  at runtime on the same supervised native connection and await the result.
  Reconnection retains the client's subscription set. `Delivery::Topic`
  remains a change hint. `take_topic_events()` drains retained topic bodies
  for tools such as Workshop: at most 256 rows and 8 MiB. Loss is reported
  first as a `citizen.topic.gap` record with `dropped`; this is local inbox
  metadata, not a published topic. Consumers still validate untrusted bodies.
- `show(handle, comp, app_id)`: restore and focus this process's window
  through [compd](../compd/README.md).
- With the `testing` feature: `Handle::sink()`, a handle with no connection
  that logs its calls, replies and quit.

Promoted from Workshop's engine when Prefs became its second owner (see
[AGENTS.md](https://github.com/markc/mixos/blob/main/AGENTS.md) §2.2). Test
with `cargo test -p citizen`.

The source is [`libs/citizen`](https://github.com/markc/mixos/tree/main/libs/citizen).
