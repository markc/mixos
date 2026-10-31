---
title: GTK and Qt export
description: How toolkit-export makes GTK and Qt applications on a desk follow the MixOS mode, accent, colours, fonts and icon theme, and how to undo it.
---

# GTK and Qt export

MixOS apps follow [settingsd](../settingsd/README.md) natively. GTK and Qt
apps (Firefox, Thunderbird, GNOME and KDE apps) read their own files.
`toolkit-export` is the desk's one writer of those files: a per-session Mix
citizen that turns each settings revision into GTK CSS, GTK `settings.ini`
keys and a qt5ct/qt6ct colour scheme and config. The portal values
(`color-scheme`, `accent-color`, `contrast`) come from
[portald](../portald/README.md), which writes no files.

It runs on every desk except one that already has its own bridge: on a
KDE Plasma host, Plasma's GTK bridge and Qt platform theme read
`kdeglobals`, so the KDE exporter covers them and toolkit-export is not
started there.

## What it writes

Everything goes under the session's `$XDG_CONFIG_HOME`:

| File | What |
|---|---|
| `gtk-4.0/mixos/dark.css`, `light.css` | libadwaita's named colours (`window_bg_color`, `accent_bg_color`, `blue_1` … `dark_4`) as `@define-color` lines |
| `gtk-4.0/gtk.css`, `gtk-3.0/gtk.css` | symlinks to the current mode's file, so a mode switch is one rename |
| `gtk-3.0/settings.ini`, `gtk-4.0/settings.ini` | `[Settings]`: `gtk-application-prefer-dark-theme`, `gtk-font-name`, `gtk-decoration-layout`, `gtk-icon-theme-name`, and in GTK 3 `gtk-theme-name` (`adw-gtk3`/`adw-gtk3-dark`, only when installed) |
| `qt5ct/colors/MixOSDark.conf`, `MixOSLight.conf` (and `qt6ct/`) | the 21 Qt palette roles for the active, inactive and disabled groups, opaque, `#AARRGGBB` |
| `qt5ct/qt5ct.conf`, `qt6ct/qt6ct.conf` | `[Appearance]`: `color_scheme_path`, `custom_palette`, `icon_theme`, `standard_dialogs=xdgdesktopportal`, `mixos_qt_version`; `[Fonts]`: `general`, `fixed` |

When the desk has `gsettings`, it also sets `org.gnome.desktop.interface`
`color-scheme`, `font-name`, `monospace-font-name`, `icon-theme`,
`gtk-theme`, `text-scaling-factor`, `org.gnome.desktop.a11y.interface
high-contrast` and `org.gnome.desktop.wm.preferences button-layout`.

Qt apps read the qt5ct/qt6ct files only through that platform theme, so
the desk's session environment sets `QT_QPA_PLATFORMTHEME=qt6ct` (a Qt 5
app looks for `qt5ct` under the same variable and falls back to its default
theme without it).

The colours are not computed here. `settings.palette` carries a `toolkit`
block per mode (`gtk`, the named colours, and `qt`, the three palette
groups), derived from the same theme the COSMIC export uses. Fonts are
`appearance.font_family` and `appearance.mono_family` (Inter and JetBrains
Mono when unset) at `appearance.font_size` in points (px × 0.75; 10 pt
when unset). The icon theme is `appearance.icon_theme`, its `-dark`
variant under dark when installed. A desk without the theme keeps the
`mixos` icon theme, which Craft title bars draw their buttons from, and
`toolkit-export.status` reports `icon_theme_missing: true`. The button
layout comes from `window.caption_side` and `window.controls.minimize` /
`.maximize`.

## How it follows settings

It subscribes to the `appearance`, `window` and `accessibility` keys
topics of the `default` profile. Each delivery, the retained replay at
start included, is only a hint: it reads one record (`settings.get
{view: values}` and `settings.palette`, both at one revision) and writes
what differs, so a desk that was off catches up at start. Files land with
an atomic rename; the colour files land before the links flip to them.

`appearance.export_toolkits = false` stops all writes. It undoes nothing.

## Your own edits

The CSS and colour-scheme files are toolkit-export's own. In
`settings.ini` and the qt conf files it owns only the keys above: every
other key, comment and section is left as it was. A key whose value is no
longer what toolkit-export last wrote was changed by hand; it is kept from
then on and listed under `kept` in the status. A foreign `gtk.css` that
was a regular file is copied once to `gtk.css.bak` before the link
replaces it.

It refuses to write through a symlink, and it does not run when
`XDG_CONFIG_HOME` is unset, relative, or the home directory's own
`.config`, so a nested desk never writes into its host desktop. A
standalone install that wants exactly that sets
`MIXOS_TOOLKIT_EXPORT_HOME=1`.

## Undo

The first time it touches a file it records the file's original state,
and the first time it sets a key it records the key's original value
(under `$MIXOS_VAR/toolkit-export/records.json`, else
`$XDG_STATE_HOME/mixos/toolkit-export`). `toolkit-export.undo` restores
every file and key that still holds what toolkit-export wrote, lists the
ones changed since under `left`, and pauses the export.
`toolkit-export.reconcile` resumes it. GSettings values are not undone.

## Commands

| Command | Answer |
|---|---|
| `toolkit-export.status` | `outcome` (`waiting`, `written`, `unchanged`, `gated`, `paused`, `skipped`, `error`), the revision, `written`, `kept`, `icon_theme_missing`, `gsettings`, `last_error` |
| `toolkit-export.reconcile` | resumes after an undo and writes the current record now |
| `toolkit-export.undo` | `{restored, left}` |

```mix
send toolkit-export toolkit-export.status
print($result)
```

## Running it

The unit is `toolkit-export@.service`, one per session, with the session's
environment file. Tests:

```text
mix services/toolkit-export/tests/toolkit_test.mix
mix services/toolkit-export/tests/export_test.mix
```

The source is
[`services/toolkit-export`](https://github.com/markc/mixos/tree/main/services/toolkit-export).
