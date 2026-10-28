# Changelog

## 0.1.0 — 2026-10-28

- Replace the unmerged Rust prototype with a Mix `--serve` service.
- Add headless storage, both selections, text history, indexed picking,
  configured mesh follow and server push/pull, lazy password reads and expiry.
- Use external wl-clipboard tools for graphical sessions. Lazy remote Wayland
  ownership is unavailable and refused; see the manual before live acceptance.
- Reuse Bus-only Mix builtins and native MCP clipboard tools.
