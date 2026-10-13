# Shipped scene templates

Canonical, read-only template directories live here:

```
<template>/scene.mix
<template>/template.conf.mix
<template>/behaviour.mix       (optional executable citizen)
```

`scene.mix` is an AMP scene document, not an executable script.
`template.conf.mix` is a strict-data map. Only `behaviour.mix` executes.
Reusable behaviour helpers are shipped in `lib/` (`data.mix`, `models.mix`,
`runtime.mix`, `taskbar.mix`, and `network.mix`, the network popup's snapshot).

Templates: `panel` (bottom panel, page `scene-panel`), `launcher`, `calendar`,
`notes`, `network` and `settings`. The loader copies a template into the user's scenes directory
only on `scenes.install`; it never enables discovered directories
automatically. Installed under their own names these reuse the legacy panel
citizen's page IDs, so the loader keeps them unmounted while `quoin-panel` is
registered and mounts them when it stops. Install under a distinct name (for
example `{template:"launcher",name:"preview-launcher"}`) to try one beside the
running panel. See [the loader manual](../../docs/cos/scenes-loader.md) and
[quoin-panel](../../docs/cos/quoin-panel.md).

The panel's buttons are icon-only; names and details are in their tooltips
(`lib/models.mix`). A task's tooltip also shows a live thumbnail of its
window, which compd draws into the card while it is up.

`settings/` is Settings/Appearance. It authors the scene `quoin-settings` on
Quoin's declared page `settings.appearance`, so `{template:"settings"}`
installs under that name and keeps the page. Its behaviour does not own any
state: it reads `shell.settings.get`, wakes on `shell.settings.changed` and
forwards clicks to Quoin's `shell.settings.*` verbs. Quoin also builds its
built-in fallback page from this `scene.mix` at compile time, so edit the
node block here and nowhere else. The template mounts on the right edge; edit
`window.edge` if your configuration declares the page elsewhere.

## Gallery metadata

`template.conf.mix` may carry these optional keys, which the
[Scene Editor](../../docs/scene-editor.md) gallery reads through
`scenes.templates`:

| Key | Type | Meaning |
| --- | --- | --- |
| `title` | string | gallery title (defaults to the scene name) |
| `description` | string, at most 200 characters | the one-line gallery text |
| `recommended` | bool | part of the Recommended set |
| `order` | int | gallery order (then template name) |
| `requires` | list of Bus service names | shown as "needs …" while not registered |
| `hidden` | bool | not listed by `scenes.templates` |

The six shipped templates are the Recommended set: `panel` (order 10,
requires `comp` and `apps`), `launcher` (20, `apps`), `calendar` (30),
`notes` (40, `notify`), `network` (45) and `settings` (50).

`network` shows links and addresses from rtnetlink, plus systemd-networkd,
systemd-resolved and the default routes when their tools answer
(`networkctl`, `resolvectl`, `ip -j`). A missing daemon degrades to the
kernel's view. Where hostd runs and grants this panel's scene loader,
a link can be configured (DHCP or static) through hostd's `net.link.apply`,
which keeps a change only if the link comes up, and the last change undone
(`lib/hostd.mix`, `docs/hostd.md`). Those handlers act only on clicks the
scene host relays, never on a Bus caller.

## The editor helpers and legacy template

The normal Scene Editor is the Wayland app in `apps/scene-editor`. Its menus
use the loader's `scenes.editor.snapshot/action` verbs, which reuse the pure
model and action planner in `editor/lib.mix`.

`editor/` also preserves the legacy compositor frontend, selected explicitly
with `SCENES_EDITOR_APP=0`. It has `hidden:true`, so the gallery never lists
it. Its `scene.mix` header is a dialog:
`{"kind":"dialog","w":880,"h":620,"title":"Scene Editor","chrome":true}`.
If a host cannot show dialogs, change it to
`{"kind":"edge","edge":"right","w":480,"title":"Scene Editor"}`; the loader
opens that as an edge popup. Its helpers (the model builder, click planning
and the ced diagnostics diff) live in `editor/lib.mix`, beside the
behaviour, which loads them with `require($SCENE_DIR .. "/lib.mix")`. A user
copy therefore edits its own logic. From the shared `lib/` it uses only
`runtime.mix`.

The loader runs this directory in place as safe mode and never writes to it.
`scenes.install {template:"editor"}` makes the editable user copy; see the
Scene Editor manual for how the two are chosen. No other template may use
`window.kind:"dialog"` or the page `scene-editor`.

## Reloading a scene

Three separate things can change, and each reaches the running panel by a
different route.

| What changed | Where it lives at runtime | How it reaches the panel |
| --- | --- | --- |
| Shared helpers (`lib/data.mix`, `lib/models.mix`, …) | `$SCENES_LIB`, which defaults to `$SCENES_TEMPLATES/lib` | Copy into the catalogue, then reload the scene that uses it |
| A scene's own `scene.mix`, `behaviour.mix` or `template.conf.mix` | The user copy under `$SCENES_DIR/<name>/` | Reload re-reads the user copy. A template edit needs `reset` or a fresh install first |
| Loader code (`services/compd/scripts/`) | The compd scenes service | Restart that service; reload does not run new loader code |

The environment fixes the paths. On the VT4 session (`/etc/mixos/session/4.env`):

- `SCENES_DIR=/var/lib/mixos/vt4/scenes` holds the user copies. The panel is
  `panel/`.
- `SCENES_TEMPLATES=/opt/mixos/share/scenes` is the catalogue. Its `lib/` is
  the one the panel's `behaviour.mix` loads (`$LIB`) when it starts.

Steps for a change to shared helpers, such as `lib/data.mix`:

1. Copy the repo's catalogue over the installed one:
   `mix share/scenes/install.mix /opt/mixos/share/scenes`.
   The default destination is `$XDG_DATA_HOME/mixos/scenes`, which is not the
   VT4 catalogue, so pass the path. On a host where `lib/models.mix` is
   root-owned, the copy needs root.
2. Reload the panel:
   `mix services/compd/scripts/scenes_cli.mix reload panel`.
   The CLI sends `scenes.reload`, which re-checks `behaviour.mix`, re-renders
   `scene.mix` and remounts the scene. Use `--target scenes.<node>.bus` for
   another node. The CLI needs that session's D-Bus address.
3. Check the result: `scenes_cli.mix list --json` should show no diagnostic
   for `panel`, and the panel should show the new output.

A forced reload re-runs the behaviour, so it should pick up the new `lib/`
files. That is the expected behaviour from reading `reconcile_one`
(`services/compd/scripts/lib/scenes.mix`). It has not been confirmed on a
live panel. If the old output persists after step 2, restart the compd
scenes service and check again. The unit's name is not recorded in this
repo, so look it up on the host. `scenes.restart` is different: it is the
loader's internal crash restart for a single scene, not a manual restart.

Reset is for template changes to a scene's own files. `scenes.reset {name}`
restores the template defaults and keeps a recovery copy, so it discards the
user's edits to that scene. Use it on purpose.
