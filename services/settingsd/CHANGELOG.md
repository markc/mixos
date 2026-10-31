# settingsd contract changes

## Unreleased

Store format 5 (key space v2, S6): the record holds the registry's sparse
`values`; writes go through `registry::apply` and resolve through
`resolve::values`, so `settings.apply`, `validate` and `reset` take v2 key
names (`window.style`, `appearance.density`, `accessibility.text_scale`,
`shell.panels.<panel>.autohide`, `reset: ["terminal.*"]`). A format-4
profile migrates on open after its seal verifies: the old record is kept as
`desktop.format4.conf.mix`, `migration.mx` records the digests and the
table's report, a `migrate-v2` receipt advances the revision. New commands:
`migrate [--dry-run] [--import-backdrop PATH]`, `downgrade --to 4 --target
BIN [--from-kept]`, `retire`, and `serve --authority | --mirror-of NODE`
(the mirror's sealed cache; its wire is S7). `settings.describe` is the
registry's catalogue, and `settings.status` reports `role`, `authority` and
`stale`. The published snapshot is unchanged in shape: its `desktop` is the
format-4 projection of the values until S7's schema 3. `design_revision`
advances for any key whose registry domains include paint, text or layout,
so changes to `terminal.*`, `email.*`, `ced.*`, density and
`accessibility.text_scale` now advance it too (format 4 counted only
`appearance` and `apps`); it may over-report, never under-report.

Contract 1.0.0 (key space v2 §5). Retained topics per prefix,
`settingsd.desktop.keys.<profile>.<prefix>` (22), plus
`settingsd.desktop.look.<profile>` and `settingsd.desktop.snapshot.<profile>`
(schema 3: values, compact designs). A commit publishes only the topics whose
content moved, each body stamped with `changed_at`; a broker (re)connect
republishes all. `settings.get` takes `view: values|look|snapshot`, with
`prefix` or `keys` for values; a live key is refused as `live_key`.
`settings.validate` adds the would-be values of the touched prefixes.

Mirror role (F1): `serve --mirror-of NODE` attaches to the authority with
`settings.mirror.attach` (answered with the record), takes each
`settings.mirror.revision` push into its sealed cache and republishes the same
topics on its own node. Writes are forwarded and the authority's receipt
relayed; with the authority unreachable they are refused as
`authority_unreachable` and `settings.status` reports `stale: true`. The
authority pushes each revision with bounded retry, drops a mirror after eight
failures (it re-attaches on its reconnect), keeps the attached nodes in
`mirrors.json` beside the store (a hint: an entry failing its first push is
dropped), and tells them when it stops. Pushes and stop notices are taken
only from the authority's broker-stamped settingsd, attaches only from the
node named, at most 16 mirrors, never an older incarnation. Known gap: a
crashed authority is noticed only at the next forwarded write (noded has no
peer-down event). `settings.source` serves the pinned source by digest (R14a).
`settings.palette` waits for the combined S2/S4b stack.

Transition until S8: the format-4 topics, `view: full|summary` (still the
default when `view` is omitted) and `settings.appearance.get` stay.

`settings.get` 0.7.0 includes `ui` in summary reads: text scale, density and
reduced motion on the same revision as appearance. Store format unchanged.

## 0.1.0

Before initial release: sealed profile format 4 re-derives the Workshop app
context and drops retired `apps.busviewer` overrides with a diagnostic after
seal verification, preserving unrelated overrides and history.

Serve describe/get/validate/apply/reset/status through native ABP; durable
changed/no-op receipts, target/revision fencing, exclusive writer, recoverable
replacement and complete scoped retained publication. Package the service unit.
Before initial release: add explicit idempotent first-run session seed, preserving
established missing-primary failure, and session wants/upholds wiring without
making GUI startup wait for authority readiness. Authority commands/schema unchanged.
First creation requires installer-only --allow-create; the automatic unit uses
plain seed and cannot recreate a wholly lost profile directory or mount.

Before initial release: pin package source and verify stored content digest;
bind storage to the held directory/lock; fence validation; add bounded pending
publication retries and preserve missing-store failure visibility.
Fence effective compiler interpretation; distinguish intact unsupported/I/O
failures from corruption; keep pending publication backoff capped at 30 seconds
until success. Inline capacity refusal has a distinct snapshot_too_large status.
