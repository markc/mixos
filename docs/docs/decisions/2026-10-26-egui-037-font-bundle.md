# egui 0.37 fallback font bundle

**Status: accepted, 2026-10-26.**

The workspace uses the crates.io egui family at 0.37.0, retaining explicit
`default_fonts` and `monochrome_emoji_fonts`. Inter and JetBrains Mono remain
the primary application fonts. The system font provider is disabled.

The exact-package font licence exception in
[the fallback font decision](2026-10-13-bundled-font-licences.md) now applies
to `epaint_default_fonts` 0.37.0. The general dependency allowlist is unchanged.
Its four existing font files and licence texts are byte-identical to 0.36.2.
The new `egui-icons.ttf` is upstream's five-glyph subset of Emoji Icon Font,
under the same John Slegers MIT licence already shipped in the toolkit's
fallback notices. MixOS does not modify these fonts. The licence installer
continues shipping the complete notices and licence texts.

The monochrome emoji feature preserves the previous bundled glyph coverage
without depending on a system font provider. Upstream's `fonts/egui-icons.txt`
records the subset's source and glyphs.
