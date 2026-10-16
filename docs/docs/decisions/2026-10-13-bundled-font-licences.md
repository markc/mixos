# Bundled fallback font licences

**Status: accepted, 2026-10-13.**

MixOS uses Inter and JetBrains Mono for its application typography and retains
egui's bundled fallback fonts for additional glyph and emoji coverage.
The pinned `epaint_default_fonts` 0.36.2 package declares
`(MIT OR Apache-2.0) AND OFL-1.1 AND Ubuntu-font-1.0`.

The dependency checker permits those licences only for that exact package
version. The general permissive code-dependency allowlist is unchanged.
Updating the font bundle version requires reviewing the exception and the
shipped licence files together. This does not change MixOS's own MIT OR
Apache-2.0 licence or replace its primary typefaces.

The bundled font notices and complete upstream licence texts are recorded in
`libs/toolkit/assets/fonts/fallbacks/`. Their hashes, the primary-font licence
texts and the Lucide licence are pinned in the toolkit's licence manifest.
`libs/toolkit/scripts/install_licenses.mix` verifies that manifest and installs
the notices under `share/licenses/toolkit/`. Desktop release archives and the
native image builder invoke this same installer.

The font licences permit embedding and redistribution subject to their
conditions, including preservation of copyright and licence notices and
restrictions on renaming modified fonts. We keep the upstream font files
unmodified. References: [SIL OFL 1.1](https://openfontlicense.org/open-font-license-official-text/)
and [Ubuntu Font Licence 1.0](https://canonical.com/legal/font-licence).

Lucide SVGs supply interface icons. Noto Emoji remains a text/emoji fallback;
removing Material Symbols does not remove emoji coverage.
