---
title: exportd
description: One-way exports of the MixOS settings to foreign desktops and toolkits, starting with the COSMIC desktop's cosmic-config tree.
---

# exportd

MixOS keeps every desktop setting in [settingsd](../settingsd/README.md).
A desk that runs a foreign desktop (COSMIC, Plasma) or foreign toolkits
(GTK, Qt) gets the same look through an exporter: a Mix Bus citizen that
follows the settings topics and writes the foreign desktop's own config.
The export is **one way**. settingsd is the source; a change made in the
foreign desktop's own settings app is overwritten at the next revision,
and nothing is read back.

Every exporter:

- reconciles once when it connects, then writes each revision's changed
  keys in one burst, with no polling;
- writes only the keys it owns, and only where the contents differ;
- keeps the contents it replaced, so one command undoes it;
- stops writing while its gate is `false`: `appearance.export_desktop`
  for the desktops, `appearance.export_toolkits` for GTK and Qt.

Colours and fonts come from `settings.palette`, the derived palette
settingsd computes; an exporter names them and computes none.

## COSMIC

`scripts/cosmic.mix` writes a COSMIC desktop's cosmic-config tree
(`$XDG_CONFIG_HOME/cosmic`, one RON file per key). cosmic-comp, the panel
and every libcosmic app watch those files, so the desktop follows at once.

```text
mix --serve <prefix>/lib/exportd/cosmic.mix --name exportd-cosmic
```

The unit `exportd-cosmic@.service` runs it in a session, as that
session's user.

### What it writes

| Settings | cosmic-config |
|---|---|
| `appearance.mode` (resolved; `auto` is settingsd's) | `com.system76.CosmicTheme.Mode` v1 `is_dark`; `auto_switch` always `false` |
| the palette, `appearance.contrast`, accent, tints, backgrounds | both builders (`com.system76.CosmicTheme.Dark.Builder`, `.Light.Builder` v2) and both derived themes (`.Dark`, `.Light` v2): every container, component, the 32-colour palette, accent text, shade |
| `appearance.roundness`, `appearance.density` | both builders' and themes' `corner_radii`, `spacing`; `com.system76.CosmicTk` v1 `interface_density`, `header_size`; the panel and dock radius, padding and spacing |
| `appearance.frosted.*` | `frosted`, `alpha_map`, `frosted_windows`, `frosted_system_interface`, `frosted_panel`, `frosted_applets` |
| `window.active_hint`, `window.hint_colour`, `window.gaps` | `active_hint`, `window_hint`, `gaps` (the inner gap at least the hint width) |
| `window.hint_shown`, `window.clip_*`, `window.shadow_tiled`, `window.edge_gravity`, focus | `com.system76.CosmicComp` v1 `active_hint`, `appearance_settings`, `edge_snap_threshold`, `focus_follows_cursor`, `focus_follows_cursor_delay`, `cursor_follows_focus` |
| `appearance.font_family`, `mono_family`, `icon_theme`, `window.controls.*` | `CosmicTk` v1 `interface_font`, `monospace_font`, `icon_theme`, `show_minimize`, `show_maximize`; `apply_theme_global` always `false` (MixOS's own GTK and Qt export does that work) |
| `shell.panels.top`, `.bottom` | `com.system76.CosmicPanel.Panel` and `.Dock` v1, and which of them `com.system76.CosmicPanel` v1 `entries` lists |
| `wallpaper.*` | `com.system76.CosmicBackground` v1 `all`, `same-on-all` and one key per output |

The fonts are the palette's too: the families MixOS draws, the session's
choice or the bundled Inter and JetBrains Mono (COSMIC takes the family
alone; it has no size setting). Panel applet
lists and the wallpaper are written only once MixOS has its own; until
then COSMIC keeps its own.

Each key is replaced atomically (a `.atomicwrite` temporary, then a
rename), builders before themes and the mode last, so a mode flip lands on
two complete themes.

### Commands

| Command | Request and reply |
|---|---|
| `exportd.cosmic.status` | → `{binding, root, revision, paused, dry_run, last}` |
| `exportd.cosmic.plan` | → `{changed: [path], unchanged}`: what an export would write; writes nothing |
| `exportd.cosmic.reconcile` | export now; clears a pause |
| `exportd.cosmic.undo` | `{to: "previous" \| "original"}` → `{restored, to, paused}`: every key back to its contents before the latest write, or before the first; exporting then pauses until `reconcile` |
| `exportd.cosmic.owned` | → `{paths}`: every key file it has written |

### Environment

| Variable | Default |
|---|---|
| `MIXOS_SETTINGS_INSTANCE`, `MIXOS_SETTINGS_PROFILE` | the hostname, `default` (the profile must be `default`) |
| `MIXOS_SETTINGS_SERVICE` | `settingsd` |
| `MIXOS_EXPORTD_COSMIC_ROOT` | `$XDG_CONFIG_HOME/cosmic` |
| `MIXOS_EXPORTD_STATE` | `$XDG_STATE_HOME/mixos/exportd/cosmic` (undo and pause) |
| `MIXOS_EXPORTD_DRY_RUN=1` | plan only: log what would change, write nothing |

## KDE Plasma

`scripts/kde.mix` runs in the Plasma user's session as `exportd-kde`:

```text
mix --serve <prefix>/lib/exportd/kde.mix --name exportd-kde
```

The user unit `exportd-kde.service` starts it with the graphical session,
only when the session is Plasma (`XDG_SESSION_DESKTOP=KDE`).

It reconciles once at start, then once per settings revision that moves
`appearance`, `window`, `accessibility` or `shell.panels`: a revision that
moves several of them delivers one topic each, and the deliveries of a
revision already reconciled are skipped. A reconcile reads `settings.get`
and, only when an `appearance.*` value changed, `settings.palette`.

### What it writes

| settings | KDE |
|---|---|
| the resolved mode (`resolved.mode` of `settings.get`; never `auto`) | `kdeglobals [General] ColorScheme=MixOS-Dark` or `MixOS-Light`, and the scheme's `[Colors:*]`, `[ColorEffects:*]` and `[WM]` groups; both modes also as `<data>/color-schemes/MixOS-{Dark,Light}.colors` |
| the accent and colour axes, through the palette | `[General] AccentColor` (`accentColorFromWallpaper=false`), `[Colors:Selection]`, every scheme colour |
| the palette's fonts, `accessibility.text_scale` | `[General] font`, `menuFont`, `toolBarFont`, `fixed`, `[WM] activeFont` |
| `appearance.icon_theme` | `[Icons] Theme`, the `-dark` variant in dark mode when installed |
| `window.hint_colour` | `[WM] activeBackground`, `activeForeground` |
| `accessibility.reduced_motion` | `[KDE] AnimationDurationFactor` (`0` when reduced, else `1`) |
| `appearance.density` | `kwinrc [org.kde.kdecoration2] BorderSize` (`Tiny`, `Normal`, `Large`), `BorderSizeAuto=false` |
| `window.caption_side`, `window.controls.*` | `kwinrc [org.kde.kdecoration2] ButtonsOnLeft`, `ButtonsOnRight` |
| `shell.panels.<panel>.gap` (enabled panels) | `floating` of the Plasma panel on that panel's `edge` (the Plasma scripting interface); panels on other edges are left alone |

The decoration plugin and theme (`library`, `theme`) are the user's and are
never written; the keys above apply to whichever decoration is active.

Scheme colours are the palette's opaque values; a translucent one is
refused. A font size in pixels becomes points at 96 dpi (× 0.75, to the
nearest half point), after the text scale. `kdeglobals [MixOS]` carries a
stamp with the revision written; the stamp alone never causes a write. A
key the user made immutable (`key[$i]`) is not written, and the report
names it (`immutable: <file>|<group>|<key>`).

### How it writes

Keys are matched by their raw text, so `Icon[de]` or `font[$i]` is never
mistaken for `Icon` or `font`. Each file is re-read, edited as text and
replaced atomically while holding KConfig's own lock (`<file>.lock`), so a
concurrent KConfig writer loses nothing. The last key of each group, and
every font, scheme, accent, icon and animation key, then goes through
`kwriteconfig6 --notify`, so KConfig's change notification reaches each
group. Then `busctl --user` sends the `KGlobalSettings` `notifyChange`
signal (palette, fonts), `KIconLoader` `iconChanged`, and
`org.kde.KWin.reconfigure`; a call that fails is reported as
`<name>_unreachable`. What a burst touched is recorded as dirty before the
first write and cleared once every call succeeded, so after a crash or an
unreachable session bus the next run notifies again.

**Known limit:** running apps follow a colour, icon and decoration change
at once but keep their old font until they restart; apps started
afterwards get the new font.

**Not exported yet** (key space §5.3 lists them for kde-export; the next
slice): `wallpaper` (desktop containments), `workspaces` (`kwinrc
[Desktops]`, `RollOverDesktops`), `input` (`kxkbrc`, `kcminputrc`), `time`
(clock `use24hFormat`, `showDate`), `region`, `keymaps`
(`ModifierOnlyShortcuts`), the window behaviour rows (focus policy, snap
zones, Super key) and `appearance.frosted.*` (KWin blur). The behaviour
rows wait for a decision, since they change how the desktop behaves, not
how it looks.

### Commands

| Command | Does |
|---|---|
| `exportd.kde.status` | the binding, last revision, gate, pause, the last run's report or error |
| `exportd.kde.plan` | what a reconcile would write now; writes nothing |
| `exportd.kde.reconcile` | reconcile now; clears a pause |
| `exportd.kde.undo` | put back the keys and panels the last burst changed, then pause |
| `exportd.kde.restore` | put back every key as it was before the first write, the scheme files (a user's own `MixOS-*.colors` gets its saved bytes back; files it created are removed) and the panels, forget the record, then pause |

While paused (undo or restore, kept across restarts) no revision writes;
`exportd.kde.reconcile` resumes.

### Environment

| Variable | Default |
|---|---|
| `MIXOS_SETTINGS_INSTANCE` | this node's name |
| `MIXOS_KDE_EXPORT_DRY=1` | plan and report, write nothing |
| `MIXOS_KDE_EXPORT_STATE` | `$XDG_STATE_HOME/mixos/kde-export` (the undo record, saved scheme files, the pause marker) |
| `MIXOS_KDE_EXPORT_RECONFIGURE=0` | no D-Bus calls |
| `MIXOS_KDE_EXPORT_KWRITECONFIG`, `MIXOS_KDE_EXPORT_BUSCTL` | `kwriteconfig6`, `busctl`; a JSON argv list replaces them (the tests' stand-ins) |

External programs: `kwriteconfig6` (KDE Frameworks 6) and `busctl`.
## GTK and Qt

`scripts/gtkqt.mix` (`--name exportd-gtkqt`, unit `exportd-gtkqt@.service`)
writes GTK CSS and `settings.ini` keys and the qt5ct/qt6ct colour schemes
and config on every desk but a KDE Plasma one, where the KDE export covers
GTK and Qt. Its commands are `exportd.gtkqt.status`, `.reconcile` and
`.undo`. The [GTK and Qt page](gtkqt.md) has the details.

## Changelog

See the [changelog](changelog.md).

Both desktop exporters read all values and the palette at one incarnation and revision, retrying a moving record before writing. Prefix topics are hints, never snapshot fragments. KDE broadcasts use busctl emit OBJECT INTERFACE SIGNAL (no positional service). Wallpaper RGB triples refuse non-opaque alpha; KDE hint colours are composited over each mode\u0027s titlebar.
