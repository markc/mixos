# exportd

One-way exports of the MixOS settings to foreign desktops and toolkits,
written in Mix. Each target is its own Bus citizen in `scripts/` with its
own unit; settingsd stays the one source and nothing is read back. The
[manual](../../docs/exportd/README.md) describes each exporter.

- `scripts/cosmic.mix`: the COSMIC desktop (cosmic-config), unit
  `exportd-cosmic@.service`.
- `scripts/kde.mix`: KDE Plasma (kdeglobals, kwinrc, colour schemes), user
  unit `exportd-kde.service`.
- `scripts/gtkqt.mix`: GTK and Qt (gtk-3.0/gtk-4.0, qt5ct/qt6ct), unit
  `exportd-gtkqt@.service`.

From the checkout root:

```
mix services/exportd/tests/cosmic_test.mix
mix services/exportd/tests/kde_test.mix
mix services/exportd/tests/gtkqt_test.mix
mix services/exportd/tests/gtkqt_export_test.mix
```
