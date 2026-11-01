# backdrop

The MixOS desktop background: an egui program on the compositor's background layer, one surface per output.

Documentation: <https://mixos.dev/backdrop/> — source [`docs/backdrop/README.md`](../../docs/backdrop/README.md).

Stage 1 live views (session selection, all outputs):

- `backdrop.view.set {view:"train", args?:{train_service, buildd_service, renderer, zoom}}`
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

The header uses `train.queue.main` (origin/main as last fetched by the train).
`train.gate` supplies stage/stages, the last stage label and a red failure
state; stale replies from a previous batch cannot move the current gate.
Gate reads run no faster than every 15 seconds and only while a batch is
gating and at least one output's train view is visible. Visibility resumption
wakes the reader without bypassing that rate limit.

The plot uses `train.history`: landings in epoch-aligned hourly buckets and
elapsed minutes per completed batch, with reused gates shown as zero minutes.
Incomplete batches are omitted from duration points. Each series has at most
600 points; long batch series are sampled evenly, preserving both ends.
`args:{zoom:"1h"}`, `"24h"` (default), or `"7d"` request 1, 24 or 168 hours.
History refreshes every five minutes, after `train.landed`/`train.rejected`,
and when the requested service or zoom changes. Missing reads are labelled
unavailable; there are no synthetic queue samples, private databases or SSH
log readers inside backdrop.

To try a built binary on a desktop without installing gold, start it in one
terminal (replace `backdrop` with the path to that binary), then select the
train from another terminal on the same Bus:

```text
backdrop --service backdrop-train
mix -c 'send backdrop-train backdrop.view.set body="{\"view\":\"train\",\"args\":{\"zoom\":\"24h\"}}" timeout=5; print($result)'
```

The new service registers independently and maps another Background surface;
it does not replace or stop the running wallpaper process. View selection
does not write the shared wallpaper document. On a desk, also supply the
hub's mesh service names in `train_service` and `buildd_service`. No new flags
are required. Stopping the test process removes its surfaces.

PNG, JPEG and WebP share the bounded decoder for both images and slideshows.
The app has neither an overlay snapshot harness nor a headless scene capture
path; `--probe` requires a compositor and is not a headless capture.
