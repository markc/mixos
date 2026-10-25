# eframe: local edits

Base: crates.io `eframe` 0.36.2, upstream egui revision
`49682f8baa058bf49e011035cfbd6e825f88a5ef` (`crates/eframe`).
MIT OR Apache-2.0. The published crate omits the workspace licence files;
`LICENSE-MIT` and `LICENSE-APACHE` are copied from that exact upstream revision.
Its workspace `clippy.toml` is also copied unchanged from that revision: the
crate's published manifest enables lints that depend on the upstream MSRV,
type-complexity threshold, exported-API policy and documentation word list.

## 1. One-shot logic wake when a redraw stalls

On Wayland, winit 0.30 withholds `RedrawRequested` while a surface's frame
callback is outstanding. A window on another virtual desktop may never receive
that callback, without `is_visible` or `is_minimized` reporting it as hidden.
The existing eframe invisible-window handling does not cover that case.

`native/run.rs` arms a one-shot 100 ms grace timer for each accepted root
repaint request. A root redraw cancels it. If no redraw arrives, the timer
runs `App::logic` through the existing `update_logic_only` path, without a UI
pass or GPU submission. Both native renderers implement the internal
`WinitApp::run_logic` hook by selecting that existing path. Wgpu skips surface
setup and screenshot handling on this path. Input remains pending for the next
real UI pass, as in upstream's hidden-window logic path.

The fallback consumes its deadline before calling app logic. It does not
poll, request another redraw, or rearm itself. An explicit new repaint request
can arm another fallback; repeated requests coalesce without postponing the
first deadline. Visible windows retain compositor frame pacing; an idle hidden
window returns to `ControlFlow::Wait`. This is not a way to capture screenshots
or execute widget input while the compositor refuses to render the window.

Guards: `apps/workshop/tests/shell.rs` includes `native/logic_wake.rs` and
tests that queued `workshop.ping` and `HELP` deliveries answer through real
shell logic with no UI pass or paint. The included scheduler tests cover
one-shot expiry, cancellation on redraw, coalescing and delayed requests.
Run `cargo test -p workshop --test shell` and the normal toolkit/prefs suites.

Drop when upstream runs root logic independently of Wayland frame callbacks.
No upstream issue has been filed by this change.

For standalone upstream tests, the normalised `Cargo.toml` adds an empty
`[workspace]` and mirrors the enclosing workspace's egui-winit, dirs-sys,
calloop and input patches. This does not affect dependency resolution when
eframe is built as an application dependency. The toolkit additionally
settles paint-dependent drive input during a logic-only wake, including when
Wayland still reports the window as visible, so it cannot block later queries.
