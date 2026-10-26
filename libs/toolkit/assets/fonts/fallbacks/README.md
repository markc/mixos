# Bundled fallback font notices

The application fonts remain Inter and JetBrains Mono. The unmodified
`epaint_default_fonts` 0.37.0 dependency embeds these additional fallback
fonts. Its original licence texts are copied here byte for byte:

| Font | Copyright notice | Licence text |
|---|---|---|
| Hack Regular | Copyright 2018 Source Foundry Authors; Bitstream Vera Sans Mono Copyright 2003 Bitstream Inc.; DejaVu contributions are public domain | `LICENSE-Hack.txt` (MIT and Bitstream Vera terms, including reserved font names) |
| Noto Emoji Regular | Copyright 2013 Google Inc. All Rights Reserved. | `LICENSE-NotoEmoji.txt` (SIL OFL 1.1) |
| Ubuntu Light | Copyright 2011 Canonical Ltd. Licensed under the Ubuntu Font Licence 1.0 | `LICENSE-Ubuntu.txt` |
| Emoji Icon Font | Copyright (c) 2014 John Slegers | `LICENSE-EmojiIcon.txt` (MIT) |

The Ubuntu and Noto notices above are also carried in the embedded upstream
font metadata. This directory adds an accessible copy alongside the complete
licence texts. No font files are modified or renamed.

Source: the published `epaint_default_fonts` 0.37.0 crate, `fonts/` directory.
`libs/toolkit/assets/licenses.conf.mix` pins the copied text hashes. The native
installer ships this README and those texts under `share/licenses/toolkit/`.

The bundled egui-icons subset carries five private-use glyphs from Emoji Icon
Font, under the same John Slegers MIT licence. The explicit monochrome emoji
feature retains Noto Emoji and the full Emoji Icon Font as fallbacks.
