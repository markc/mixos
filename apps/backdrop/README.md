# backdrop

The MixOS desktop background: an egui program on the compositor's
Background layer, one surface per output, drawn with `toolkit::layer`.

It draws a solid colour, a PNG/JPEG image or a slideshow per output, with
fill, fit, centre, tile and stretch modes. With no document it uses the session theme's colour. Its
document is `backdrop.mx` (strict data, `schema: 1`); its Bus verbs are
`backdrop.ping`, `backdrop.state`, `backdrop.set` and `backdrop.next`. The full format, the
limits and the verbs are in [`docs/backdrop/README.md`](../../docs/backdrop/README.md).

Layout:

- `src/main.rs`: command line, the start-up document, the layer loop;
- `src/control.rs`: the Bus controller, decode worker and slideshow timer;
- `src/program.rs`: the layer program, which only draws a published view;
- `src/probe.rs`: `--probe`, the regression probe (JSON lines on stdout);
- `crates/wallpaper`: the engine: the document, colour, fit, decode limits
  and revisions. Pure data, no Bus and no window.

Run it inside a MixOS session: `backdrop`, or `backdrop --colour #1e1e2e`,
or `backdrop --image /abs/path.jpg --service backdrop-test`.

Build and test (not in this tree's checks yet; the cargo gate runs on
cbc2/cbc3):

- `cargo test -p wallpaper` (engine: strict document, colour, fit, decode
  limits, revisions);
- `cargo test -p backdrop` (command line, registry and verb classes);
- `cargo build -p backdrop` (it enables toolkit's `layer` feature);
- `mix tests/desktop/backdrop_gate.mix --bin <compd> --config-file tests/desktop/smoke-settings.json`
  runs the probe (the gate passes `--probe`) on a private nested compd and
  checks the band, the paint, the screen, click delivery and occlusion. On a
  build worker add `--software --private-bus --noded <noded>`.

The wallpaper's own desktop gate is `tests/desktop/wallpaper_gate.mix`: a
private broker and compd, real colour and PNG screenshots, `backdrop.set` and
`backdrop.state`, a refused image, rapid sets, a non-document, the stored file
and the idle check. Run it from the checkout (`--image-fixture PATH` names a
different PNG, `--shot-out DIR` keeps the PPMs).
