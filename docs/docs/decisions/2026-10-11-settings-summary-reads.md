# Plain settings followers read a summary, not the snapshot

Status: accepted. Settings contract 0.2.0 (additive). Builds on
[Desktop settings have one native ABP authority](2026-10-07-settings-authority.md).

## Context

The settings snapshot carries the profile's complete effective design: one
compiled projection per context (the desktop and each native app). Measured
on a running session with the embedded design source:

- a `settings.get` reply is 499 KB of JSON, 498 KB of it `effective`: seven
  byte-identical 71 KB projections, 62 KB of each a button table;
- settingsd publishes that whole snapshot as the retained payload of
  `settingsd.desktop.changed.<profile>`;
- an application following the session look on a plain Bus connection
  (`settings::follow::Follower`) cannot verify a delivery's publisher, so it
  treats the delivery as a hint and reads `settings.get` again. It uses only
  the profile's identity, revision and appearance names.

One appearance change therefore moved about 0.5 MB to every plain follower
twice (the delivery and the re-read), and each parsed it. Idle traffic is
zero; the cost is per change. Native consumers (the portal, the compositor's
scene host and applications using the shared `settings` consumer) do use the
projections, from owner-stamped deliveries.

## Decision

1. **A summary topic.** settingsd also publishes a retained
   `settingsd.desktop.summary.<profile>` whose payload is a
   `settings::Summary`: schema, binding, incarnation, revision, design
   revision, source digest, the appearance names and `custom_source`. It
   never carries the projections or a custom design source. noded reserves
   it for settingsd exactly as it reserves the snapshot topic: only the
   registered `settingsd` service may publish or clear it, and inner routing
   headers are canonicalised. A revision counts as published once both
   topics are; a failure retries both.
2. **A summary read.** `settings.get` takes an optional `view`: `full` (the
   default, the complete snapshot as before) or `summary` (the same
   `Summary` under `summary`, with the usual status fields). An unknown view
   is refused like any unknown field. A request without `view` reads exactly
   as it did under 0.1.0, and the field is never sent at its default.
3. **Plain followers move to the summary.** The `Follower` subscribes to
   the summary topic and reads the summary view; Prefs does the same. A
   follower still treats a delivery only as a hint: this decision does not
   change how deliveries are trusted.
4. **Native consumers are unchanged.** The snapshot topic and the full view
   keep their schema and behaviour for every consumer that uses projections.

Measured on the same session after the change, one appearance change sends
a plain follower a 566-byte summary delivery and a 587-byte summary read,
where it had a 542 KB delivery and a 542 KB read. The remaining large
transfers belong to native consumers: the snapshot publish, the portal's
`settings.appearance.get` and the compositor scene host's full read, each
about 542 KB. They are the subject of the deduplication step below.

## Rollout

The summary topic is new, so the order matters: noded (the reservation),
then settingsd (publishes and serves the summary), then the followers. A
follower that reaches a settingsd without the summary view is refused
(`unknown field`) and keeps its last look, and it hears no hints from a
settingsd that does not publish the summary topic. Roll back in the reverse
order.

## Considered and deferred

- **Deduplicating `effective` by content digest** (one copy per distinct
  projection, contexts naming a digest) would make the full snapshot about
  72 KB. It changes snapshot schema 1, which native consumers decode and the
  optional cache stores, so it needs a schema 2 negotiated per request and
  the consumer, reducer and cache moving together. It is the next step for
  native consumers.
- **Compiling projections locally from the summary.** A projection is a
  function of the pinned source and the selected axes, and native consumers
  already link the design library: they could compile instead of
  transferring, at a few milliseconds per context. It needs the source
  pinned by digest and a compiler-version check, which the cache already
  records.
- **Immutable artifacts fetched by digest** remain the planned path for
  settings too large to inline (the 960 KiB snapshot ceiling).
- **Deltas between revisions** are not worth it: a scheme change rewrites
  most colour values in a projection, so a JSON delta is nearly the size of
  the projection. A summary is already smaller than any delta of it.
- **Compression** under the frozen ABP framing would mean base64 bodies;
  WebSocket permessage-deflate is not available in the Bus's WebSocket
  library and its window is too small for repeated 71 KB projections.
- **Trusting summary deliveries** (dropping the re-read) needs the
  publisher stamp noded already records to reach plain clients. It is a
  separate change to the Bus client API.
