---
title: Prefs
description: MixOS's preferences window — a sidebar of editors, starting with Applications, which follows, installs, updates, rolls back and removes upstream releases.
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

## For agents

Prefs answers on the Bus as `prefs`. `prefs.info` reports the whole state;
`prefs.apps` the rows, selection and notes; `prefs.select {app}` and
`prefs.panel {name}` select; `prefs.execute {id}` runs any command a button
or menu would (`apps.check`, `apps.install`, …), and `prefs.commands` lists
them with their enablement. The window is drivable like every MixOS app
(`prefs.ui.*`, `prefs.window`). The verbs are registered in
`docs/spec/bus/verbs.conf.mix`.

For scripted work, call the releases service directly (`releases.*`): Prefs
is its window, not a second implementation.
