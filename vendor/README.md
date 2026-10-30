# vendor/

Patched upstream crates the workspace builds against. Each directory is kept
intact as imported, with its own licence files, which is why the root
`Cargo.toml` lists `vendor` under `[workspace] exclude`. Workspace crates reach
them through `[patch.crates-io]`.

Each crate's local edits against its upstream base are listed in that crate's
`PATCHES.md`, with the guard that fails if a re-vendor drops an edit.

| Crate | Dir | Upstream version | Wired via | Local edits | Notes |
|---|---|---|---|---|---|
| eframe | `eframe/` | crates.io 0.37.0 | `[patch.crates-io]` | Pending paint actions use a full offscreen pass after a stalled redraw; viewport-scoped Wayland window-menu requests; see `eframe/PATCHES.md` | MIT OR Apache-2.0; guards: toolkit's `offscreen_patch`, `window_menu_patch` and drive state-machine tests |
| egui-wgpu | `egui-wgpu/` | crates.io 0.37.0 | `[patch.crates-io]` | Surface-independent capture and viewport-scoped final rounded-frame pass; see `egui-wgpu/PATCHES.md` | MIT OR Apache-2.0; guards: `offscreen_tests`, toolkit's `window_frame_patch` |
| mail-parser | `mail-parser/` | crates.io 0.11.5 | `[patch.crates-io]` | 64-level embedded-message nesting cap; see `mail-parser/PATCHES.md` | Apache-2.0 OR MIT; guard: maild's `parser_nesting` integration suite |
| pipewire | `pipewire/` | crates.io 0.10.1 | `[patch.crates-io]` | `Buffer::find_meta_mut`, so portald can stamp each frame's PTS; see `pipewire/PATCHES.md` | MIT; guard: the compile-time `MetaHeader` size assert in portald |
| msedit (Microsoft Edit buffer) | `msedit/` | revision `826b4c097b6f14ba0a846dc56f2f0223a3aaf73a` | `#[path]` include from `libs/edit` | see `msedit/PATCHES.md` | MIT; its `lsh` and `stdext` workspaces are excluded |
| smithay | `smithay/` | 0.7.0, git master `7ddcd1736b47` (2026-08-17), recovered by diff; see `smithay/PATCHES.md` | `[workspace.dependencies]` path | **yes**: compd's hooks (filtered seat global, drag start, selection source, pre-request interposer) and local fixes, +1,574/−200 in 28 `src` files, each with a guard | MIT; guards run by `mix tools/smithay_guards.mix`; its own `[patch.crates-io]` mirrors the root's so a standalone test build resolves the same `input` and `calloop`; pins crates.io winit `=0.31.0-beta.2` (the apps use 0.30.x) |
| calloop | `calloop/` | crates.io 0.14.5, SHA-256 `43b32313…`; see `calloop/PATCHES.md` | `[patch.crates-io]` | **yes**: `src/sources/channel.rs` +57/−2, a bounded channel batch re-pings only after servicing its own token (stock 0.14.5 lets two composed channels wake each other forever) | MIT; guard `sources::channel::tests::composed_channels_do_not_ping_each_other_for_unrelated_tokens` (`cargo test --manifest-path vendor/calloop/Cargo.toml composed_channels`); applies to the whole graph, apps included |
| input (input-rs) | `input/` | crates.io 0.10.0, SHA-256 `f9793345…`; see `input/PATCHES.md` | `[patch.crates-io]` | **yes**: +15 lines, the tablet-pad DIAL event routed and parsed (`libinput_1_26`) | MIT; guard `event::tablet_pad::compd_dial_patch_guard`; drop when upstream routes DIAL |
| teletypewriter (rio) | `teletypewriter/` | rio git `932c1a7d9e07b4db5924f7a0dd689e823c3a1442`, package 0.5.27; see `teletypewriter/PATCHES.md` | `[patch."https://github.com/raphamorim/rio"]` | **yes**: child-only sealed session-descriptor handoff between fork and exec | MIT; guard `teletypewriter_patch_guard` in term-core's unit tests |
| dirs-sys | `dirs-sys/` | crates.io 0.5.0, `8bcd4aa2c35990d57a2cff2953793525fc42709c` | `[patch.crates-io]` | replace one OptionExt comparison with standard Option equality; remove option-ext | MIT OR Apache-2.0; upstream XDG tests and licence gate |

egui-winit uses crates.io 0.37.0. Upstream PR 8631 supplies the
one-shot 250 ms stalled-redraw logic pass; PR 8472 supplies image-only
clipboard paste events, handled by Term as the terminal's Ctrl+V.
