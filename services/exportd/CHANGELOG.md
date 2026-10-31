# Changelog

## 0.1.0 — unreleased

- The COSMIC exporter (`scripts/cosmic.mix`): settingsd's look, window,
  panel and wallpaper settings written into a cosmic-config tree, with
  undo.
- The KDE Plasma exporter (`scripts/kde.mix`): the colour scheme from
  `settings.palette` in `kdeglobals` and the `MixOS-Dark`/`MixOS-Light`
  scheme files, the accent, the resolved fonts and the icon theme, the
  Breeze border size and buttons in `kwinrc`, and panel floating, then
  Plasma and KWin reconfigured over D-Bus; writes under KConfig's lock,
  re-notifies after a crash or an unreachable bus; undo and restore pause it.
