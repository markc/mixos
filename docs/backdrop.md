---
title: Backdrop
description: The MixOS desktop background — drawn by its own program on the compositor's background layer, on every output, at native and fractional scale.
---

# Backdrop

Backdrop draws the MixOS desktop background. It is its own program, not
part of the compositor: it puts one surface on the compositor's
background layer for each output, under every window and panel, and
draws it with egui. A crash in it never takes the session down.

What it will draw:

- a **colour**, an **image** (fill, fit, centre, tile or stretch) or a
  **slideshow** that crossfades through a folder;
- optionally, over any of them, a **live folder**: the files of a folder
  as icons, photos and videos as thumbnails, updated as files come and go.

**Today it is a probe.** It fills each output with the session theme and
shows the output's size and scale and the clicks it receives. That proves
the surfaces, the scaling and the input before the wallpapers and the
folder view are built on them.

It costs nothing while nothing changes: it draws only when its content,
size, scale or input changes.
