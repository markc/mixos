---
title: Decision — shipped system dialogs
description: Besides the Scene Editor, a shipped scene template that is hidden and declares a dialog may load as a dialog (a system dialog, such as the pager's rename prompt). It is enabled only while it shows, so the single dialog seat stays free.
---

# Decision — shipped system dialogs

Status: accepted 2026-10-12. Widens the scene loader's v1 rule that only
the Scene Editor may be a dialog.

## The problem

The scene host has one dialog seat. A dialog takes it when it loads, keeps
it while loaded, takes the keyboard when it shows and gives it back when it
hides. The loader therefore refused `window.kind: "dialog"` for every
installation except the Scene Editor, so no other part of the desktop could
ask a short question, such as the name for a workspace.

## The decision

- **A system dialog** is a shipped scene template that:
  - is marked `hidden: true` in its shipped `template.conf.mix`;
  - declares `window.kind: "dialog"` in its shipped `scene.mix`;
  - is installed under its own template name.

  The loader allows it to load as a dialog. The permission is read from the
  shipped files under the templates root, never from the user-editable
  installed copy. A shipped dialog that is not hidden, a system dialog
  installed (or forked) under another name, and any other scene edited into
  a dialog are all still refused. The installed copy of a system dialog is
  the person's own, like any installed scene, and keeps the permission its
  name carries.
- **Hidden templates stay out of the gallery**, so people still see one
  dialog scene, the Scene Editor.
- **A system dialog holds the seat only while it shows.** Its owner installs
  it disabled and enables it to ask. The dialog disables itself once its
  dialog is hidden by Save, Cancel or Esc, or by another dialog taking the
  seat.
- **The Scene Editor.** By default the Scene Editor is a native application
  window, not a dialog scene. It never uses the dialog seat, so it and a
  system dialog can both be open. The legacy scene frontend
  (`SCENES_EDITOR_APP=0`) is a dialog scene and keeps priority:
  - opening it pre-empts a system dialog;
  - it holds the seat for as long as it is loaded, hidden or not, so a
    system dialog asked for meanwhile is refused (`DIALOG_BUSY`) and its
    owner says so;
  - unloading it (`scenes.editor.close {unload: true}`) frees the seat.
- **One system dialog at a time.** A dialog asked to show for something else
  while it is already showing (another workspace) refuses until it is saved
  or cancelled. A failed start is rolled back (the scene is disabled again),
  and the dialog releases the seat only when the host's current state shows
  it hidden, never on a stale notice.
- **Focus.** A dialog takes the keyboard when it maps and gives it back when
  it hides. Its `window.autofocus` field gets focus as it opens.

The first system dialog is `share/scenes/workspace-rename`. Right-clicking a
pager button opens it for that workspace, and it names the workspace through
`comp.workspace.rename`.
