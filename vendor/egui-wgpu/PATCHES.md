# egui-wgpu 0.37.0 local patch

Base: crates.io `egui-wgpu` 0.37.0, archive SHA-256
`0309cbe1febed48af5b974e4bcc424b35e9664863a4d402697147b57a488c864`,
upstream revision `7c0b40402271e8d40cda69f87cf3c2617c2e4382`.
Imported intact from the published archive. The archive omits the workspace
licences; LICENSE-MIT and LICENSE-APACHE are retained from egui's matching
0.37.0 eframe publication revision (identical workspace licence texts).

- `src/winit.rs`: add `Painter::paint_offscreen_and_update_textures`, sharing
  the normal painting implementation. It uses the viewport's configured
  size, renderer target format, scale, clear colour, depth/MSAA attachments,
  texture uploads/frees and existing CaptureState. It never acquires,
  reconfigures, recreates or presents a surface. A bounded native background
  device poll completes screenshot callbacks without requiring another paint;
  browser callbacks retain wgpu's event-loop-driven progress.
- `Cargo.toml`, `Cargo.lock`: add pollster as a dev dependency for the headless GPU guard.
  `offscreen_tests::hidden_window_capture_without_surface` renders without a
  window or surface, checks padded row widths, resizing, scaled primitives,
  clear colour, RGBA/BGRA ordering, multiple callbacks and consumption of texture deltas.

Guards: `cargo test --manifest-path vendor/egui-wgpu/Cargo.toml --lib --features winit,wayland
hidden_window_capture_without_surface` (requires a headless wgpu adapter),
and `cargo test -p toolkit --test offscreen_patch` (re-vendor wiring).
All Rust commands run on the build worker. Drop alongside the eframe patch
once upstream provides the same surface-independent paint path.
