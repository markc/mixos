# KWin shadow protocol

`shadow.xml` is copied unchanged from KDE plasma-wayland-protocols:
https://invent.kde.org/libraries/plasma-wayland-protocols/-/blob/master/src/protocols/shadow.xml

Copyright 2015 Martin Gräßlin, LGPL-2.1-or-later (header preserved).
The licence text is in `LICENSE.LGPL-2.1`.
This protocol-only file is used by wayland-scanner to describe KWin's wire
interface; no KDE implementation code or plasma protocol crate is linked.
The native `shadow` feature reuses the workspace's wayland-backend,
wayland-scanner and winit versions. Refresh deliberately with its licence.

Winit retains ownership of the display and root surface. The plugin retains
an `Arc<Window>` until all guest-connection objects have been dropped. It
dispatches only its dedicated queue; winit reads the socket. Registry discovery
is asynchronous and bounded repaint requests stop after 250 ms. A compositor
without the global (including compd) gets no shadow requests.

Tiles use the theme's window-shadow blur, spread, offsets and colour, scaled
by the native output scale independently of egui zoom. They are immutable shm
buffers; no in-flight pixels are overwritten. Changes in theme or scale rebuild
them. Maximised/fullscreen/SSD windows detach; restore reattaches before
winit's next surface commit. The pure generator test runs without a display.
