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

`native/run.rs` arms a one-shot 100 ms grace timer when it actually calls the
root window's `request_redraw()`, after repaint scheduling and visibility
throttling. Delayed requests arm only when their redraw is issued, including
when an earlier immediate redraw has already consumed its fallback. Windows
reported as invisible retain upstream's throttled direct logic path and do not
arm an additional fallback. A root redraw cancels it. If no redraw arrives, the timer
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
An app that keeps calling `request_repaint()` while hidden still runs logic at
roughly the 100 ms grace interval. Eframe installs no periodic timer of its own.
Invisible-root handling cancels the fallback before it is taken; nested redraw
checks are deferred until the outer check finishes, avoiding two logic passes
for the same viewport in one iteration.

Guards: `apps/workshop/tests/shell.rs` text-checks `run.rs`'s redraw-site
arming, both cancellation paths, deadline consumption and sleeping control
flow, the nested-check guard, and both renderers' `run_logic` dispatch to
logic-only execution. A pristine re-vendor fails these checks. It also includes
`native/logic_wake.rs` and
tests that queued `workshop.ping` and `HELP` deliveries answer through real
shell logic with no UI pass or paint. The included scheduler tests cover
one-shot expiry, cancellation on redraw, coalescing and delayed deadlines.
Run `cargo test -p workshop --test shell` and the normal toolkit/prefs suites.

Drop when upstream runs root logic independently of Wayland frame callbacks.
No upstream issue has been filed by this change.

Standalone Clippy on the pinned toolchain additionally flags upstream's
`std::io::Cursor` in `IconDataExt::to_png_bytes`. A method-scoped lint
expectation preserves that unrelated encoder implementation unchanged.

For standalone upstream tests, the normalised `Cargo.toml` adds an empty
`[workspace]` and mirrors the enclosing workspace's egui-winit, dirs-sys,
calloop and input patches. This does not affect dependency resolution when
eframe is built as an application dependency. The toolkit additionally
settles paint-dependent drive input only after `INPUT_QUIET_PERIOD` (1000 ms)
without a UI pass, including when Wayland still reports the window as visible.
Brief animations or GPU stalls preserve the input; a window producing no
frames releases later queries after about one second. The cancellation message
reports the elapsed milliseconds without frames, rather than advising restore.
The quiet period starts when input first waits and restarts on each root UI
pass, so an idle window still gets the full grace period for newly queued input.

Wgpu's `initialized_all_windows` creates missing viewport windows, their winit
input states and GPU surfaces; it is not a gate for app logic. `resumed` calls
`init_run_state`, which installs the root window and its input state before
returning its first repaint request, even if the root has been hidden since
creation. `run_logic` is called only for that root's existing window ID. It
deliberately skips the all-window scan to avoid creating unrelated deferred
viewport windows or GPU surfaces during a logic-only wake. Normal UI and window
events retain the scan.
