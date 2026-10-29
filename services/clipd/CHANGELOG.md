# Changelog

## 0.1.0 — 2026-10-28

- Keep active store blobs pinned and release cancelled fetch pins on completion;
  bound eager Wayland text probes, preserve byte sizes and rotation order,
  refuse password-marker blobs and return an error for argumentless clip_set.
- Run clipboard policy and private-broker service tests in the standard Mix gate
  using freshly built Mix and blobd binaries.

- Replace the unmerged Rust prototype with a Mix `--serve` service.
- Add headless storage, both selections, text history, indexed picking,
  configured mesh follow and server push/pull, lazy password reads and expiry.
- Use external wl-clipboard tools for graphical sessions. Lazy remote Wayland
  ownership is unavailable and refused; see the manual before live acceptance.
- Reuse Bus-only Mix builtins and native MCP clipboard tools.
- Resume pending Wayland blob ownership on blobd events; a newer copy or clear
  fences late arrivals independently for both selections.
