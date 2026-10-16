---
title: Edit service
description: The edit service, which holds shared text buffers that people and agents edit together, with revision checks, per-origin undo, anchors, atomic save, external-change detection and recovery.
---

# Edit service

`editd` registers the `edit` service and serves the `edit.v1` contract.
[Ced](../ced/README.md) and agents share its buffers through native ABP.
The service is headless and requires no compositor or D-Bus session.

`edit.open`, `edit.list` and `edit.get` discover buffers and page their
contents. `edit.insert`, `edit.delete`, `edit.replace` and `edit.apply` change
text. `edit.save`, `edit.reload` and `edit.close` manage disk state.
`edit.undo`, `edit.redo` and `edit.history` expose history; selection, cursor
and anchor verbs retain positions across edits. Properties have get, list,
describe and watch ports. Ask `HELP` for the complete manifest.

Mutations can carry origin, operation identity and expected revision.
Buffer identities include a daemon epoch, so a restarted service does not
silently reinterpret an old identifier. Paged snapshots remain tied to their
captured revision while later edits arrive. Slow work on one buffer does not
block operations on another.

## Buffers and recovery

The buffer model is the [edit](../edit/README.md) library, shared with Ced:
revision CAS, per-origin undo, anchors, atomic save, external-change
detection and a recoverable change feed. This service owns its actors,
filesystem watching and the recovery writer.

Discovery uses the shared node configuration. Recovery defaults beneath
`MIXOS_VAR/edit/recovery`, following the MixOS Var directory rule.
`MIXOS_EDIT_RECOVERY_DIR` selects an absolute recovery directory, and
`MIXOS_EDIT_RECOVERY=0` disables durable recovery. The session supplies its
own state root.

## Tests

The retained integration tests exercise dispatch, recovery and the actual
service over a private native noded, rather than a substitute control
socket.

The source is
[`services/editd`](https://github.com/markc/mixos/tree/main/services/editd).
