# Title band layout

Status: accepted. Mark, 2026-10-27.

The menu group, title, Action Bar and window controls each align left, centre
or right. CSD and SSD use `design::title_band::layout`. The app mark belongs
to the menu group. MixOS titles centre by default; mac centres and win11
starts at the left.

| Appearance key | Names | Default |
| --- | --- | --- |
| `menu_side` | left, center, right | left |
| `title_align` | auto, left, center, right | auto |
| `actions_align` | left, center, right | right |
| `actions_place` | row, titlebar | row |
| `caption_side` | auto, left, center, right | auto |

These are named registries in `libs/design`. Unknown plain names survive
storage and resolve as defaults. Defaults are omitted in full and compact
settings, preserving existing profile seals. Caption auto resolves to right
in CSD and the preset's cluster side in SSD. Previously omitted captions
were right in CSD; SSD did not read them. Explicit right remains explicit
right. The appearance portal keeps its older effective right default and
omission rule independently of the stored request. An omitted effective caption
request now reads as `auto`; older projection readers cannot parse `center`.

`layout(width: f32, groups: &[Group]) -> Vec<Rect>` uses logical coordinates.
Each group supplies preferred width, minimum width, alignment, optional
shrink priority and centre reading order. Rectangles contain x and width;
the caller supplies the common band's y and height. Input order is controls,
menu, actions, title, so groups stack from either edge in that order. Centre
reading order is menu, title, actions, controls. The centre pack uses the
whole band's centre if free, otherwise the gap between the side stacks.
Title shrink priority is 0, Action Bar priority 1. Controls and menus are
fixed. Action Bars drop app actions from the left and retain the standard
tail. SSD retains enough frame width for fixed groups and that tail.
The CSD edge groups include the existing inner margins. Its default centred
title retains the previous half-margin offset; an explicit centre uses the
full band's centre. Only this frame's gaps outside all four groups drag.
SSD `auto` reads the preset's title alignment; the client registry records the
same default and a consistency test checks the presets against it.

Action Bar hosting shares the compact-menu registration agreement:
`drive::set_service` must match the app's own drive, and decorations must be
SSD. The compositor uses the same scene-host registry snapshot and exact
`dev.mixos.NAME` to registered `NAME` mapping. Disconnection removes hosting
data. CSD or an unregistered drive keeps the app's bar.

Apps expose a revisioned `bar` in `<app>.ui.titlebar.form` only while hosting
is possible. Toolkit coalesces item changes for 100 ms and schedules the
trailing update. Through the drive, each app's existing Bus publisher emits
`<app>.ui.titlebar.changed {revision}` only on change. The scene-host worker
subscribes before reading once at hosting start, then reads on notifications
stamped by the local broker as that registered service. Changes arriving during
a read coalesce into one follow-up read. A 30-second resync recovers lost
notifications or failed initial reads. It retains the last good bar on read
failure, fences replies by connection generation and reader epoch, cancels
obsolete work and removes departed services' bars. The topic uses the existing
connection and registry.

Pointer actions carry the bar revision and index; revisions are seeded from
the app's startup timestamp so a new process cannot restart the click fence at
zero. Stale presses are refused. Layouts are cached on each window by geometry
revision, size and theme generation; hover changes reuse their geometry.
`comp.window.action {id, generation, command}` uses the same enabled-item
check and scene-host event sink to send `<app>.execute {id}`. Calls are
non-blocking and failures are logged. `comp.props` exposes
`decoration.actions` and each surface's
`action_count`. `ui.titlebar.form` reports requested and effective placement,
alignments, the matched service and hosted items.

Tooltips use the existing compositor tooltip delay and card renderer, with
the session tokens and output clamping. Lucide names and SVGs belong to
`libs/icons`; toolkit loads them through egui and decor rasterises them with
the workspace resvg.

A future group adds an input in each renderer and its rendering and hit
targets; the generic layout function stays unchanged. A future alignment
extends `Alignment`, the named registries and their conversions, placement
logic, Prefs labels and combination tests. Per-app overrides and new styles
are outside this decision.
