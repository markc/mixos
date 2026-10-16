# egui-winit: local edits

Base: crates.io `egui-winit` 0.36.2 (MIT OR Apache-2.0), copied unchanged except
for the edit below.

## 1. Ctrl+V sends a paste even with no clipboard text

Upstream `on_keyboard_input` (src/lib.rs), on a paste chord, pushes
`Event::Paste` only when the clipboard holds non-empty text, then returns. With
an image-only clipboard (a screenshot) no event is sent at all, so the terminal
never writes `^V`, and Claude Code, which reads images from the clipboard on
Ctrl+V, never gets the signal.

This patch always pushes `Event::Paste`, with empty text when the clipboard has
no text. The terminal turns that into `^V`; text fields insert nothing.

Drop when upstream sends a paste (or a key event) for Ctrl+V with no text.
Report upstream: egui issue to be filed.

Guard: none in code. Checked by hand: an image on the clipboard, Ctrl+V in the
terminal, Claude Code shows the pasted image.
