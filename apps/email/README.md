# email

The reserved MixOS mail client (`dev.mixos.email`): a pure JMAP client —
mail, calendar and contacts over HTTPS to any RFC 8620/8621-shaped server.
Read mail and send it; list calendars, events, address books and contacts.
The manual is [`docs/email/README.md`](../../docs/email/README.md).

- `crates/jmap`: the headless engine — session discovery, token auth, the
  wire types and calls for mail, compose, calendars and contacts. Ported
  from the retired Dioxus client's `jmap.rs` with its faults fixed.
- `src/`: the egui shell (`state` machine + `work` queue, `shell` effects
  runtime, `view` three panes, `commands` registry, `config` persistence).

The one architectural exception it owns: the app speaks HTTPS itself from
its effects runtime (a pure JMAP client's server is whatever account the
person points it at), where every other app reaches services through the
Bus.

Run it with `email` (a second launch raises the first). The first run asks
for the server URL, address, password and an optional CA bundle path for a
self-issued certificate; the account persists under the app's config
directory (`config.conf.mix`, mode 0600 — the password lives there until
MixOS grows a keyring).

Test it on a build worker:

```
cargo test -p email -p jmap
```

`tests/view.rs` drives the real view over a fixture state through kittest,
with no network.
