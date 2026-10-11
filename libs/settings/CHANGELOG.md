# settings contract changes

## 0.4.0

Authority verbs 0.2.0 (additive), snapshot schema still 1
([decision](../../docs/decisions/2026-10-11-settings-summary-reads.md)):

- `Summary` (schema 1): binding, incarnation, revision, design revision,
  source digest, appearance names and `custom_source`; never projections or
  a custom source. `summary_topic(profile)` /
  `SUMMARY_TOPIC_PREFIX` name its retained topic,
  `settingsd.desktop.summary.<profile>`, published beside the snapshot topic.
- `ReadRequest.view` (`View::Full`, the default and never serialised, or
  `View::Summary`) selects `settings.get`'s answer.
- `follow::Follower` subscribes to the summary topic and reads the summary
  view. **Breaking:** `Followed::Look` gains `custom_source`, and its
  `appearance.source` is always `None`.
- `describe()` lists the read views and both topics.

The cache interpretation includes the library version, so caches written by
0.3.x fall back visibly once.

## 0.3.5

Add the appearance keys `appearance.style` (null, the default, takes the
scheme's own style; else plain, pro, studio or classic),
`appearance.decorations` (csd, the default, or ssd) and
`appearance.caption_side` (right, the default, or left). They resolve into
every effective context; a style change is a paint and layout change and a
decorations or caption-side change a layout change. Older desktops and
snapshots load with the defaults. Authority verbs/schema remain 0.1.0/1.

## 0.3.4

Expose the public snapshot identity of an activated cache capture so hosts can
report fenced persistence receipts without copying its projection or private
producer serial. Authority verbs/schema remain 0.1.0/1. The cache interpretation
includes the library version; older caches fall back visibly without affecting
authority history.

## 0.3.3

Add immutable cache targets for borrowed workers, capture deduplication and
producer-fenced writer opening. Add lazy persistent fallback loading after
retained resource validation, preserving load diagnostics when embedded works.
Authority verbs/schema remain 0.1.0/1. Older presentation caches are rejected
because the cache interpretation includes this library version.

## 0.3.2

Expose shared consumer evidence with separate accepted and applied identities,
connection generation, confirmation, presentation kind and faults. Readback
never claims frame presentation or broker participant registration. Authority
verbs and snapshot schema remain 0.1.0 and 1.

## 0.3.1

Split authenticated native delivery decoding from UI consumer handling, retaining
binding/generation fences in the decoded value. Add shared session binding and
capture identity predicates for coalesced resource scheduling. Authority wire
contract/schema remain 0.1.0/1. The cache interpretation includes this crate's
version, so older local presentation caches are rejected and embedded defaults
remain available; authority persistence is unaffected.

## 0.3.0

Add shared retained/cache/embedded fallback preparation with host resource
validation, immutable stage fencing and explicit presentation labels. Persisted
data never seeds authority ordering or mutation fences. Optional cache I/O
uses bounded no-follow directory-relative reads and atomic replacement, with
schema/target/capability/digest checks plus complete projection recompilation.
Applied-generation captures and one locked serial writer reject stale saves;
ambiguous post-rename failure remains visible. Exact JSON float round trips
preserve compiler projection comparison. The authority wire/schema stay
0.1.0/1. Real resource loading, GUI/first-map timing and artifact integration
remain pending.
Keep fallback preparation/activation faults separate from authority/current
resource faults, and detect unsupported cache headers before strict body parsing.

## 0.2.0

Add transport-neutral consumer and render-domain change plan, with an optional
native executor over an app-owned supervised connection. Bound work/buffering,
coalesce queue-loss recovery, fence connection/consumer/renderer completions,
and advance unchanged render evidence without a redraw. Retired confirmation
candidates are deduplicated within a bounded history. No owned runtime or
renderer dependency. Authority wire contract/schema remain 0.1.0/1.
Use canonical app/shell constructors, captured read baselines/tickets and one
absolute recovery deadline. Malformed-event storms cannot bypass the deadline;
same-incarnation rollback is distinct from a read racing a newer publication.
DTO additions require explicit render/evidence classification at compile time.
Persisted fallback cache, artifact/resource preparation and GUI activation are
still pending; this API does not advertise their completion.

## 0.1.0

Initial binding/revision/receipt/snapshot types, independent design read
projection, validated batch vocabulary and ordering/work-ticket reducer.
Renderer transport/bootstrap adapters remain pending.
Before initial release: report confirmed same-revision contradictions and
validate authored app enum values before accessibility precedence.
