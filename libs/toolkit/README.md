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
