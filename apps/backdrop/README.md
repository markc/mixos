# backdrop

The MixOS desktop background: an egui program on the compositor's background layer, one surface per output.

Documentation: <https://mixos.dev/backdrop/> — source [`docs/backdrop/README.md`](../../docs/backdrop/README.md).

Stage 1 live views (session selection, all outputs):

- `backdrop.view.set {view:"train", args?:{train_service, buildd_service, renderer}}`
  selects the train; `view:"wallpaper"` restores the existing wallpaper.
  Service arguments default to `train` and `buildd`; on a desk pass the hub's
  mesh service names. Selection is not persisted. A successful `backdrop.set`
  returns to wallpaper; slideshow timers retain their existing behaviour.
- `backdrop.view.list {}` lists `wallpaper` and `train` and the current view.
- `backdrop.view.cycle {dir:"next"|"prev"}` cycles those views. Keys belong
  in the user's inputd keymap or Mix macros; backdrop handles none.

The train uses `train.queue` and `buildd.queue`, subscribes to `train.landed`,
`train.rejected`, `buildd.started` and `buildd.done`, and refreshes every ten
seconds as a fallback. Identical snapshots do not redraw. Changes animate for
600 ms at at most ten frames/second; idle views have no animation timer.
Compositor frame callbacks gate drawing even when hidden. Each output owns
one host and view instance over the shared immutable `Arc` snapshot; there
is no spanning scene. Views receive visibility and plain data and return a
frame description and `wants_frame`, with no egui `Context` or `Ui` access.
Only the host schedules repaints, clamps intervals to at least 100 ms and
revokes the animation lease after two seconds without a new event. Hidden
views retain only the latest snapshot and do not build scene state. The host
also filters widget-generated repaint requests while idle or hidden.

The GL path draws at most 256 instanced cubes, with a single depth buffer,
no shadows or compute, and 1x sampling. Its offscreen target is capped at
1280 × 720. Both targets and vertex/instance buffers share the existing
wallpaper texture budget, including resize overlap. The paint callback adds
its pass to egui's encoder and composites it in egui's pass, with one layer
submission. `args:{renderer:"2d"}` selects the egui_plot train layout;
`renderer:"3d"` restores 3D. Budget refusal also selects the 2D fallback.

Backdrop never writes an image file. Exported stills belong in image-authoring apps
and must be requested over the Bus.

The graph is explicitly a **live queue sample stub**, not gates per hour or
complete history. Follow-up: owner-side read-only `train.history` and
`buildd.history` commands for 24-hour bins from batches/jobs, plus main SHA
and gate stage progress in the queue response. Missing values display `?`;
stage progress is used only when supplied as `stage_progress` in [0,1].
There is no private database or SSH/log reader in backdrop.

PNG, JPEG and WebP share the bounded decoder for both images and slideshows.
The app has neither an overlay snapshot harness nor a headless scene capture
path; `--probe` requires a compositor and is not a headless capture.
