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
  shape, accounts cache, schema-1 migration), `drafts` (unsent messages
  kept locally), `view` (tree, panes, dialogs).

Nothing typed is lost. A compose is saved as a local draft within a
second of each edit and on every close path, one file per draft in the
app's state directory (`<state>/drafts/<id>.json`, 0600, written
atomically); drafts are not settings and never reach settingsd. Cancel,
Escape or closing the window over a draft with something in it asks
first, and the default keeps it. A draft is removed only once its message
is sent or the person discards it; one kept from an earlier run is
offered at start.

The composer body uses the shared editor in process, with Markdown source,
soft wrap and the editor's undo and redo. It has no preview. Its document
is the source of truth; the wire body stays empty until sending, saving a
draft or reporting state. Cloning a compose keeps the text and drops undo
history. Registry shortcuts wait while the body has keyboard focus.
The body stays monospace: the shared editor lays text on a cell grid.
`email.composer.font.family` accepts only the bundled JetBrains Mono face
or the default (also JetBrains Mono), the same limit as `ced.editor`.
Prose hides the current-line band and shows the Body hint while empty.

The reader keeps its Markdown/plain choice per email. Its parsed body is
cached by `toolkit::markdown::Preview`, using the email id as the key and
the body hash as the revision. The shell hands reader links to
`toolkit::markdown::open_links`; only web and mail links open.

Sending creates the message in Drafts, submits it, and moves it to Sent
only once the server confirms the submission, so a failed send never
shows in Sent. A retry reuses the same server copy (its ids are kept with
the draft); after a lost reply it first asks the server
(`EmailSubmission/query`) whether the earlier submission landed, and never
submits twice.

The one architectural exception it owns: the app speaks HTTPS itself from
its effects runtime (a pure JMAP client's server is whatever account the
person points it at), where every other app reaches services through the
Bus.

Accounts live in settingsd's `email` group and are edited in Preferences; their
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
