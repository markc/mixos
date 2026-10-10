---
title: Decision — a modern style as data
description: New palettes and a modern desktop style are added to the design as data, through general, defaulted style tokens that leave every existing look unchanged.
---

# Decision — a modern style as data

Status: accepted 2026-10-10.

## The decision

- **Two new chrome palettes, authored exactly.** `adwaita` (light and
  dark) and `solarized` (dark, with Studio Light as its light mode) join
  the chrome schemes. Their colour roles are authored at nine decimals,
  like the others. Adwaita's own style is Studio's grammar with 9 pt cards,
  menus and popups. Its light close button is grey, not red; the close
  button's colour is a role like any other, never assumed.
- **A modern style, `gnome`, selectable on any palette.** It has a 47 pt
  header bar, 34 pt rows and controls, pill buttons with generous padding,
  and 12 pt cards and windows. Surfaces are flat and separated by tone,
  with no outlines or bar rules. There are no bevels, and selection is an
  accent tint. Adwaita is its natural pairing. With a hue palette, its
  chrome colours derive from that palette's pairs, as for any chrome style.
- **New tokens are general and defaulted.** The look needed eight things
  the style family could not express. Each is a general token, not a
  component branch:
  - `item_spacing_x` and `item_spacing_y`;
  - `control_height`;
  - `button_padding_x` and `button_padding_y`;
  - `push_padding`;
  - `outlines`;
  - `bar_rules`.

  Each has a default equal to the value every existing style already drew.
  A style that leaves a token out keeps its look, and designs written
  before the token existed still compile. Components only read the tokens;
  none of them branches on a style.
- **A scheme left unbound takes `plain`.** A design written before a scheme
  existed has no binding for it. That scheme now takes the `plain` style
  instead of failing, so an earlier design still compiles in every scheme.

## Why

The point of style tokens as data is that a new look costs data, not code.
This style is the first test of that. Its few needs that were not yet
expressible became tokens that every style can use. They are defaulted, so
nothing that existed moved by a pixel.

## Consequences

- To add a token: give it a type, a span and a default that reproduces
  every existing style; then author it only in the styles that differ.
- A further modern style (for example macOS-, Windows- or Plasma-like) is a
  new token set, plus whichever general tokens it is the first to need.
