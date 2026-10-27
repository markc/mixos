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
omission rule independently of the stored request.

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

Action Bar hosting shares the compact-menu registration agreement:
`drive::set_service` must match the app's own drive, and decorations must be
SSD. The compositor uses the same scene-host registry snapshot and exact
`dev.mixos.NAME` to registered `NAME` mapping. Disconnection removes hosting
data. CSD or an unregistered drive keeps the app's bar.

Apps expose a revisioned `bar` in `<app>.ui.titlebar.form` only while hosting
is possible. Toolkit coalesces item changes for 100 ms and schedules the
trailing update. The scene-host worker reads the property at most four times
per second while titlebar placement is selected, through its existing Bus
client. It retains the last good bar on read failure, fences replies by
connection generation and removes departed services' bars. No new topic,
connection or registry is introduced.

Pointer actions carry the bar revision and index; stale presses are refused.
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
