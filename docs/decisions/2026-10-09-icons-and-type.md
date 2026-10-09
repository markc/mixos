---
title: Decision — icons and type
description: MixOS applications use Lucide icons and the Inter and JetBrains Mono typefaces, with type weight and icon stroke as design tokens so a lighter profile is a design change.
---

# Decision — icons and type

Status: accepted 2026-10-09.

## The decision

- **Icons are Lucide.** Every icon in MixOS applications and the desktop
  comes from the [Lucide](https://lucide.dev) set: 24 px grid, round caps and
  joins, a single stroke colour. Icons are taken unmodified from a pinned
  upstream release and ship with Lucide's licence (ISC, with MIT for the
  icons derived from Feather) and a `NOTICE` line. A component ships only the
  icons it uses.
- **Type is Inter and JetBrains Mono.** Interface text is Inter; numbers,
  code and JSON are JetBrains Mono. Both are SIL OFL 1.1 and ship as
  unmodified static faces from their upstream releases.
- **The shipped desktop sizes** are Inter Regular at 12.5 px for body text,
  10.5 px for small text, Inter SemiBold at 15 px for headings, Inter Medium
  for emphasis, and JetBrains Mono at 12 px.
- **Weight is a design token.** Each typography role names a weight, and the
  renderer selects the static face of that weight (300 Light, 400 Regular,
  500 Medium, 600 SemiBold). A light profile, with body text at 300, is a
  change to the design's weights and nothing else.
- **Icon stroke is a design token.** The design metric `icon.stroke_width`
  sets the stroke every icon is drawn with (default 2, Lucide's own), so a
  lighter icon set accompanies a lighter type profile without new assets.
- **Colour comes from the theme.** Icons take the colour of the text around
  them; no icon carries a colour of its own.

## Why

One icon family and one type family give every MixOS application, and the
external applications that run alongside them, the same look. Making weight
and stroke tokens keeps the visual weight of the whole desktop adjustable in
one place.

## Consequences

- `libs/toolkit` embeds the faces and icons and applies them from the theme.
- A new icon is added by copying its SVG from the pinned Lucide release into
  the component that uses it and listing it there.
