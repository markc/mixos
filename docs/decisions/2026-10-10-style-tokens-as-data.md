---
title: Decision — style tokens as data
description: The forms and lengths of application chrome move out of renderer code into a typed style family in the design, so a new style is new data.
---

# Decision — style tokens as data

Status: accepted 2026-10-10.

## The decision

- **A typed `style` family in the design.** The chrome family says which
  colours the chrome is. The style family says which forms it takes and
  how big they are. Every choice a renderer used to make by asking which
  scheme it was drawing is now one token:
  - forms, such as a tab strip or cards for panel groups, a checkbox or a
    switch for on/off controls, bevels or outlines, a ringed, plain or block
    slider knob, and pill or rounded push buttons;
  - lengths, such as the title-bar height, the menu row padding, the bar
    and dock sizes, the push-button height and the corner radii;
  - the renderer base: pair-based or chrome widgets, and a light, dark or
    mode-following base palette.
- **Tokens are typed and closed.** Each token is a named choice, a flag or a
  number with a declared range. Every style must author every token. A
  missing, unknown or ill-typed token is a compile error, as is a scheme with
  no style or one bound to a style that does not exist.
- **Styles are named and bound per scheme.** `styles` holds named token
  sets, and `schemes` gives every scheme exactly one of them. The shipped
  design has four styles: `pro`, `studio` and `classic` for the chrome
  schemes, and `plain` for the six hue schemes. `plain` writes down, as data,
  the geometry those schemes used to derive from their pair-based style. A
  style does not vary by mode or contrast, and modifier blocks cannot alter
  it.
- **Renderers read the resolved style, never the scheme.** The egui toolkit
  builds its chrome from the compiled style. No component branches on a
  scheme any more. A design that authors no style family takes the embedded
  design's style for its scheme.

## Why

The chrome styles should be values a designer can edit and compare, not
branches a programmer has to find. With the style family, a new style is a
new token set in the design, with no renderer change. Moving every decision
across changed no pixel: the toolkit's and the applications' snapshots
render the same images before and after.

## Consequences

- This lays the ground for a separate style axis, where any palette can be
  combined with any style. Today the binding from scheme to style is fixed
  in the design; a later decision can make the style its own selection.
- `Scheme::is_chrome_scheme()` now only groups schemes, for example in theme
  menus. The style's `widgets` token decides which styling path applies.
- A form the token set cannot express yet, such as a new tab shape, still
  needs code once: a new choice for the token and the code that draws it.
  After that, any style can select it.
