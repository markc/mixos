---
title: settings — changelog
description: Release notes of the settings component, mirrored from libs/settings/CHANGELOG.md.
generated: by docs/_build/gen-site.mix from libs/settings/CHANGELOG.md; do not edit
---

# settings contract changes

> Mirrored from [`libs/settings/CHANGELOG.md`](https://github.com/markc/mixos/blob/main/libs/settings/CHANGELOG.md), where releases update it. The component's page is [settings](README.md). This page describes the current `main` branch of MixOS.

## Unreleased

Title band names add `appearance.title_align`, `appearance.actions_align` and
`appearance.actions_place`. Menu and caption positions accept centre. Caption
placement now defaults to `auto`: right in CSD and the style's own side in SSD.
Unknown names are retained, and defaults remain omitted from full and compact
data, so old seals still verify. Explicit right remains right. The appearance
portal keeps its independent effective right default for older readers.

`appearance.menu_style` and `appearance.menu_side` select the session's menu
form and button position. Their design registries supply names and Prefs labels.
Unknown plain names survive round-trips and render as the defaults, `bar` and
`left`. Defaults are omitted from authored and effective bytes, preserving
existing seals without a store format or wire schema change.

`appearance.window_style` carries the server-side furniture name in authored,
effective, summary and compact snapshot data. The design registry supplies
known names and Prefs labels; unknown plain names survive round-trips and
render as the default. The default is omitted from both authored and effective
bytes, preserving existing seals. Store and wire schemas are unchanged.

Appearance projection schema 2 carries the effective desktop `caption_side`
for portald's GTK settings export and session cache. Schema 1 remains readable
with a default of `right`. `settings.appearance.get` keeps its original schema 1
wire view, so older strict readers still work. Portald enriches that view from
the caption placement already in the same atomic snapshot, with no extra read.
Store format unchanged.

`Summary.ui` carries the session's text scale, density and reduced motion on
the summary's revision. Store format unchanged; `settings.get` is 0.7.0.

Additive, store format unchanged: the mail options `email.mail.quote_on_reply`
(boolean, default true) and `email.mail.reply_position` (`"above"` or `"below"`,
default `"above"`), which the email app's Reply now reads instead of fixed
values. Omitted at their defaults, so existing profiles are unchanged.

## 0.8.0

Additive, store format unchanged (3):

- `Desktop.email` (`email::Email`): the email app's JMAP accounts
  (`email.accounts`, replaced whole, in tree order: id, name, url, email,
  ca_path, enabled, signature), `email.default_account` and the mail options
  `email.mail.page_size` (10–500, default 100), `mark_read_on_open` and
  `confirm_delete` (both default true). Omitted at its defaults, so older
  profiles keep their seals; it does not touch `Effective`. A profile that
  sets any of it will not load in a 0.7.0 build: reset `email` first.
- Nothing in it is secret: an account has no password field and every
  record refuses unknown keys, so `settings.apply` refuses an account that
  carries `password`.
- `Summary.email` and `Followed::Look.email` carry it to followers;
  `email::from_snapshot`, `email::from_summary` and `Email::from_account`
  read it; `email::mint_account_id` mints a new unique `acct-` id.
- `settings.describe` lists each `email.*` path; `email` alone resets or
  replaces the whole group.

## 0.7.0

Additive, store format unchanged (3):

- `Desktop.terminal` (`terminal::Terminal`): the term's preferences under
  `terminal.*` (font family/size/weight, padding, scrollback lines, scroll
  speed, border width, cursor shape/blink, focus ring/strength, copy on
  select, `TERM`). Every default reproduces the term before the group
  existed except padding (8 px instead of 1 px) and the focus ring: its
  default, `accent` at strength 0.25, draws the focused pane's border as the
  card border moved a quarter toward the theme accent, where the term drew
  it halfway before. That change is intentional (a subtler tinted ring);
  `neutral` remains available, and `accent` with strength 0.5
  gives the old look. Omitted at its defaults (and each value within it), so older
  profiles keep their seals. It does not touch `Effective`, as `keymaps`
  does not. A profile that sets any of it will not load in a 0.6.0 build:
  reset `terminal` first.
- `Summary.terminal` and `Followed::Look.terminal` carry it to followers.
- `settings.describe` lists each `terminal.*` field with its range and
  default; `terminal` alone resets or replaces the whole group.
- `terminal: null` resets the whole terminal group, as advertised.
- Correct the scroll speed unit: rows per wheel line, scaled by speed/40;
  touchpad travel counts in cell heights at that scale. Default 40 keeps
  the existing one-row-per-wheel-line behaviour; it is not a pixel distance.

## 0.6.0

Additive, store format 3 ([decision](https://github.com/markc/mixos/blob/main/docs/docs/decisions/2026-10-14-session-icon-stroke-and-type.md)):

- `Appearance` and `Effective` gain four optional session choices:
  `icon_stroke` (1.0 to 3.0 in quarter steps), `font_family`,
  `font_weight` (100 to 900 in steps of 100) and `mono_family`. Each is
  omitted when unset, so profiles without them keep their digests.
- `ChangePlan`: `icon_stroke` is a paint change; the font choices are
  resource and text changes.
- `CompactEffective` carries the same fields.
- The store writes format 3. A format-2 build refuses a profile that sets
  any of them; reset them first with `settings.reset` or a null value.

Rust API change: `Followed::Look.appearance` is now `Box<Appearance>` to keep
follower events compact as font settings grow. Borrowed callers use deref
coercion; callers that take ownership unwrap with `*appearance`.

## 0.5.0

Authority commands 0.3.0 (additive): snapshot schema 2
([decision](https://github.com/markc/mixos/blob/main/docs/docs/decisions/2026-10-11-settings-snapshot-schema-2.md)).

- `compact`: `CompactSnapshot` (schema 2) sends each distinct design
  projection once under `designs`, keyed by digest; each context's `design`
  names one. `encode` builds it; `decode` reads schema 1 or 2 into the same
  in-memory `Snapshot`, so consumers, reducer, cache and effective digests
  are unchanged. A dangling or unnamed design is refused.
- `compact_topic(profile)` / `COMPACT_TOPIC_PREFIX`: the retained schema 2
  topic, `settingsd.desktop.compact.<profile>`, published between the
  schema 1 topic and the summary.
- **Breaking:** `ReadRequest.schema` (1 by default and never serialised;
  2 for the compact form) on `settings.get` and `settings.appearance.get`.
  Another schema is refused with `unsupported_schema`.
- `native` follows the compact topic and reads schema 2. Consumers on the
  schema 1 topic are not affected.
- `describe()` lists the snapshot schemas and the compact topic.

## 0.4.0

Authority commands 0.2.0 (additive), snapshot schema still 1
([decision](https://github.com/markc/mixos/blob/main/docs/docs/decisions/2026-10-11-settings-summary-reads.md)):

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
snapshots load with the defaults. Authority commands/schema remain 0.1.0/1.

## 0.3.4

Expose the public snapshot identity of an activated cache capture so hosts can
report fenced persistence receipts without copying its projection or private
producer serial. Authority commands/schema remain 0.1.0/1. The cache interpretation
includes the library version; older caches fall back visibly without affecting
authority history.

## 0.3.3

Add immutable cache targets for borrowed workers, capture deduplication and
producer-fenced writer opening. Add lazy persistent fallback loading after
retained resource validation, preserving load diagnostics when embedded works.
Authority commands/schema remain 0.1.0/1. Older presentation caches are rejected
because the cache interpretation includes this library version.

## 0.3.2

Expose shared consumer evidence with separate accepted and applied identities,
connection generation, confirmation, presentation kind and faults. Readback
never claims frame presentation or broker participant registration. Authority
commands and snapshot schema remain 0.1.0 and 1.

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
