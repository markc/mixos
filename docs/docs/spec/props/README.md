---
title: Service property contract
description: Structured property commands, capabilities, replay and live topics, errors and SQLite table ownership.
---

# Service property contract

Status: accepted, version 0.2.1. This records the existing structured
namespace wire surface implemented by `props`' optional store and router.
The [ownership decision](../../decisions/2026-10-18-property-store-in-props.md)
places it in `libs/props`. Commands belong to the service exposing them and
must be registered under that owner in the [Bus registry](../bus/commands.conf.mix).

## Addressing

Commands are `<svc>.props.<suffix>`. The `namespace` header names a
registered unqualified namespace; its capability name is `<svc>.<namespace>`.
Collection records require a non-empty `key`. Singleton requests omit `key`
or send an empty key; replies use the schema's canonical key.

`if_version` is an optional decimal u64 fence, required when the namespace
declares `require_version`; zero fences creation. Record `version` and
namespace `nseq` increase on commits. `cause` is an optional correlation
string on mutations. Actor identity comes from the authenticated peer,
never a caller-supplied actor header. `_lifecycle` is reserved.

This surface requires structured addressing. `set` refuses `path` with
`flat_path_mutation_deferred`; other commands refuse it as mixed addressing.
The compositor's flat `{path,value,generation}` mutation remains its own
contract. `field_path` is refused on set/delete/get, even when empty.
Get refuses any non-empty body. List refuses `cursor`, `limit`, `order_by`,
`filter`, `fields` and any non-empty body. These refusals carry diagnostic
prefixes `field_path_deferred`, `get_projection_deferred` or
`list_option_deferred` under `validation_error`.

## Commands

| Suffix | Headers/body and successful reply |
| --- | --- |
| `set` | JSON object body; `merge=true`/`patch` (default) or `false`/`replace`. Simple reply: `{namespace,key,version,nseq,created,warnings?}`. Saga reply: `{namespace,key,set_version,set_nseq,complete_version,complete_nseq,lifecycle,created,reason?}`. |
| `delete` | Collection only; optional `if_version`, `cause`. Reply: `{namespace,key,deleted:true,tombstone_version,nseq}`. |
| `get` | `{namespace,key,version,nseq,fields}`; `nseq` is the observed namespace cursor. |
| `list` | `{namespace,nseq,records:[{key,version,fields}],next_cursor:null}`. |
| `describe` | `view=public` (default) or `full`; `{namespace,schema_version,cardinality,fields,primary_key_field?}`. Fields carry name, type, secret flag, default, validators, help and optional since/until. |
| `watch` | Optional decimal u64 `since_nseq` (default zero); grant live delivery then return `{namespace,observed_nseq,events,live:true,caught_up:{event_type:"caught_up",namespace,nseq}}`. |
| `audit.watch` | Same cursor and grant sequence; audit rows in `events`, with no `caught_up` field. |

Get/list/describe are queries; set/delete/watch/audit.watch are intents.
Watch commands install an observable broker subscription. Replay-only
`live:false` is available exclusively in the library's unit tests.

## Capabilities and redaction

Policies resolve `PeerIdentity` into exact capability strings:
`props.read:<svc>.<ns>`, `props.write:<svc>.<ns>`,
`props.describe:<svc>.<ns>:public`, `props.describe:<svc>.<ns>:full`,
`props.audit:<svc>.<ns>` and `props.read:<svc>.<ns>:secrets`.
Set/delete require write, get/list/watch require read, and audit.watch
requires audit independently of read. Describe requires the selected view's
capability; `schema_public=deny` refuses public describe.

Get/list replace fields declared `secret:true` with null unless the caller
also has the explicit secrets capability. Stored values are projected
verbatim without filling missing defaults, preserving audit reproducibility.
Public describe omits secret fields and redacts secret validator names.
Passive event topics never carry record values or secret field names.
Audit keys remain in the owner's private storage and never enter the Bus.

## Watch and topics

The reserved topics are `<svc>.props.records.changed` and `<svc>.props.audit`.
Only the owner publishes; direct `topic.subscribe` is refused, including
for the owner. After its capability check, the service calls
`noded.props.subscribe_grant` with non-empty `topic`, `target_peer` and
`namespace` headers. The broker authenticates the topic owner, requires a
connected registered target, and installs a top-level namespace body filter.
Success returns `{subscription_id,namespace}` with rc 0; refusal uses rc 10
(including `target_peer_not_connected`). The registered command's owner is
`noded`, class `intent`, version 0.1.0.

The grant completes **before** the observed cursor is captured. Replay
contains `since_nseq < nseq <= observed_nseq`; later events are eligible
for live delivery. The `caught_up.nseq` is always the observed cursor,
including when the caller supplied a higher cursor. Process replay, then
the marker, and discard overlapping live events through that cursor;
deduplicate by `(namespace,key,nseq)`.

Records-changed bodies contain `namespace,key,kind,command,nseq,version,
audit_epoch,actor,ts` and optional `fields_changed`,
`secret_fields_changed_count`, `cause`, plus `lifecycle,reason?` on completed
events. Kinds are `created`, `updated`, `deleted`, `completed`, `reconciled`.
Audit bodies contain `namespace,key,command,version,nseq,audit_epoch,actor,at,
audit_digest` and optional `fields_changed`; they omit kind, lifecycle,
cause and secret count. Timestamps use RFC 3339 UTC with milliseconds.
Records-changed bodies omit the digest. Both topics use `retain:false`.

Replay and live publication share the same projection functions. Each
dispatcher starts at the current cursor, preserving older history for
replay. Publication is best-effort: errors are logged and the cursor still
advances; recover through watch. A missing granter returns `unavailable`;
grant failure returns `grant_failed` without leaking upstream routing detail.

## Errors

Failure bodies contain `{error_code,message}`; the Bus error field carries
the human message. Success is rc 0. The closed vocabulary is:

| rc | `error_code` |
| --- | --- |
| 10 | `auth_denied`, `not_found`, `validation_error`, `conflict`, `version_mismatch`, `replay_window_exceeded`, `grant_failed` |
| 20 | `storage_error`, `hook_error`, `unavailable` |

Version mismatches add `expected_version,current_version`; replay-window
errors add `since_nseq,oldest_nseq`. Validation-hook rejection maps to
`validation_error`; an operational hook failure maps to `hook_error`.

## Storage

`SqliteStore` owns exactly five table names and their frozen DDL:
`__props_meta` (namespace nseq and audit epoch), `__props_records` (versions,
lifecycle and tombstones), `__props_events` (durable replay/audit history),
`__props_audit_keys` (per-namespace private keys) and `__props_values`
(the optional default JSON mapping). The schema-compatibility test pins
all five definitions from `sqlite_master.sql`. Existing databases open
without migration. Domain mappings own their business tables and must
participate in the substrate's single `BEGIN IMMEDIATE` transaction.

Hard delete removes metadata and restarts a recreated record at version 1;
it cannot require version fencing. Soft delete retains the tombstone
version and recreates against that version. Tombstones do not yet expire.
Saga writes commit provisioning before completion to active or failed.
Reconciliation alone increases audit epoch. Each event digest is lowercase
hex HMAC-SHA256 over canonical sorted-key JSON plus its nseq as eight
big-endian bytes, using a private 32-byte namespace key; deletes hash null.
There is no previous-digest chain. The in-memory backend is for tests and
in-process services and implements hard deletion only.

## Logging

The opt-in `logging/props` bridge registers singleton `log`, canonical key
`current`. Its fields are `level` (`none,error,warn,info,debug`, default info),
`filter` (EnvFilter string, default empty), `format` (`human,json`, default
human), `applied_at` and `source` (strings, default empty), and `applied_by`
(`cli,env,props,default`, default default). All remain mutable. Hooks reject
invalid enums/filter syntax and all deletes using prefixes
`log_unknown_enum_variant:`, `log_filter_invalid:` and `log_undeletable:`.
The watcher applies level/filter, using `off` for none; it neither rebuilds
sinks nor writes metadata back. Its private Notify is separate from the
dispatcher signal. Registration/read failures map to `LogError::Props`.
