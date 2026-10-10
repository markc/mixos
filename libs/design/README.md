# design

The MixOS design-token system. A design source is a strict-data document
(`theme.conf.mix`) holding OKLCH colour primitives, metrics, type scales,
semantic text pairs and the button family's mapping rules, plus modifier
blocks per scheme and mode. The compiler flattens it for one
`DesignContext` (scheme, mode, contrast, optional app) into a resolved
design: gamut-mapped colours with WCAG contrast checked, resolved metrics
and typography, and a total button table keyed by variant, size,
interaction and focus. Every failure is a stable, source-addressed
diagnostic.

The crate is headless and has no renderer dependency. Its only
dependencies are `strict` (the source format) and `serde`.

- `parse_design_source` / `parse_legacy_v0_source` read a source; the
  embedded default is `EMBEDDED_DEFAULT_SOURCE`.
- `compile_design` produces a `DesignCompileResult`; `apply_compiled_design`
  stamps a revision onto an accepted candidate.
- `Scheme`, `Mode`, `Contrast` are the closed selection axes;
  `active_typography` reads a typography role with embedded defaults.
- The default selection, used when nothing chooses a theme, is studio,
  dark. `DesignContext::revision_one()` is revision one's ocean, light, which
  the v0 equivalence gate and the revision-one parity tests pin.

Build and test with `cargo test -p design`.
