---
title: Backdrop
description: The MixOS desktop background — drawn by its own program on the compositor's background layer, on every output, at native and fractional scale.
---

# Backdrop

Backdrop draws the MixOS desktop background. It is its own program, not
part of the compositor: it puts one surface on the compositor's
background layer for each output, under every window and panel, and
draws it with egui. A crash in it never takes the session down.

What it draws today:

- a **solid colour** per output, or the **session theme's** colour when no
  document names one;
- a **PNG or JPEG image**, centre-cropped to fill each output (the default
  and only fit). Each output may have its own.

Not yet: fit modes other than centre-crop, slideshows, the live folder view
and preferences UI. The probe (`backdrop --probe`) is still there: it is the
regression gate for the surfaces, scale and input, and draws its own labels.

## The document

`backdrop.mx` (strict data, `schema: 1`), in the app's config directory
(`AppDirs`, `config/backdrop.mx`) unless `--config PATH` names another.

```text
schema: 1
default: { colour: "#1e1e2e" }
outputs: {
  "DP-1": { image: "/usr/share/wallpapers/hills.jpg", fallback: "#000000" },
}
```

- `schema` is required and must be `1`.
- `default` is one wallpaper; without it the session theme's colour fills
  every output.
- Each wallpaper names exactly one of `colour` (`#RRGGBB`, six hex digits) or
  `image` (an absolute path). `fallback` (`#RRGGBB`) goes with an image only:
  it is shown until the image is decoded.
- `outputs` maps an output's name (`DP-1`) to its own wallpaper; an output
  without one takes the default.
- Unknown keys, wrong types, bad colours, relative paths and other shapes
  are refused, naming the field. A bad document at start stops the app.

Command line: `--colour #RRGGBB` or `--image FILE` replaces the document's
**default** for this run (it is not stored). `--service NAME` registers the
Bus name (default `backdrop`).

## Images

- PNG and JPEG only. The file is opened once, without blocking (a FIFO or
  device is refused), and read through a limit of 32 MiB; anything larger is
  refused. The format, the header's dimensions and the pixels all come from
  those same bytes. A header over 16384 pixels a side, or over 32 Mpx, is
  refused before any pixel buffer exists. The decoder's own allocation cap is
  256 MiB.
- Decoding runs on one worker thread, one image at a time. The kept image is
  downscaled so its longest side is at most 3840 pixels.
- At most four distinct images per document, counted after command-line
  overrides: a `--image` that makes five is refused.
- Every image a set names is decoded again, even one already on screen, so a
  file replaced at the same path is picked up by the next set.
- A set applies only once all its images are decoded. A missing, corrupt,
  oversize or unsupported image refuses the whole change: the last applied
  backdrop stays on screen, and the reason is in `backdrop.state`. Images being
  decoded for a refused or superseded set are dropped at once.

### Memory budgets

Two aggregate budgets, both enforced; the per-image limits above are not
totals.

| Budget | Limit | Worst case |
|---|---|---|
| Decoded RGBA held by the controller (applied and candidate images together) | 512 MiB (`RESIDENT_BUDGET`) | a decode that would take the total over it is refused (`IMAGE`); the last applied images stay until a whole candidate set applies |
| Accounted wallpaper texture bytes, across all outputs, overlap included (`TEXTURE_BUDGET`) | 512 MiB | four full-size outputs swapping (four old and four new textures) |

A full-size image is 3840 × 3840 × 4 bytes = 56.25 MiB, the largest kept. Four
applied plus four candidates is 450 MiB, which fits the 512 MiB resident budget.
Textures are accounted as the bytes of the new size reserved while the old is
still held, so a swap is counted in full. When bytes are released, live outputs
are woken once so one that was starved draws again; nothing wakes when nothing
is freed.

- A context owns at most one image texture. Replacing it reserves the new size
  while the old is still held, so the overlap is counted; a texture that does
  not fit leaves that output on its fallback colour, with the reason in its
  `error`. Removing an output, showing a solid colour, or a change of image
  releases its bytes.
- A texture upload copies the decoded image once (a transient copy of at most
  one image, on the CPU).

These account for wallpaper data, rather than total process RSS or GPU-driver
memory. Compressed input, decoder allocations, RGBA conversion and resizing
have their own transient bounds; the renderer also owns resources beyond the
wallpaper textures.

## The Bus

Service `backdrop` (or `--service`). Verbs (registered in
`docs/docs/spec/bus/verbs.conf.mix`):

| Verb | Class | Body | Reply |
|---|---|---|---|
| `backdrop.ping` | query | `{}` | `{schema, service}` |
| `backdrop.state` | query | `{}` | the state below |
| `backdrop.set` | intent | a complete document (above) | the change, once applied |

`backdrop.set` replaces the whole document; it does not patch it. A valid
set waits for its images, is stored in the document's file (atomically,
through `config::atomic`), and then applies. Its reply is `rc 0` with
`{requested, active, config_file}`. A refusal is `rc 10` with
`{error_code, message}`:

- `ARGUMENT`: the body is not a valid document (nothing changed);
- `IMAGE`: an image did not decode (nothing changed);
- `STORE`: the document could not be stored (nothing changed), or it was
  stored but its durability is not confirmed (applied; the reply says so);
- `SUPERSEDED`: a newer set replaced this one before it applied.

`backdrop.state` answers with no output on screen:
`{schema, service, requested, active, pending, error, config, config_file,
outputs}`. `requested` and `active` are revisions: `requested` numbers every request
made, and `active` is the revision of the accepted configuration. `pending` is
the revision still decoding, if any. `error` is the last refused change's
reason. `outputs` lists each output drawn so far, with its size, scale, what it
shows (`theme`, `colour`, `image` or `fallback`), its `error`, and
`presented_revision`: the revision that output last drew. Use that field to see
what is on screen; `active` alone does not mean every output has presented it yet.

Every applied set is one run of `backdrop.set` and is published on
`backdrop.command.executed` with origin `bus`. The command line's default is
not a run: it is the start state.

## Cost

It costs nothing while nothing changes. A static colour or image draws once
per output and then sleeps; a set, a decode, a change of output or scale wakes
it through egui's repaint request.

## Start-up and failure

- The Bus registration must succeed: if the broker refuses or is unreachable,
  backdrop exits with the reason rather than running a second, unregistered
  wallpaper.
- An output that goes away is released (`LayerProgram::removed`): its context,
  its texture and its status.

## Known gaps

- The theme's colour is taken from the installed style's panel fill, not a
  named design token. Naming the desktop token is a follow-up.
- Overlapping sets (one still decoding when the next arrives) are exercised by
  the controller's unit tests; the Mix gate sends its sets one after another.

## Running it

Run it inside a MixOS session: `backdrop`, or `backdrop --colour #1e1e2e`,
or `backdrop --image /abs/path.jpg --service backdrop-test`.

## Source layout

The source is
[`apps/backdrop`](https://github.com/markc/mixos/tree/main/apps/backdrop):

- `src/main.rs`: command line, the start-up document, the layer loop;
- `src/control.rs`: the Bus controller and its one command, `backdrop.set`;
- `src/program.rs`: the layer program, which only draws a published view;
- `src/probe.rs`: `--probe`, the regression probe (JSON lines on stdout);
- `crates/wallpaper`: the engine: the document, colour, fit, decode limits
  and revisions. Pure data, no Bus and no window.

## Tests

Building and the cargo tests are not in this tree's checks yet; the cargo
gate runs on a build worker. `cargo build -p backdrop` enables toolkit's
`layer` feature.

- `tests/desktop/wallpaper_gate.mix` runs the wallpaper on a private broker and
  a private nested compd, takes screenshots of a solid colour and a PNG
  (`share/brand`'s favicon), and checks `backdrop.set` and `backdrop.state`: a
  refused image keeps the last good pixels and revision, rapid sets apply in
  order, a non-document is refused, the accepted document is stored, and the
  process is idle. Run it from the checkout; `--image-fixture PATH` names a
  different PNG and `--shot-out DIR` keeps the PPMs.
- `tests/desktop/backdrop_gate.mix` is the `--probe` regression gate: input,
  paint and occlusion. It runs the probe on a private nested compd and
  checks the band, the paint, the screen, click delivery and occlusion:
  `mix tests/desktop/backdrop_gate.mix --bin <compd> --config-file tests/desktop/smoke-settings.json`.
  On a build worker add `--software --private-bus --noded <noded>`.
- Unit tests (`cargo test -p wallpaper`, `cargo test -p backdrop`) cover the
  document, colour, fit, decoder limits (including an oversized header and a
  FIFO), revisions, candidate and active images, the texture budget and the
  verb registry.
