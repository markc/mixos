# backdrop

The MixOS desktop background: an egui program on the compositor's background layer, one surface per output.

Documentation: <https://mixos.dev/backdrop/> — source [`docs/backdrop/README.md`](../../docs/backdrop/README.md).

Stage 1 live views (session selection, all outputs):

- `backdrop.view.set {view:"train", args?:{train_service, buildd_service}}`
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
Compositor frame callbacks gate drawing even when hidden.

The graph is explicitly a **live queue sample stub**, not gates per hour or
complete history. Follow-up: owner-side read-only `train.history` and
`buildd.history` commands for 24-hour bins from batches/jobs, plus main SHA
and gate stage progress in the queue response. Missing values display `?`;
stage progress is used only when supplied as `stage_progress` in [0,1].
There is no private database or SSH/log reader in backdrop.

PNG, JPEG and WebP share the bounded decoder for both images and slideshows.
The app has neither an overlay snapshot harness nor a headless scene capture
path; `--probe` requires a compositor and is not a headless capture.
