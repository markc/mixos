---
title: settingsd
description: The headless desktop settings authority, the one writer for a session profile's settings, with durable receipts and retained change publication on the Bus.
---

# settingsd

The headless native ABP desktop settings authority, contract 1.0.0. An
established profile has one writer and durable whole-document settings and
operation receipts. Native publication is retained and owner-stamped by
[noded](../noded/README.md). [Ced](../ced/README.md) and
[compd](../compd/README.md)'s scene-host consume shared appearance jobs on
their existing Bus lifetimes. Full desktop integration, panel geometry and
native frame acceptance remain in development.

See [the settings contract](../docs/spec/settings/README.md). Clients use
the [settings](../settings/README.md) library. The source is
[`services/settingsd`](https://github.com/markc/mixos/tree/main/services/settingsd).

## Running it

```text
settingsd init --instance example
settingsd seed --allow-create --instance example
settingsd serve --instance example
```

Init explicitly initialises new state. An installer can explicitly seed a
first-run session profile with `--allow-create`. Plain seed validates an
existing profile unchanged and refuses a wholly missing root; a retained
writer lock prevents seed from recreating an established missing primary,
including without a backup. Serve fails for a missing or unsupported
established store. Both accept `--profile` and `--root` for an explicit
binding or test directory. Use an isolated broker when testing.

## Topics and mirrors

Contract 1.0.0 publishes one retained topic per settings prefix,
`settingsd.desktop.keys.<profile>.<prefix>` (`appearance`, `window`,
`shell.panels`, `time` and so on, 22 in all), plus the follower view
`settingsd.desktop.look.<profile>` and the schema 3 snapshot
`settingsd.desktop.snapshot.<profile>`. A prefix body carries every key under
it with its defaults filled, the resolved mode and accent beside the authored
choice for `appearance`, and `changed_at`, the revision that last changed it.
A commit republishes only the topics whose content moved; a broker reconnect
republishes all of them. Live keys (network, Bluetooth, system, audio levels)
are in no topic: their owners publish them.

Retained topics are node-local, so a machine has one authority and every
desk on it runs a mirror:

```text
settingsd serve --instance alpha                     # the authority
settingsd serve --instance alpha --mirror-of alpha   # a desk's mirror
```

A mirror attaches with `settings.mirror.attach`, keeps the authority's
record in a sealed cache, and republishes the same topics on its own node
from each `settings.mirror.revision` push. It answers reads from the cache,
forwards writes and relays the authority's receipts, and refuses writes as
`authority_unreachable` while the authority is down; `settings.status`
then reports `stale: true`. Nothing polls: the authority pushes each
revision (with a bounded retry, dropping a mirror that keeps failing), a
mirror re-attaches when its own broker reconnects, and a restarted authority
pushes to the mirrors it had attached (`mirrors.json` beside the store is a
hint: an entry that fails its first push is dropped).

The mirror commands keep their integrity checks on the open Bus. A push or
stop notice is taken only from the configured authority's node as the
broker identifies the sender (its `broker_peer` stamp, or the peer bridge
the message came in on), and from its settingsd wherever the broker stamps
the service. Because noded has no remote-service identity on every mesh
path yet, the mirror also confirms each push with the authority itself: it
takes the record only when the authority's own `settings.status` reports
that incarnation, revision and `mirror_digest`, and ignores a stop notice
while the authority still answers. An attach is taken only from the node it
names, at most 16 mirrors attach, and a mirror never takes an older
incarnation.

One gap is known: a crashed authority sends no notice, and noded has no
peer-down event a service can hear (topics are node-local and the mesh link
state is internal). A mirror then reports current until its next forwarded
write is refused, which marks it stale; the same holds when the authority's
broker goes away. Only a clean stop is seen at once.

`settings.source {binding, digest}` answers the pinned design source when
`digest` is the profile's current `source_digest`, and `stale_digest` with
the current one otherwise (theming ADR R14a).

## Profiles and upgrades

Profiles pin their initial package design source. Reload checks a content
digest; a binary upgrade cannot change effective settings within the same
revision. An effective interpretation digest fences compiler drift for
explicit migration. The accepted record's format (`store::FORMAT`, now 4)
is that migration: an older record whose seal verifies as stored is
re-derived and re-sealed on open by `Accepted::upgrade`, whose doc comment
says how to add the next field.

Format 4 re-derives the renamed Workshop app context. After verifying the
stored seal, it drops any retired `apps.busviewer` override with a warning,
advances the profile revision when dropping authored settings, and atomically
re-seals both the primary and any valid backup. Other app overrides, the
pinned design source and receipt history are preserved.

An embedded default can outlive the compiler rules that accepted it. If an
accepted desktop has no authored `appearance.source`, fails resolution, and
its pinned source digest exactly matches a known retired package default,
the upgrade re-pins it to this binary's embedded default. It keeps authored
appearance choices, advances both profile and design revisions, retains
receipts, and atomically re-seals each valid primary and backup. The retired
embedded tokens disappear with the old package source; there is no separate
authored token override map. A failing authored source or unknown embedded
source still refuses with the compiler's token/rule diagnostics, without
rewriting the store. Valid older sources stay pinned.

The known defaults are the earlier installed source from `740c0c13`
(`33a33291c676de5bd1c30cb395534e8656244f57e6771a98bd61e3669e11f235`)
and v0.1.2 at `fe9dfa90`, last changed by `bc2faa69`
(`34726165cf21927a700a0eb8e5770f6a374d44a7cc2f2fc09d987faf2a288ce2`).
As of 2026-10-24, published release history contains only v0.1.2; v0.1.1 is an unpublished
draft and there is no v0.1.0 tag. No other shipped default is inferred from
unreleased source edits. Init continues to pin the actual source so accepted
revisions remain reproducible; following the running binary by reference
would change settings without a revision or receipt fence.

Present corrupt primary data can restore a validated backup under a new
incarnation. A missing primary fails visibly, including when a backup
exists. I/O faults and intact unsupported documents fail without automatic
rollback.

## The unit

The packaged unit binds the instance to its machine hostname; its
session/VT number is a supervision attribute and never a second instance
identity. The session target wants and upholds the authority. The unit
validates with plain seed before serving, and application and compositor
startup does not wait for settings readiness. An image must provision its
service account's writable `MIXOS_ETC/settings` root and explicitly seed
once during first installation. Automatic startup never receives
`--allow-create`, so lost mounts or directories do not silently become new
default profiles.

## Tests

Worker tests: `cargo test -p settingsd`. The broker tests (`tests/bus.rs`:
change-only topics on one broker, then an authority and a mirror on a
private broker pair) run with `MIXOS_TEST_NODED` naming a noded built from
the same commit and `--ignored`. The compositor fixtures arrive with the
desktop test harness.
