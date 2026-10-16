# email

The reserved MixOS mail client (`dev.mixos.email`): a pure JMAP client —
mail, calendar and contacts over HTTPS to any RFC 8620/8621-shaped server,
for any number of accounts, shown as a Thunderbird-style tree. The manual
is [`docs/email/README.md`](../../docs/email/README.md).

- `crates/jmap`: the headless engine — session discovery, token auth, the
  wire types and calls for mail, compose, calendars and contacts, reads
  and writes. Ported from the retired Dioxus client's `jmap.rs` with its
  faults fixed.
- `src/`: the egui shell — `state` (the engine: one view per account,
  the tree, the forms), `work` (what the state asks for), `shell` (the
  effects runtime, one client per account), `verbs` (the `email` Bus
  port), `commands` (the registry), `writer` (account changes as fenced
  settingsd applies), `secrets` (the passwords file), `config` (window
  shape, accounts cache, schema-1 migration), `view` (tree, panes,
  dialogs).

The one architectural exception it owns: the app speaks HTTPS itself from
its effects runtime (a pure JMAP client's server is whatever account the
person points it at), where every other app reaches services through the
Bus.

Accounts live in settingsd's `email` group and are edited in Prefs; their
passwords live only in the app's `secrets.conf.mix` (0600)
([decision](../../docs/docs/decisions/2026-10-16-email-accounts-in-settings.md)).
Run it with `email` (a second launch raises the first).

Test it on a build worker:

```
cargo test -p email -p jmap
```

`tests/view.rs` drives the real view over a fixture state through kittest,
with no network, and keeps offscreen snapshots in `tests/snapshots/`
(regenerate with `UPDATE_SNAPSHOTS=1` and look at them).
`tests/registry.rs` checks the verbs it describes are exactly the ones
`docs/docs/spec/bus/verbs.conf.mix` registers.
