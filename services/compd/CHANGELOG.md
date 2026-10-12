# Changelog

## 0.2.0

- `comp.region.select` 0.2.0: a click (under 4 logical px of travel, with
  no drag on the way) and Enter or keypad Enter now select the whole output
  under the pointer, answering exactly as a drag from corner to corner does;
  a click used to re-arm the selection. A drag stays a drag even if it
  returns to its start. The overlay dims the output (clear inside a drag),
  outlines the selection in the theme's focus-ring colour and shows a hint
  ("Drag to select an area · Click or Enter for the whole screen · Esc to
  cancel"), only on the selected output; a selected reply waits for a frame
  of that output without the overlay.
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
- The whole `comp.*` verb catalogue is registered, and a registry test keeps
  it exact.
- Window titles are shaped with cosmic-text over the apps' embedded faces, so
  they match the apps' type and need no fonts installed.
