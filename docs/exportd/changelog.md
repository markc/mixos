---
title: exportd — changelog
description: Release notes of the exportd component, mirrored from services/exportd/CHANGELOG.md.
generated: by docs/_build/gen-site.mix from services/exportd/CHANGELOG.md; do not edit
---

# Changelog

> Mirrored from [`services/exportd/CHANGELOG.md`](https://github.com/markc/mixos/blob/main/services/exportd/CHANGELOG.md), where releases update it. The component's page is [exportd](README.md). This page describes the current `main` branch of MixOS.

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
- The GTK and Qt exporter (`scripts/gtkqt.mix`): libadwaita colours with
  mode links, settings.ini keys, qt5ct/qt6ct schemes and config, with
  hand edits kept and undo.

- Review fixes: coherent desktop record reads, COSMIC live topic delivery, GTK/Qt durable recovery and path refusals, KDE alpha hints and undo notifications, and strict busctl fixture arguments.
