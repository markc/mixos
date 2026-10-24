---
title: Decision — session icon stroke and type
description: The desktop session can choose its icon stroke and its interface and monospace font families and weight, applied to every MixOS app through the settings profile.
---

# Decision — session icon stroke and type

Status: accepted 2026-10-14. Extends
[icons and type](2026-10-09-icons-and-type.md) and the
[keymaps in settings](2026-10-11-keymaps-in-settings.md) precedent.

## The decision

- **Four optional session choices** join the profile's appearance:
  `appearance.icon_stroke` (1.0 to 3.0 in quarter steps, on Lucide's 24-unit
  grid), `appearance.font_family`, `appearance.font_weight` (100 to 900 in
  steps of 100) and `appearance.mono_family`.
- **None means the design's own value**: the `icon.stroke_width` token (the
  approved 1.5 baseline), Inter, the scheme's own weight, and JetBrains Mono.
- **The stroke baseline stays 1.5.** It is the approved baseline (hub plan
  `2026-10-13-lucide-panel-icons`), and the shipped token matches it.
- **A user weight replaces the weight locked by the chrome schemes.** Body
  text takes the chosen weight; headings keep their design offset above it.
- **System fonts are runtime data.** A chosen family is read from the host's
  font files when it is available. Nothing is embedded, so the bundled-font
  licence obligations in
  [bundled font licences](2026-10-13-bundled-font-licences.md) are unchanged.
  A missing or still-scanning family falls back to the bundled Inter, which
  stays behind any system font as glyph fallback.
- **Carried by the existing carrier.** `settings.apply` takes the new keys,
  `prefs.appearance.set` is the Preferences front command, and every app follows the
  summary topic as it does for the scheme. No new command.

## Why

The icon stroke and the typeface are the two things a person most often
wants to tune, and they are session-wide, so they belong in the profile and
not in each app. Making them optional keeps every existing profile's digest
valid, so nothing is migrated.

## Digest and rollback

- The new `Appearance` fields are omitted when unset, like `style`,
  `decorations` and `caption_side`. A profile that sets none of them
  re-serialises byte for byte.
- The store is format 3. A profile that sets any of the four keys will not
  load in a format-2 build, because `Appearance` denies unknown fields. Roll
  back by resetting the keys first, with `settings.reset` or a null value.

## Scope

- **Apps following the session look** (busviewer, ced, dopus, screenshot)
  pick the choices up through `session_look` and `toolkit::install`, with no
  edits beyond the overlay.
- **Preferences** builds its own theme from its draft; it is updated alongside.
- **backdrop** draws textures only, with no text or icon widgets, and
  installs once at startup. It is out of scope.
- **compd's title bars** use their own theme builder and an embedded-only
  font database. They follow in a later slice, once the contract is proven
  nested; the `Effective` and `ChangePlan` fields are already in place.
- **Profiles that name their own design source** receive no session look,
  so they keep their design's own values. The choices are still saved.

## Consequences

- `libs/settings` 0.6.0 and store format 3. The change is additive for every
  profile that does not set a choice.
- `libs/toolkit` gains a theme overlay applied by `session_look`, a stroke
  precedence (overlay, then token, then `DEFAULT_STROKE` 1.5), a weight
  override that beats the chrome lock, and a background scan of the host's
  font files.
