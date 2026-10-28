# toolkit

The shared shell for MixOS egui applications: theme, fonts, Lucide icons, title bar, menus, chrome components, command registry, Fluent strings and Bus drive commands.

Documentation: <https://mixos.dev/toolkit/> — source [`docs/toolkit/README.md`](../../docs/toolkit/README.md).

`markdown::Preview<K>` keeps one parsed `Arc<Document>` by a caller's key
and `u64` revision. `document(key, revision, source)` calls the source
closure only on a cache miss; `show(ui, key, revision, source)` draws it
and returns the activated link. `cached()` borrows the last document for
views that prepare it before drawing; `clear()` forgets it. The cache is
plain data, with no egui state, and clones share the document.
`cached_for(&key, revision)` borrows a matching document so callers can skip
preparing their source text on a hit.
`first_heading_baseline(ui, document)` measures the opening heading with
the renderer's font and line height, for adjacent source panes to align
their first row without copying the heading style constants.

Shells call `markdown::open_links(ctx, app_name)` after drawing to take
egui's OpenUrl requests and hand allowed web and mail links to `xdg-open`
as one argument. Other output commands stay in place. The app name labels
errors and the thread that reaps the child (`<app_name>-xdg-open-reap`).

## Choosing files

`toolkit::choose(request)` returns a non-blocking handle. Build a validated
`choose::Request` with `choose::request(json)` using the
[`files.choose` request shape](../../docs/docs/spec/files/README.md).
The types and validation live in `files::chooser`, also used by DOpus;
the toolkit supplies translated defaults.

```rust,ignore
let request = toolkit::choose::request(r#"{
  "mode": "open", "multiple": true, "correlation": "compose-3"
}"#).unwrap();
let mut pending = toolkit::choose(request);
// In compose-3's frame, each frame until it answers:
if let Some(outcome) = pending.poll(ctx) {
    // Outcome::Chosen { paths: Vec<PathBuf>, choices, current_filter },
    // Outcome::Cancelled, or Outcome::Failed(reason).
}
```

The first `poll` binds the opening viewport and starts its worker. Results
are consumed once, only in that viewport; completions request a repaint of
that viewport. `viewport()` and `tier()` expose the handle's state. Keep the
handle alive until it answers; dropping it cancels the native request or
calls portal `Request.Close`. Paths retain exact OS bytes in `PathBuf`.
Set `correlation` to the app's own window key; native outcomes are routed
by the provider's request id, with `files.choose.result` recovering missed
publications. Nothing is cached between calls.

The tiers run sequentially: registered Bus `files`, the system chooser,
then `file_chooser::FileChooser` inside the calling viewport. Discovery,
calls and filesystem work run off the frame thread. Connection and discovery
operations have a two-second bound. A visible chooser has no user deadline;
a slow acknowledgement or lost connection is resolved or cancelled before
another tier starts. Refusals and failures advance; Cancel ends the request.

On Linux, the default `portal` feature calls FileChooser directly with
zbus, preserving all request fields and distinguishing Responses 0, 1 and
2. The Response subscription precedes the method call; any Response 2
advances to the local chooser. There is no GTK or zenity fallback. Disable
default features to omit this tier. Windows and macOS alone compile rfd
0.17.2 (MIT). Its dialog cannot return choices or the chosen filter, set an
accept label, express general glob/MIME filters, or distinguish failure
from Cancel: choices retain their defaults, `current_filter` is `None`,
simple extension filters are mapped, and both failure and Cancel end the
request. SaveFiles skips rfd and uses the local chooser.

The local chooser supports open (single/multiple files or folders), save,
SaveFiles, filter menus, choices, suggested names, exact byte names and
replacement confirmation. Folders remain visible through filters. Save
adds a lone `*.ext` filter's extension to a name without one. Legacy
`FileChooser::{new,single,save}` constructors and outcomes remain available
for callers being migrated.
