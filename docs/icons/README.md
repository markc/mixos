# Icons

The `icons` library owns the pinned Lucide 1.54.0 SVGs and their names. It has
no egui dependency. `Icon::from_name`, `Icon::name`, `Icon::ALL` and
`Icon::svg` let a renderer load the same source by name. Toolkit re-exports
`Icon` and keeps its `bytes://lucide/` loader and design stroke token.

`Bar` contains a revision and a list of `BarItem` values: command, icon name,
tooltip, enabled and toggled. `Bar::valid` bounds incoming bars and rejects
unknown icons, invalid command names and duplicate commands. `standard`
identifies the retained macros, search, keyboard and theme tail.

The compositor reads registered apps' `ui.titlebar.form` through its existing
scene-host port, after registration and on trusted titlebar change notifications.
It rasterises the shared SVGs with resvg and caches the
result by icon, physical size, tint and stroke. See the
[title band decision](../docs/decisions/2026-10-27-title-band.md).

SVGs retain the upstream ISC and Feather-derived MIT notices in
`libs/icons/assets/LICENSE-lucide.txt` and the repository's `NOTICE`.
