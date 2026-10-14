---
title: Prefs
description: MixOS's preferences window — a sidebar of editors: Applications follows, installs, updates, rolls back and removes upstream releases; Appearance sets the session's colour scheme, style, mode, contrast and window frame.
---

# Prefs

Prefs holds MixOS's preference editors in one window: a sidebar of editors
on the left, the selected editor on the right. The name is a nod to the
Prefs drawer of AmigaOS, where each setting had its own small editor.

## Applications

Applications shows the apps you follow, from the [releases
service](releases.md): each app's installed version, the latest release,
when it was published, and its status (up to date, update available, not
installed, not checked, or check failed).

- **Check for updates** (Ctrl+Shift+R) asks GitHub about every followed app.
- **Update all** installs every newer release.
- Select an app to see its latest **release notes**, then:
  - **Install or update** (Ctrl+I) installs it, or updates it if it is
    installed;
  - **Roll back** returns to the previous version;
  - **Remove…** asks first; Cancel is the default, so Enter never removes.
- **Refresh list** (Ctrl+R) reads the list again without asking GitHub.

One operation runs at a time; the status bar says what is happening and how
it ended. Installs are per user, verified against the checksums each release
publishes, and the previous version is kept for rolling back: see
[Application releases](releases.md).

If the releases service is not running, the editor says so instead of an
empty list. You follow apps by listing them in
`~/.config/mixos/releases.mx`.

## Appearance

Appearance sets the session's look through the settings service
(settingsd): the **colour scheme** (each shown with its accent), the
**style** (the scheme's own, or Plain, Pro, Studio or Classic), **light or
dark**, **contrast**, and the **window frame**: a title bar drawn by the app
or by the system, and which side the app's window buttons sit on.

Each change applies to the whole session as you make it, as in GNOME's
settings: the apps that follow the session look (Prefs itself, BusViewer,
and the portal's clients) change with it. Only the setting you changed is
written. Changes made quickly one after another are sent in turn, never
lost.

If settingsd refuses a change, Prefs keeps it and says so: **Apply**
(Ctrl+S) tries again and **Revert** drops it. If the appearance is changed
elsewhere at the same moment (another window, or an agent), Prefs follows
it on every setting you did not touch. If it changed one you did, Prefs
names it and applies nothing until you choose **Keep my changes** or
**Revert**. Every change is checked against the revision you were shown, so
it never lands on a change you have not seen. If its answer is
lost, Prefs asks settingsd for the change's receipt before anything else is
applied (**Check again** asks now); if settingsd no longer holds one, the
session's current state decides what the status line says.

## For agents

Prefs answers on the Bus as `prefs`. `prefs.info` reports the whole state;
`prefs.apps` the rows, selection and notes; `prefs.appearance` the session
look, the expected look while a change settles, any held or conflicting
edits and the revision read; `prefs.appearance.set`
changes it at once (`scheme`, `style` with `own` for the scheme's own, `mode`,
`contrast`, `decorations`, `caption_side`); `prefs.select {app}` and
`prefs.panel {name}` select; `prefs.execute {id}` runs any command a button
or menu would (`apps.check`, `apps.install`, `appearance.apply`,
`appearance.revert`, …), and `prefs.commands` lists them with their
enablement. The window is drivable like every MixOS app
(`prefs.ui.*`, `prefs.window`). The verbs are registered in
`docs/spec/bus/verbs.conf.mix`.

For scripted work, call the services directly (`releases.*`,
`settings.*`): Prefs is their window, not a second implementation.
## Wallpaper

In **Appearance → Wallpaper**, choose the default or an output, then Theme,
Colour or Image. Colour accepts presets, a picker or `#RRGGBB`. Browse opens the native
image browser at the installed wallpaper folder, or Pictures in your home
folder. It lists PNG/JPEG files and folders without blocking the window.
Presets, colour-picker and browser choices apply immediately. Finish a hex
colour or image path with Enter or leave the field to apply it. **Apply wallpaper**
also applies a valid entry; **Revert wallpaper** drops an unfinished edit;
**Refresh wallpaper** reads the current state again. The status shows acceptance
or the refusal reason.

An output's **Use default** removes just that override. Editing the default
preserves every output override. Images currently crop to fill. Backdrop reports
no fit modes, so there is no fit selector. Slideshow controls are not yet available.

Backdrop must be running on the same session Bus. A refusal shows its reason,
restores the displayed choice and leaves the last good wallpaper in place. Prefs checks for
intervening changes before sending the whole document; conflicts require
Revert and a fresh edit. This check is best effort: backdrop does not yet
provide an atomic compare-and-set operation. A lost reply is checked by a
state read, never automatically resent.

Wallpaper and browser state are included in `prefs.info`, wallpaper state in
`prefs.appearance`. Commands `wallpaper.apply`, `wallpaper.revert`,
`wallpaper.refresh` and `wallpaper.browse` are available through
`prefs.execute`; the normal `prefs.ui.*` verbs drive the controls and browser.

