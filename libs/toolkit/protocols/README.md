# KWin shadow protocol

`shadow.xml` is copied unchanged from KDE plasma-wayland-protocols:
https://invent.kde.org/libraries/plasma-wayland-protocols/-/blob/master/src/protocols/shadow.xml

Upstream revision: `1db07441870d54157e937674b99380a7dcc0fb32`.
XML SHA-256: `3796bbc8d922df764dbee8ff147c3b945c023469d92695c0d59eabe39793afaa`.

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

Tiles use compd's shared `design::frame` shadow spec, rounded-box raster and
corner radius, rasterised at native output scale independently of egui zoom.
KWin's [Shadow::elementSize](https://invent.kde.org/plasma/kwin/-/blob/master/src/shadow.cpp)
uses buffer dimensions as logical pixels and this protocol has no buffer-scale
request. Native pixels are therefore filtered into logical-sized transport
tiles; sending the larger buffers directly would enlarge the corner cutout.
They
extend inward by the radius and offset; the actual client rounded rectangle
is cut out, preserving shadow beneath transparent corners. They are immutable
shm buffers; no in-flight pixels are overwritten. Theme, scale, size and focus
changes rebuild them, using compd's focused/unfocused alpha. Maximised,
fullscreen and SSD windows detach; restore reattaches before winit's next
surface commit. Pure generator tests run without a display. The separate final
GPU frame pass enables rounding only on alpha-capable surfaces, and winit's
transparent-window hint leaves the surface opaque region unset.

compd's decor renderer clips only server-decorated windows: it returns before
creating its clip for CSD. The toolkit therefore rounds CSD on compd too;
there is no second compositor round applied to the same CSD surface.
