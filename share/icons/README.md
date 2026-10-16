# Lucide icon bundle

`share/icons` is the self-contained Lucide icon bundle that MixOS scenes and the
panel use in place of the Material controls. It holds:

- `manifest.conf.mix`: the finite catalogue, strict data loaded with `load_data`
  and never executed. Schema 1, family `lucide`, release `1.54.0`, stroke width
  `1.5` on the 24-unit canvas.
- `lucide/*.svg`: the 27 upstream SVGs, copied byte for byte. Each is pinned by
  its SHA-256 in the manifest.
- `LICENSE-lucide.txt`: the Lucide licence text (ISC; the Feather-derived icons
  are also MIT). Attribution is in `NOTICE` and `REUSE.toml`.
- `index.theme`: the theme index the installer publishes.
- `install.mix`: the offline installer.

## Installer

```text
mix share/icons/install.mix [--root /absolute/share]
mix share/icons/install.mix --check
mix share/icons/install.mix --verify
```

The default root is `$MIXOS_SHARE`, otherwise `/opt/mixos/share`. The installer
makes no network access.

- **`--check`** validates the bundle and writes nothing, not even the root. It
  does not look at the root at all. It checks the manifest schema, family,
  release and stroke width; that `icons` is a non-empty list and `sources` a
  non-empty map; every source's name, path and SHA-256 (a `null` or non-hex
  digest is refused); that each source is a plain file; each SVG's 24-unit
  `viewBox`, single root, upstream stroke of 2 and line-icon traits; alias names,
  contexts, uniqueness and the `-symbolic` suffix; and the licence and theme index.
- **install** (the default) runs the same checks, then publishes the outputs.
- **`--verify`** runs the same checks and then compares every installed output
  byte for byte with what the bundle generates. It exits non-zero on any
  difference, including a missing file.

## Outputs

All outputs are under `<root>/icons/mixos`, the theme `apps.mix` selects when
the user has no icon preference:

- `scalable/apps/<alias>.svg`, `scalable/status/<alias>.svg` and
  `scalable/actions/<alias>.svg`: one file per manifest alias. Each is its
  upstream SVG with every `stroke-width` changed from 2 to 1.5, and nothing else.
- `LICENSE-lucide.txt`: the licence, beside the icons it covers.
- `index.theme`: `Directories` lists `scalable/apps`, `scalable/status`,
  `scalable/actions`, then the legacy `symbolic/apps`. The native directories
  come first, so live absolute icon paths resolve to the fresh files and not the
  older Material copies. Old Material status directories are not indexed. Each
  section must be `Type=Scalable`, with a positive `Size` inside its
  `MinSize`..`MaxSize` bounds and its own `Context`. The theme must not
  `Inherits` another theme.

The brand icons are not in the bundle. `share/brand/install.mix` owns
`icons/mixos/scalable/apps/dev.mixos.svg` (the full-colour launcher icon) and
`icons/mixos/symbolic/apps/dev.mixos-symbolic.svg`, and creates `index.theme`
only when none exists. The bundle's index lists both directories, and this
installer never touches files it does not own, so the two installers can run
in either order.

## Transaction and safety

- Nothing is written until every output is generated and validated. The
  installer stages each output in `<root>/icons/.lucide-install-<uuid>/` and
  verifies the staged bytes.
- Each output is then renamed into place. A file being replaced is renamed
  aside (not read), so its exact bytes come back on rollback. `index.theme` is
  published last.
- A failed publish undoes this run's renames, newest first. It removes the files
  this run created and restores the bytes it replaced. The original error is
  kept in the message.
- If an undo step also fails, the error says so (`ICON_ROLLBACK`), and the
  staging directory is kept with the replaced bytes for manual recovery. Remove
  it by hand once the tree is repaired. A later run never deletes it.
- The root must be an absolute path with no empty, `.` or `..` segment. Every
  existing directory from `/` down to the root must be a plain directory, not a
  symlink. This walk runs before anything is created, and `--verify` runs it too.
  Directories and outputs under the root are checked the same way, so a symlink
  anywhere on the path is refused and nothing is written outside the root. The
  check is not race-free against a writer inside the root.
- Files the installer does not own are never touched, and the user's icon
  preference (for example `~/.config/kdeglobals`) is never read or written.
- Removing an alias from the manifest does not delete its old file, which stays
  in place, unlisted. Nothing is deleted by the installer.

## Lookup

The panel, the window fallback and the scene-host fallbacks resolve these names
through the apps citizen (`services/compd/scripts/lib/apps.mix`):

- Panel status icons: the scene models' `PANEL_THEMED_ICONS` with `-symbolic`
  appended, except `start-here-kde`, which is the KDE launcher and not a bundle name.
- `application-x-executable-symbolic`, the window fallback.
- MixOS application aliases (`dev.mixos.<app>-symbolic`).

The tests in `services/compd/tests/desktop_icons_test.mix` run the real lookup
against every manifest alias and panel name at several sizes. Their rollback
case is a real failure, not an injected one: the status directory is made
read-only, so the publish replaces the app aliases and then a status rename is
refused with `Permission denied`. As root, the installer runs under `setpriv`
with `DAC_OVERRIDE` and `DAC_READ_SEARCH` dropped, so the mode bits apply.

After installing into a running session, send `apps.reload` to the apps citizen
to refresh its theme index and cached misses and publish `apps.changed`. Reload
the panel through `scenes.reload {name:"panel"}`. This migration moves icons
from `symbolic` to fresh `scalable` paths, which also refreshes the renderer's
decoded assets. Verification checks installed bytes. A later repair that
changes bytes at the same filename needs a compositor restart to refresh its
decoded image cache.

## Scene coverage

- Panel: Lucide status icons and the generic `start-here-symbolic` fallback.
  Existing MixOS brand artwork has priority. Application and tray artwork
  keeps its original lookup.
- Launcher: the MixOS app aliases and generic executable fallback come from
  this bundle; third-party application artwork keeps its original lookup.
- Editor: eight order arrows use `go-previous-symbolic` and
  `go-next-symbolic`; eight size controls use `minus-symbolic` and `plus-symbolic`.
- Calendar: paging uses `chevron-left-symbolic` and `chevron-right-symbolic`.
- Settings: eight size controls use `minus-symbolic` and `plus-symbolic`.
- Notes, network and the generated workspace rename dialog: their remaining
  labels and state markers are ordinary text, with no icon substitutes.

The 26 editor, calendar and settings controls refer directly to their installed
assets under `/opt/mixos/share/icons/mixos/scalable/actions`. These replace
controls previously drawn as text glyphs. They do not rely on a distro theme
or change a user's desktop theme selection. Test fixtures point these paths at
their temporary share directory.

The shared app design token `icon.stroke_width` also defaults to 1.5.
Changing that token through Preferences affects app rendering; these installed
scene SVGs retain their generated 1.5 stroke. Dynamic regeneration and cache
invalidation through Preferences are not implemented by this bundle.

## Not icon-family assets

These stay as text or shapes and get no alias: ordinary labels, workspace
numbers (`ws_label`), and the status dots and bullets (`●`, `○`, `•`).
