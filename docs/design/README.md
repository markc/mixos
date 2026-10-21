---
title: design
description: The MixOS design-token system, which compiles a strict-data theme source into resolved colours, metrics, typography and button tables for one scheme, mode and contrast.
---

# design

The MixOS design-token system. A design source is a strict-data document
(`theme.conf.mix`) holding OKLCH colour primitives, metrics, type scales,
semantic text pairs and the button family's mapping rules, plus modifier
blocks per scheme and mode. Breeze and Adwaita each provide light and dark
chrome palettes and a Studio-based style; scheme and style remain independent.
The append-only compatibility table supplies both palettes, styles and terminal
defaults to older sources with `defaulted-scheme` provenance. An authored
scheme binding retains its source colours. Missing bindings for the original
nine schemes remain errors.

Breeze keeps its surface colours, blue accent and selection opacity. Four
foreground/action values are adjusted to meet the contrast requirement after
egui's rounded, premultiplied sRGB blending. Text is checked on all seven
resting, hovered and pressed surfaces, including their accent-soft tints.
Primary-button ink is checked at 100%, 93% and 85% opacity on all five
resting surfaces. The minimum rendered ratios are:

| mode / role | reference | shipped | minimum contrast |
|---|---|---|---|
| light / `text_faint` | `#515A64` | `#4D5660` | 4.558:1 on tinted pressed |
| light / `primary_bg` | `#237AB4` | `#0F66A0` | 4.536:1 at 85% on field |
| dark / `text_faint` | `#A4AAB1` | `#B8BEC5` | 4.504:1 on tinted pressed |
| dark / `accent_text` | `#50B8EE` | `#61C9FF` | 4.543:1 on tinted pressed |

The reference inputs measured 4.283:1 for light faint text on tinted pressed,
3.601:1 and 3.789:1 for dark faint/accent text there, and 3.813:1 for the
light primary button at 85% on chrome. These adjustments preserve the 4.5:1
requirement rather than importing those failures.

The compiler flattens it for one
`DesignContext` (scheme, mode, contrast, optional app) into a resolved
design: gamut-mapped colours with WCAG contrast checked, resolved metrics
and typography, and a total button table keyed by variant, size,
interaction and focus. Every failure is a stable, source-addressed
diagnostic.

The crate is headless and has no renderer dependency. Its only
dependencies are [strict](../strict/README.md) (the source format) and
`serde`.

- `parse_design_source` / `parse_legacy_v0_source` read a source; the
  embedded default is `EMBEDDED_DEFAULT_SOURCE`.
- `compile_design` produces a `DesignCompileResult`; `apply_compiled_design`
  stamps a revision onto an accepted candidate.
- `Scheme`, `Mode`, `Contrast` are the closed selection axes;
  `active_typography` reads a typography role with embedded defaults.
- The default selection, used when nothing chooses a theme, is studio,
  dark. `DesignContext::revision_one()` is revision one's ocean, light,
  which the v0 equivalence gate and the revision-one parity tests pin.

Build and test with `cargo test -p design`.

The source is [`libs/design`](https://github.com/markc/mixos/tree/main/libs/design).
