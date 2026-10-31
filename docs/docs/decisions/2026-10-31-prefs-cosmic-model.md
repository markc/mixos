---
title: Decision — Preferences on the COSMIC settings model
description: settingsd is the one settings authority for every desktop on a machine, Preferences is rebuilt to the shape of COSMIC Settings (clean-room), one theme derivation in libs/design feeds every egui app, the compositor and one-way exporters to COSMIC, KDE Plasma, GTK, Qt and the settings portal, so one change restyles every desktop.
---

# Decision — Preferences on the COSMIC settings model

Status: accepted 2026-10-31. Amends [desktop theming and style](2026-10-21-desktop-theming-and-style.md).

## Context

A MixOS machine can run several desktops side by side, each on its own
console: a distribution's own desktop (for example KWin and Plasma), a
COSMIC session, and MixOS's own compositor (compd) with its shell (Quoin).
MixOS egui applications already follow the settings authority, settingsd,
on every desk. The desktops around them did not: each kept its own settings
and its own look.

Preferences had grown layouts a person could not navigate: it was hard to
tell which control changed which part of the screen.

COSMIC Settings (GPL-3.0) was studied clean-room as the model. Three
findings decided the shape:

- Only the settings app derives COSMIC's theme. It builds the full theme
  from a few inputs (mode, accent, tints, roundness, density, frosted glass,
  contrast); applications and the compositor read only the derived values.
- COSMIC's own export to GTK and Qt is off by default.
- Its page tree (about 45 pages and 190 controls, each control writing one
  key) maps cleanly onto one key prefix per page.

## The decision

1. **settingsd is the one authority on a machine, for every desktop.**
   Every setting COSMIC Settings offers becomes a settingsd key. One
   Preferences page is one key prefix.
   - Every setting has one value for every desktop. A value that is
     inherently per output (display layout, panel output, wallpaper per
     screen) is keyed by connector name, never per desktop.
   - Settings that a host service applies (displays, time, power, default
     applications) are still settingsd keys; the service follows them.
   - **Live host state is not a setting.** Hostname, users, firmware,
     network and Bluetooth inventories, battery and the About page are read
     and written live through hostd. Preferences shows and edits them;
     settingsd never stores them.
   - Each desktop's settingsd either is the authority or mirrors it. A
     mirror republishes the authority's revisions locally and forwards
     writes to it.
   - backdrop's wallpaper configuration moves into settingsd
     (`wallpaper.*`); backdrop becomes a follower.
2. **The key space changes in one clean break.** A migration writes a new
   sealed record at the next store format and keeps the previous record
   beside it; `settingsd downgrade` converts back. There are no
   compatibility shims. The density float becomes three named steps,
   `compact | comfortable | spacious`.
3. **One theme derivation, one implementation.** The derivation of the full
   palette, radii and spacing from the appearance inputs lives in
   `libs/design`. Every follower uses it: the toolkit (every egui
   application), compd's window decorations and Quoin. Exporters derive
   nothing: settingsd answers `settings.palette` with each mode's finished
   values, and the exporters write what they are given.
4. **Export to other desktops is one-way, by Mix Bus citizens.** Each
   exporter follows settings revisions (no polling), owns the files it
   writes (one owner per file per desktop), reconciles once at start, and
   keeps the previous values for undo. Nothing is read back.
   - **COSMIC:** the theme inputs and derived theme for both modes, plus
     panel, dock, wallpaper, compositor and toolkit keys. COSMIC's own
     exporter is always switched off.
   - **KDE Plasma:** a colour scheme from the palette, fonts, accent and
     window decoration, then a reconfigure request. Plasma's panel
     configuration is snapshotted before any panel change, because its
     scripting interface has no undo.
   - **GTK, Qt and the settings portal:** on desktops that are not Plasma,
     GTK colour CSS and settings, qt5ct/qt6ct palettes, and the portal's
     colour scheme, accent and contrast. On Plasma these belong to Plasma's
     own bridges, which read the colour scheme the KDE exporter writes.
   - **XDG:** `mimeapps.list` and the autostart directory are rewritten from
     the default and startup application settings; Preferences is the way
     to change them.
   - `appearance.export_toolkits` gates the GTK, Qt and portal writes, and
     `appearance.export_desktop` gates the COSMIC and KDE exporters.
5. **Preferences is rebuilt in the shape of COSMIC Settings.**
   - The same pages, order, page anatomy and controls. Values apply
     instantly; only display changes confirm, with a revert countdown.
   - The application and its command stay `prefs`; the window title is
     "Preferences".
   - A page or control whose backend does not exist yet is shown disabled,
     naming the missing backend, never hidden.
   - The sidebar card, the section card and the setting row are toolkit
     widgets that any application may use. Preferences gets no private
     chrome, and the title bar stays MixOS's own.
   - Every page and control is a Bus command: `prefs.page.open` navigates
     and `prefs.ui.*` operates controls as a person would. Values are still
     written only through `settings.apply`.
   - The old layouts are deleted.
6. **Where Preferences differs from COSMIC on purpose:** search indexes
   every visible label and highlights the matched row; every control works
   from the keyboard; Escape closes a dialog. Everything else follows
   COSMIC, including "back" going to a page's parent.
7. **compd honours the `window.*` group:** the gap between tiles (default
   0), the active window hint, title-bar controls, clipping and tiled
   shadows.
8. **Missing backends are work items of their owners:** input devices,
   workspaces, blur and accessibility features in compd; Wi-Fi, Bluetooth,
   time, locale, accounts, firmware and power in hostd; display changes in
   monitord; a new audio service over PipeWire; a daytime source for
   automatic light and dark; a screen reader and an on-screen keyboard.
   New services start as Mix Bus citizens unless Mix cannot do the job.
9. **Clean room.** COSMIC (GPL-3.0 and MPL-2.0) is reference only. One
   reader studied the pinned source and wrote behaviour specifications;
   implementers receive the specifications without source references and
   never the source. MixOS takes no libcosmic dependency. Observing the
   running application is clean evidence.

## Amendments to desktop theming and style

- **Mode `auto`:** a daytime source resolves it to dark or light; followers
  never see `auto`.
- **Accent is an axis:** `auto`, a named swatch (resolved per mode), or
  `#rrggbb`.
- **Derived schemes:** a scheme may be authored as a few seed colours, from
  which `libs/design` derives every role. The default MixOS scheme becomes a
  derived one. Authored schemes stay.
- **Roundness:** `round | slightly_round | square` overlays the style's
  radii and adds a pill radius.
- **Density:** three named steps replace the float. Each keeps today's
  length multiplier and adds a spacing table for new surfaces.
  `ui.reduced_motion` and `ui.text_scale` move to `accessibility.*`.
- **Window and frosted groups:** `window.*` (active hint, gap, title-bar
  controls, clipping, tiled shadows) and `appearance.frosted.*`.
- **Export keys:** `appearance.icon_theme`, `appearance.export_toolkits`,
  `appearance.export_desktop`.
- **Placement:** the advanced, registry-driven appearance controls move to
  Appearance › Experimental; per-application look lives under Applications
  › <application> › Look.

## Consequences

- One change in Preferences restyles the egui applications, the compositor,
  the shell, COSMIC, Plasma and GTK/Qt applications on every desktop of the
  machine.
- Exporters write other desktops' configuration, so every export is
  undoable, and hand edits to files an exporter owns do not survive.
- The settings and derivation work comes first, and the Preferences rebuild
  follows it.
- Many controls wait on backends. Preferences ships with those controls
  disabled, and each backend lands on its own schedule.
