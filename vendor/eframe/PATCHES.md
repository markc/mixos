# eframe 0.37.0 local patch

Base: crates.io `eframe` 0.37.0, archive SHA-256
`9c5642501a65ca657d05f00c643a5c14ff2346cb3818032bfe23d58e1f8e5f84`,
upstream revision `3c6eb8261c3293ad9c7ea1ed58487e038860ebef`.
Imported intact from the published archive. The archive omits the workspace
licences; LICENSE-MIT and LICENSE-APACHE are from that upstream revision.

- `native/winit_integration.rs`, `native/run.rs`, `native/wgpu_integration.rs`:
  overdue redraws with pending screenshot or PaintWhileHidden actions run a
  full UI pass offscreen. Other overdue redraws and glow remain logic-only.
  Known hidden windows also bypass surface acquisition during direct Full
  passes. Immediate child viewports inherit offscreen painting. Offscreen
  rendering does not run the first-present hook that makes a window visible.
- Remember stalled redraws until the outstanding callback arrives, avoiding a
  fresh 250 ms grace period for every requested offscreen step. No idle timer
  is added. Destroyed windows release this bookkeeping.
- Direct stalled passes retain a 250 ms repaint floor across later wakes;
  deduplicated redraw requests remain armed for compositor-driven recovery.
  Explicit pixel requests and their next 250 ms of logic/readback wakes bypass
  the floor for their bounded capture handshake. Ordinary logic never extends
  this grace, and neither the grace nor the floor schedules idle work.
- Glow marks offscreen capture unsupported in context data so toolkit can
  immediately refuse a known hidden window with an actionable message.
- Uses the matching egui-wgpu patch, without changing public eframe APIs.
- Preserve the root's explicitly requested visibility across first paint and
  logic-only passes. Wgpu paints an explicitly hidden root offscreen even
  when the native backend cannot report visibility. This lets chooser-only
  services create secondary viewports without mapping a main window.
  Explicit `Visible` commands still show or hide that owner. Native coverage:
  `apps/dopus/tests/chooser_bus_test.mix`; refresh guard below.

Guard: `cargo test -p toolkit --test offscreen_patch`; state-machine coverage
in toolkit's `drive::tests`. Renderer guard and headless GPU test are recorded
in `../egui-wgpu/PATCHES.md`. All Rust commands run on the build worker.

Drop this patch when upstream supports hidden-window painting without a
compositor frame callback or a surface acquire. Refresh deliberately and run
the guards before changing the pinned version.
