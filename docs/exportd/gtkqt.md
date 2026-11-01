---
title: GTK and Qt export
description: How exportd's GTK and Qt exporter makes GTK and Qt applications on a desk follow the MixOS mode, accent, colours, fonts and icon theme, and how to undo it.
---

# GTK and Qt export

MixOS apps follow [settingsd](../settingsd/README.md) natively. GTK and Qt
apps (Firefox, Thunderbird, GNOME and KDE apps) read their own files.
`exportd-gtkqt` is the desk's one writer of those files: a per-session Mix
citizen that turns each settings revision into GTK CSS, GTK `settings.ini`
keys and a qt5ct/qt6ct colour scheme and config. The portal values
(`color-scheme`, `accent-color`, `contrast`) come from
[portald](../portald/README.md), which writes no files.

It runs on every desk except one that already has its own bridge: on a
KDE Plasma host, Plasma's GTK bridge and Qt platform theme read
`kdeglobals`, so the KDE exporter covers them and exportd-gtkqt is not
started there.

## What it writes

Everything goes under the session's `$XDG_CONFIG_HOME`:

| File | What |
|---|---|
| `gtk-4.0/mixos/dark.css`, `light.css` | libadwaita's named colours (`window_bg_color`, `accent_bg_color`, `blue_1` … `purple_5`, `light_1` … `light_5`, `dark_1` … `dark_5`) as `@define-color` lines; the light and dark greys are one ramp of the palette's `neutral_5` from white to black, with libadwaita's names and meaning (COSMIC writes 0-based names no stylesheet reads) |
| `gtk-4.0/gtk.css`, `gtk-3.0/gtk.css` | symlinks to the current mode's file, so a mode switch is one rename |
| `gtk-3.0/settings.ini`, `gtk-4.0/settings.ini` | `[Settings]`: `gtk-application-prefer-dark-theme`, `gtk-font-name`, `gtk-decoration-layout`, `gtk-icon-theme-name`, and in GTK 3 `gtk-theme-name` (`adw-gtk3`/`adw-gtk3-dark`, only when installed) |
| `qt5ct/colors/MixOSDark.conf`, `MixOSLight.conf` (and `qt6ct/`) | the 21 Qt palette roles for the active, inactive and disabled groups, opaque, `#AARRGGBB` |
| `qt5ct/qt5ct.conf`, `qt6ct/qt6ct.conf` | `[Appearance]`: `color_scheme_path`, `custom_palette`, `icon_theme`, `standard_dialogs=xdgdesktopportal`, `mixos_qt_version`; `[Fonts]`: `general`, `fixed` |

When the desk has `gsettings`, it also sets `org.gnome.desktop.interface`
`color-scheme`, `font-name`, `monospace-font-name`, `icon-theme`,
`gtk-theme`, `org.gnome.desktop.a11y.interface high-contrast` and `org.gnome.desktop.wm.preferences button-layout`.

Qt apps read the qt5ct/qt6ct files only through that platform theme, so
the desk's session environment sets `QT_QPA_PLATFORMTHEME=qt6ct` (a Qt 5
app looks for `qt5ct` under the same variable and falls back to its default
theme without it).

The colours are not computed here. `settings.palette` carries a `toolkit`
block per mode (`gtk`, the named colours, and `qt`, the three palette
groups), derived from the same theme the COSMIC export uses. The fonts
are the palette's too (`fonts.ui`, `fonts.mono`: the family, size and
weight MixOS draws, the session's choices over the design's), written
even when no font key is set. Sizes go to points as every exporter does:
px × `accessibility.text_scale` × 0.75, to the nearest half point; the
text scale is in the point size, so `text-scaling-factor` is not written.
A weight other than Regular becomes a Pango style word in GTK (`Inter
Light 9`) and Qt 5's weight scale in the qt conf. The icon theme is `appearance.icon_theme`, its `-dark`
variant under dark when installed. A desk without the theme keeps the
`mixos` icon theme, which Craft title bars draw their buttons from, and
`exportd.gtkqt.status` reports `icon_theme_missing: true`. The button
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

The CSS and colour-scheme files are exportd-gtkqt's own. In
`settings.ini` and the qt conf files it owns only the keys above: every
other key, comment and section is left as it was. A key whose value is no
longer what exportd-gtkqt last wrote was changed by hand; it is kept from
then on and listed under `kept` in the status. A foreign `gtk.css` that
was a regular file is copied once to `gtk.css.bak` before the link
replaces it.

It refuses to write through a symlink, and it does not run when
`XDG_CONFIG_HOME` is unset, relative, or the home directory's own
`.config`, so a nested desk never writes into its host desktop. A
standalone install that wants exactly that sets
`MIXOS_EXPORTD_GTKQT_HOME=1`.

## Undo

The first time it touches a file it records the file's original state,
and the first time it sets a key it records the key's original value
(under `$MIXOS_VAR/exportd/gtkqt/records.json`, else
`$XDG_STATE_HOME/mixos/exportd/gtkqt`). `exportd.gtkqt.undo` restores
every file and key that still holds what exportd-gtkqt wrote, lists the
ones changed since under `left`, and pauses the export.
`exportd.gtkqt.reconcile` resumes it. GSettings values are not undone.

## Commands

| Command | Answer |
|---|---|
| `exportd.gtkqt.status` | `outcome` (`waiting`, `written`, `unchanged`, `gated`, `paused`, `skipped`, `error`), the revision, `written`, `kept`, `icon_theme_missing`, `gsettings`, `last_error` |
| `exportd.gtkqt.reconcile` | resumes after an undo and writes the current record now |
| `exportd.gtkqt.undo` | `{restored, left}` |

```mix
send exportd-gtkqt exportd.gtkqt.status
print($result)
```

## Running it

The unit is `exportd-gtkqt@.service`, one per session, with the session's
environment file. Tests:

```text
mix services/exportd/tests/gtkqt_test.mix
mix services/exportd/tests/gtkqt_export_test.mix
```

The source is
[`services/exportd`](https://github.com/markc/mixos/tree/main/services/exportd).

Each file mutation has a durable write-ahead intent. Restart recovers an interrupted operation without losing the original. Undo persists its pause first and can be repeated after interruption. Hand-edit records survive undo and reconcile; generated empty INI files and owned backup copies are removed. An unreadable records.json is preserved and reported through status; writes remain refused until it is repaired. Every config ancestor must be a plain directory, and HOME/.config is also checked against the canonical HOME path.
