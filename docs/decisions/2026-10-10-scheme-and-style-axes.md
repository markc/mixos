---
title: Decision — scheme and style are separate axes
description: A theme is chosen on independent axes (scheme for the colours, style for the chrome's forms, mode, contrast), with window decorations and caption side beside them, so any palette can take any style.
---

# Decision — scheme and style are separate axes

Status: accepted 2026-10-10.

## The decision

- **Scheme and style are separate choices.** The scheme picks the colours:
  one of the nine palettes. The style picks the chrome's forms and lengths:
  `plain`, `pro`, `studio` or `classic`. A theme context has a scheme, a
  style, a mode and a contrast. A style of "none" means the scheme's own
  style, which keeps every existing selection exactly as it was.
- **Any palette takes any style.** A chrome style draws from the chrome
  colour roles. The `pro`, `studio` and `classic` palettes author those
  roles. Every other palette derives them from its semantic pairs, through
  a typed and documented mapping in the design (`families.chrome.derive`).
  For example, the bar is the `base` surface, cards the `secondary`
  surface, and selections and the menu highlight the `accent` pair. A
  colour a palette does author always wins over the derivation. In the
  other direction, the plain style draws any palette through its semantic
  pairs, which every palette has.
- **The contrast rules still hold.** The pairs keep their WCAG checks.
  Derived text roles sit on surfaces of the same pair, or close to them,
  and the design tests that text on derived chrome meets AA (4.5:1) in
  every hue palette, mode and contrast. The authored chrome palettes stay
  exact, as before.
- **Light or dark base comes from the palette.** Whether the chrome sits
  on a light or a dark base is read from the lightness of its own `chrome`
  colour, not from the style. A light palette in a style first designed
  for a dark one is still drawn light.
- **Decorations and caption side sit beside the axes.** They change no
  colour or length, so they are not part of the design context:
  - `csd`: the application draws its own title bar, as before;
  - `ssd`: the compositor decorates the window, and the application shows
    its menus and right-hand controls in a menu-bar row;
  - caption side `right` (the default) or `left`. On the left the order is
    Close, Minimize, Maximize, with Close in the corner, and the menus
    follow the captions.
- **Every surface takes the axes.** Settings has `appearance.style`,
  `appearance.decorations` and `appearance.caption_side`. The shared theme
  file has `style:`, `decorations:` and `caption_side:`. Each application's
  View → Theme menu has a Style group and a Decorations group. An app's own
  choice persists, and older saved state loads with the defaults.

## Why

Palette and style answer different questions. Tying them together meant a
colour preference forced a layout, and a layout preference forced colours.
As separate axes, each new palette works in every style and each new style
works with every palette.

## Consequences

- Every scheme, style, mode and contrast combination compiles; the design
  tests compile them all.
- A new palette needs no chrome colours to work in a chrome style. It can
  still author any of them where the derived colour is not the one wanted.
- The style family's binding from each scheme to its own style remains.
  It is what "the scheme's own style" resolves to.
- The style family no longer carries a base token. The light or dark base
  is the palette's, as described above.
