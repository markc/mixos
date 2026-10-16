---
title: Decision — the chrome family and chrome schemes
description: The design gains a typed chrome family of exact, non-derived colour roles, and three chrome schemes whose light and dark modes step the interface brightness.
---

# Decision — the chrome family and chrome schemes

Status: accepted 2026-10-09.

## The decision

- **A typed `chrome` family in the design.** Application chrome (title bars,
  menus, tabs, panels and controls) is described by one closed set of colour
  roles, such as `chrome`, `card`, `field`, `hover`, `text_dim`, `accent` and
  `caption_close`. Each role names a colour primitive.
- **Chrome colours are exact.** No contrast adjustment, recipe or derivation
  applies to a chrome role: it renders exactly as authored. Colours are
  written as OKLCH at nine decimals, which round-trips every 8-bit sRGB value
  (the design crate tests the full cube).
- **Coverage is explicit.** A design that authors the chrome family must
  author every role, and an unknown role or primitive is a compile error.
- **Three chrome schemes**, `pro`, `studio` and `classic`, join the six hue
  schemes. Their light and dark modes step the interface brightness:

  | scheme | dark | light |
  |---|---|---|
  | `pro` | Pro | Pro Medium Gray |
  | `studio` | Studio | Studio Light |
  | `classic` | Classic | Classic |

- **Semantic pairs keep their guarantees.** In the chrome schemes the semantic
  pairs (`base`, `card`, `popover` and the rest) take the neutral `mono`
  palette for the mode, so every contrast rule still holds for components that
  read pairs. The egui toolkit draws the chrome schemes from the chrome family.

## Why

A desktop whose applications sit side by side should look like one desktop,
down to the pixel. Exact chrome roles make that a matter of values, not
adjustments, while the semantic pairs keep their accessibility guarantees.

## Consequences

- `Scheme::is_chrome_scheme()` tells consumers which styling path applies.
- Tests that pinned the six revision-one schemes iterate `Scheme::REVISION_ONE`;
  the chrome schemes carry their own exact-value tests.
- Components that read semantic pairs, such as the compositor's own panels,
  move to the chrome family over time.
