# Changelog

## 0.2.0

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
