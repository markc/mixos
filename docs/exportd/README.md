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
- stops writing while `appearance.export_desktop` is `false`.

Colours come from `settings.palette`, the derived palette settingsd
computes; an exporter names colours and computes none.

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

A null font is the design default (Inter, JetBrains Mono). Panel applet
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

## Changelog

See the [changelog](changelog.md).
