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

Every change shows in Prefs' own window first. Nothing is saved until
**Apply** (Ctrl+S), which writes all your changes in one step; **Revert**
drops them. Only the settings you changed are written.

If the appearance is changed elsewhere while you edit (another window, or an
agent), Prefs follows it on every setting you did not touch. If it changed
one you did, Prefs names it and applies nothing until you choose **Keep my
changes** or **Revert**. Apply is checked against the revision you were
shown, so it never lands on a change you have not seen. If its answer is
lost, Prefs asks settingsd for the change's receipt before anything else is
applied (**Check again** asks now); if settingsd no longer holds one, the
session's current state decides what the status line says.

## For agents

Prefs answers on the Bus as `prefs`. `prefs.info` reports the whole state;
`prefs.apps` the rows, selection and notes; `prefs.appearance` the session
look, the unapplied draft and the revision read; `prefs.appearance.set`
edits the draft (`scheme`, `style` with `own` for the scheme's own, `mode`,
`contrast`, `decorations`, `caption_side`); `prefs.select {app}` and
`prefs.panel {name}` select; `prefs.execute {id}` runs any command a button
or menu would (`apps.check`, `apps.install`, `appearance.apply`,
`appearance.revert`, …), and `prefs.commands` lists them with their
enablement. The window is drivable like every MixOS app
(`prefs.ui.*`, `prefs.window`). The verbs are registered in
`docs/spec/bus/verbs.conf.mix`.

For scripted work, call the services directly (`releases.*`,
`settings.*`): Prefs is their window, not a second implementation.
