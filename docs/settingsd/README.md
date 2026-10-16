---
title: settingsd
description: The headless desktop settings authority, the one writer for a session profile's settings, with durable receipts and retained change publication on the Bus.
---

# settingsd

The headless native ABP desktop settings authority, contract 0.1.0. An
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

## Profiles and upgrades

Profiles pin their initial package design source. Reload checks a content
digest; a binary upgrade cannot change effective settings within the same
revision. An effective interpretation digest fences compiler drift for
explicit migration. The accepted record's format (`store::FORMAT`, now 2)
is that migration: an older record whose seal verifies as stored is
re-derived and re-sealed on open by `Accepted::upgrade`, whose doc comment
says how to add the next field.

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

Worker tests: `cargo test -p settingsd`. The native ABP gate and the
compositor fixtures arrive with the desktop test harness.
