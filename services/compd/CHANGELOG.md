# Changelog

## 0.2.0

- `comp.window.maximize` accepts optional `output` (key or native name),
  including retargeting an already maximised window. Normal restore geometry
  survives, and focus and workspace stay unchanged. Unknown outputs refuse
  before effects. Toolkit uses this to restore saved maximised frames.

- The user keys belong to the user. compd enforces the rule itself, ahead
  of its own bindings, because on a MixOS VT it reads the keyboard itself.
  The user keys are bare F1–F12 and Right Ctrl, Right Alt or Right Shift with
  an arrow, matched side-exact with inputd's classifier and press-latched
  disposition. A user key fires the session user's binding, or goes to the
  focused client when the user bound it `passthrough`; otherwise it is
  swallowed, and no client sees its press, repeat or release. Under a
  session lock every user key is swallowed and nothing fires.
  - The bindings are read from keymap.v1 (`inputd/keymap.json` in the
    user's MixOS config dir, or `$MIXOS_USER_KEYMAP`) and reloaded when the
    file changes. A bad file keeps the last good keymap, and a missing one
    binds nothing.
  - A `comp.*` command runs as its Bus request does. Any other command is sent to
    its service fire-and-forget, with the row's args as the body.
    `repeat: allow` rows repeat at the seat's rate while held.
  - `bindings.user` lists the rows.
  - compd ships no user-key binding. The opt-in bare-F9 Bus key can no
    longer fire.
- `comp.region.select` 0.2.0: a click (under 4 logical px of travel, with
  no drag on the way) and Enter or keypad Enter now select the whole output
  under the pointer, answering exactly as a drag from corner to corner does;
  a click used to re-arm the selection. A drag stays a drag even if it
  returns to its start. The overlay dims the output (clear inside a drag),
  outlines the selection in the theme's focus-ring colour and shows a hint
  ("Drag to select an area · Click or Enter for the whole screen · Esc to
  cancel"), only on the selected output, in the session look (followed on
  every change); a selected reply waits for a frame of that output without
  the overlay.
- While a `comp.region.select` run holds the seat the cursor is a
  crosshair (the theme's `crosshair`, else `cross` or `tcross`, else the
  arrow); the normal cursor is back on the frame that ends the run, however
  it ends, so the clean frame never shows the crosshair. A cursor's hotspot
  is now read after its shape is chosen, so a change of shape lands in place
  on its first frame.
- The capture panels (the setup chooser, region border and dim, Stop
  buttons and the Continue, Save, Encoding and error dialogs) draw in the
  session theme, followed on every change like the region overlay, with
  the embedded theme until the shell delivers one. Open panels repaint at
  once.
- Workspaces: `comp.workspaces.list`, `comp.workspaces.set_count {count,
  persist}` and `comp.workspace.rename {index, name, persist}`, and the
  `comp.workspace.changed` topic. The count and names persist in
  `preferences.json` (`workspaces_count`, `workspace_names`) and compd
  starts with them. `workspaces.list` rows carry `name`.
- The pointer moves at the same logical speed at every output scale: a
  relative delta is multiplied by the output scale as well as
  `cursor_sensitivity`, so a scale-2.5 output no longer needs a sensitivity
  of 2.5.

- compd's own UI is egui: capture overlays and dialogs, the frame-rate
  overlay, notifications and the Mix Scenes shell are egui programs painted
  with egui-wgpu into textures on compd's own EGL context, on the same MixOS
  look the apps draw with (`toolkit`).
- Mix Scenes are laid out as flexbox with taffy, as Quoin lays them out;
  `shell.scene.layout` reports the rectangles each node was drawn and
  hit-tested at.
- The session look follows settingsd through the shared consumer on the scene
  host's own connection (compact schema-2 deliveries, offline cache), and
  window decorations take their themes from the same look.
- A UI surface off screen is suspended: its animations stop waking the
  compositor until it is shown again.
- The whole `comp.*` command catalogue is registered, and a registry test keeps
  it exact.
- Window titles are shaped with cosmic-text over the apps' embedded faces, so
  they match the apps' type and need no fonts installed.
