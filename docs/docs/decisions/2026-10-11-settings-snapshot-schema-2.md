# Settings snapshot schema 2 sends each design once

Status: accepted. Settings contract 0.3.0 (additive), snapshot schema 2 beside
schema 1. Follows [plain followers read a summary](2026-10-11-settings-summary-reads.md).

## Context

A snapshot carries one compiled design projection per effective context:
the desktop and each native app. With the embedded design every context
shares one projection, so a 499 KB schema 1 snapshot holds seven copies of
one 71 KB projection. Native consumers (the portal, the compositor's scene
host and applications on the shared `settings` consumer) need those
projections, receive the snapshot as a retained topic delivery and read it
again to confirm, so each of them moves about 1 MB per change.

Several of those consumers belong to applications still awaiting their port
to this tree's egui toolkit. They carry their own copy of the settings
library and must keep working, unchanged, until each is ported.

## Decision

1. **Schema 2 is a wire form.** `settings::compact::CompactSnapshot` holds
   each distinct projection once under `designs`, keyed by its digest; each
   context's `design` is a digest naming one. `compact::decode` expands
   schema 1 or 2 into the same in-memory `Snapshot`, so the consumer,
   reducer, cache and effective digests do not change. A context naming an
   unknown design, or a design no context names, is refused. Digests are
   opaque keys to the decoder.
2. **Asked for, never assumed.** `settings.get` (full view) and
   `settings.appearance.get` take `schema`: 1 is the default and is never
   sent; 2 answers in the compact form; any other is refused with
   `unsupported_schema`.
3. **A compact topic.** settingsd publishes the retained snapshot on three
   topics, in order: `settingsd.desktop.changed.<profile>` (schema 1),
   `settingsd.desktop.compact.<profile>` (schema 2), then the summary. A
   revision is published once all three are; a failure retries all three.
   noded reserves the compact topic for settingsd as it does the others.
4. **Ported consumers move now.** The shared native consumer
   (`settings::native`) follows the compact topic and reads schema 2, and
   the portal asks for schema 2. Measured with the embedded design, the
   compact snapshot is about 72 KB where schema 1 is 499 KB.
5. **Unported consumers move with their port.** An application or service
   still on its own copy of the settings library keeps following the schema
   1 topic and reading schema 1. Each moves when it is ported, by using the
   shared consumer. Nothing in an unported consumer changes for this
   decision.

## Retiring schema 1

The schema 1 topic and default stay while any consumer still uses them.
Publishing schema 1 costs a full snapshot per change, so once the last
consumer has moved, a later decision stops publishing it and makes schema 2
the default read. The library keeps decoding schema 1 for caches and old
retained state.

## Not chosen

Deltas between revisions, compression and local compilation are discussed
in the summary decision. Schema 2 removes duplication without changing what
a consumer holds; local compilation (no projection on the wire) remains the
next step once consumers can pin the design source by digest.
