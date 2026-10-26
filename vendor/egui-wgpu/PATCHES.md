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
  texture uploads/frees and existing CaptureState. Paint-only hidden passes
  skip readback allocation and copying. It never acquires,
  reconfigures, recreates or presents a surface. A bounded native background
  device poll (one process-wide worker, eight queued requests, ten-second
  wait limit) completes screenshot callbacks without requiring another paint;
  browser callbacks retain wgpu's event-loop-driven progress.
- `src/capture.rs`: copy mapped readback bytes into host memory using SSE4.1
  streaming loads on aligned x86-64 mappings (runtime-detected, portable bulk
  copy fallback) before
  RGBA/BGRA conversion and unmap immediately. Scalar channel reads from
  uncached integrated-GPU mappings can outlive the capture deadline and
  leave readback workers consuming CPU after the request has been refused.
- `src/winit.rs` logs missing surface/render state and zero-size captures.
  ScreenshotCallback has no error completion and dropping it does not panic;
  callers retain their capture timeout as the failure backstop.
- The headless GPU guard lives in `tests/desktop/offscreen_gpu.rs`, included
  as a unit-test module for private painter access. It needs no extra dependency
  or vendored manifest/lockfile changes. It renders without a
  window or surface, checks padded row widths, resizing, scaled primitives,
  clear colour, RGBA/BGRA ordering, multiple callbacks and consumption of texture deltas,
  including a 3840 by 2160 capture.

Guards: `cargo test --manifest-path vendor/egui-wgpu/Cargo.toml --lib --features winit,wayland
offscreen_tests::` (requires a headless wgpu adapter; also checks aligned,
unaligned and partial-vector copies),
and `cargo test -p toolkit --test offscreen_patch` (re-vendor wiring).
All Rust commands run on the build worker. Drop alongside the eframe patch
once upstream provides the same surface-independent paint path.
