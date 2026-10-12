# backdrop

The MixOS desktop background: an egui program on the compositor's
Background layer, one surface per output, drawn with `toolkit::layer`.

It will draw a colour, an image or a slideshow, with an optional live
folder overlay (the files of a folder as icons, media as thumbnails).
Today it is the first stage: a probe that draws the session theme on every
output and counts pointer clicks, proving the layer surfaces, the GL path,
fractional scale and input. Each fact a test needs is a JSON line on
stdout: `drawn` after a pass at a new size or scale, `click` for each
primary press.

Run it inside a MixOS session: `backdrop`.

Build and test:

- `cargo build -p backdrop` (it enables toolkit's `layer` feature);
- `mix tests/desktop/backdrop_gate.mix --bin <compd> --config-file tests/desktop/smoke-settings.json`
  runs it on a private nested compd and checks the band, the paint, the
  screen, click delivery and occlusion. On a build worker add
  `--software --private-bus --noded <noded>`.
