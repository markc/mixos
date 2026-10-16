---
title: Toolkit
description: The shared shell for MixOS egui applications, giving every app the same theme, fonts, icons, title bar, menus, chrome components, command registry, Fluent strings and Bus drive verbs.
---

# Toolkit

`toolkit` is the library every MixOS egui application is built on. It is
the shell layer of the
[egui applications decision](../docs/decisions/2026-10-09-egui-applications.md)
(see [AGENTS.md](https://github.com/markc/mixos/blob/main/AGENTS.md) §4.1).
An app keeps its behaviour in a headless engine and its UI state in one
serialisable struct. The toolkit draws the window around that state and
routes every action through one command registry.

It gives each app:

- the resolved [design](../design/README.md) as an egui style, plus the
  embedded Inter and JetBrains Mono faces and the Lucide icons;
- the client-side title bar, menu bars and menus, and the View › Theme
  submenu;
- the chrome components (bars, buttons, panels, tabs, fields, toggles,
  combos, sliders, canvas scrollbars, tooltips, dialogs), drawn to the
  chrome specification in each chrome scheme;
- the command registry, which generates menus, shortcuts, the palette and
  the Bus surface, and records every executed command;
- the drive verbs, the UI-level Bus verbs that every app shares;
- Fluent string catalogues, a key-chord grammar, and thumbnail decoding.

The source is
[`libs/toolkit`](https://github.com/markc/mixos/tree/main/libs/toolkit).

## Who uses it

The apps [term](../term/README.md), [ced](../ced/README.md),
[dopus](../dopus/README.md), [prefs](../prefs/README.md),
[screenshot](../screenshot/README.md), [backdrop](../backdrop/README.md)
and busviewer; the [editor](../editor/README.md) library; and four of
[compd](../compd/README.md)'s crates (`decor`, `ui`, `scene-host`,
`world`). backdrop is the only one that turns on the `layer` feature.

## Starting an app

Call `toolkit::install(ctx, &theme)` once on the egui context. It installs
the image loaders the icons need and applies the theme's style and fonts.
`Theme::load()` reads the shared `theme.conf.mix` from the MixOS etc
directory (`$MIXOS_ETC`, else `$MIXOS/etc`, else `/etc/mixos` for root,
else `$XDG_CONFIG_HOME/mixos`). It falls back to the embedded design, and
says why on stderr, when the file is missing or unusable.

A minimal window, condensed from `tests/snapshot.rs`:

```rust
use toolkit::command::{Command, Registry, always};
use toolkit::{Icon, Strings, Theme, icons};

#[derive(Default)]
struct Demo { refreshed: u32 }

let mut commands = Registry::new();
commands.add(Command {
    id: "bus.refresh",           // stable, never reused once published
    label: "cmd-refresh",        // Fluent key
    menu: Some("menu-bus"),      // Fluent key of the top-level menu
    submenu: None,
    group: 0,
    shortcut: Some(egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::R)),
    icon: Some(Icon::RefreshCw),
    enabled: always,
    run: |s: &mut Demo| s.refreshed += 1,
});

let theme = Theme::load();
toolkit::install(ctx, &theme);
let strings = Strings::new(include_str!("../i18n/en/demo.ftl"));
let stroke = icons::stroke_width(&theme);

// each frame:
let _ = toolkit::titlebar::show(ui, "Demo", Some(Icon::Server), stroke,
                                &commands, &state, &strings);
```

## Main pieces

### Theme and look

- `theme`: `Theme` holds a resolved design together with the context it
  was compiled for (scheme, style, mode, contrast) and its framing
  (server- or client-side decorations, caption side). Build one with
  `Theme::embedded()`, `for_context`, `from_source` (a full design
  document or a selection-only file) or `load()`. Then adjust it with
  `with_choice`, `framed` and `with_overlay`.
- `session_look`: turns the desktop profile's appearance, as
  [settingsd](../settingsd/README.md) holds it, into a `Theme`. It is pure,
  with no Bus and no I/O. An appearance that names a design this build
  does not know leaves the app's session theme in place.
- `theme_menu`: the View › Theme submenu, added with
  `Registry::theme_menu`. It covers the hue schemes (Ocean, Crimson, Stone,
  Forest, Sunset, Mono), the chrome schemes (Pro, Studio, Classic), modes,
  styles and framing. Command ids are `view.theme.<scheme>`,
  `view.mode.*`, `view.style.*`, `view.decorations.*` and
  `view.captions.*`, and each axis can follow the session.
- `style`: maps a hue scheme's design onto egui's `Style`. The module docs
  give the field-by-field mapping table.
- `chrome`: the colours and geometry of the title bar and menus. For the
  chrome schemes (`pro`, `studio`, `classic`) it supplies the whole egui
  style, taken from the design's chrome family. Components read
  `Chrome::style` and `Chrome::metrics` and never ask which scheme they are
  drawing, so a new style needs only design data. See the
  [chrome family decision](../docs/decisions/2026-10-09-chrome-family-and-schemes.md).
- `fonts`: the embedded faces. Weight is a design token: each typography
  role names a weight, and the static Inter file of that weight is the one
  installed. Monospace is JetBrains Mono.
- `icons`: Lucide icons, from unmodified upstream SVGs. The `Icon` enum
  names them. Stroke width comes from the design metric
  `icon.stroke_width`, and colour comes from the widget's text colour at
  draw time. See the
  [icons and type decision](../docs/decisions/2026-10-09-icons-and-type.md).

### Window chrome and components

- `titlebar`: the client-side title bar. From the left it holds the app
  mark, the registry's menus, the centred title, an optional right-hand
  group of controls, and the minimize, maximize and close buttons. Captions
  can be on the left instead. A window opened with `titlebar::viewport` is
  undecorated. Only the free gap between the menus and the buttons drags
  the window. Call `titlebar::edges` last in the frame to add the resize
  zones. Under server-side decorations the row becomes a plain menu bar.
- `menu`: menu bars and menus drawn by the toolkit, not by egui's
  `MenuBar`. One highlight is shared by pointer and keyboard, a menu opens
  on press, and press-drag-release runs an item. `Nav` holds the open menu
  and its highlights as pure data and is tested without egui.
- `bars`, `button`, `panel`, `tabs`, `field`, `toggle`, `combo`, `slider`,
  `canvas`, `tooltip`, `dialog`: the chrome components (options, status
  and tool bars, icon rail, push and icon buttons, dock panel groups,
  document tabs, value, text and search fields, checkboxes and switches,
  combo boxes, sliders, canvas scrollbars, tooltip wording, and modal
  dialogs).

### Commands

- `command`: `Command<S>` is one action, with an id, a label key, a menu
  and submenu, a group, a shortcut, an icon, an `enabled` rule and a `run`
  handler over the app's state `S`. A handler only changes state. Work that
  leaves the process, such as a Bus call or a file write, is queued on the
  state for the app's effect loop. `Registry<S>` provides `add`,
  `add_choice`, `add_toggle`, `execute(id, state, origin)`, `describe`,
  `menus` and `model`, `shortcuts`, `search`, `shortcut_grid` and `rebind`.
  The UI, the command line and Bus verbs all dispatch by id.
- `executed`: every run is recorded with its `Origin` (`menu`, `shortcut`,
  `palette`, `ui`, `bus`, `macro`) and given a per-process sequence number.
  It goes into a bounded `Journal` that counts the runs it drops.
  `Registry::take_executed` hands the records to the app's Bus connection,
  which publishes them on `<app>.command.executed`
  ([decision](../docs/decisions/2026-10-11-executed-commands.md)).
- `keymap`: the chord grammar (`Ctrl`, `Alt`, `Shift`, then one key, joined
  by `+`). `Registry::rebind` applies the person's bindings from the
  desktop profile.

### Drive verbs

`drive` lets an agent do in the window anything a person can.
`drive::install(ctx, app)` turns on AccessKit, keeps each pass's widget
tree, and injects queued pointer and key events through egui's input hook,
the same path real input takes. The app registers these verbs under its own
prefix (`<app>.ui.tree` and so on):

`ui.tree`, `ui.click`, `ui.pointer`, `ui.scroll`, `ui.key`, `ui.type`,
`ui.menu`, `ui.menu.list`, `ui.menu.open`, `ui.menu.close`, `ui.capture`,
`window`, `window.state`

The app forwards each request, with the prefix removed, to
`drive::request` from inside a frame. It sends back whatever `request`, or
`drive::logic` (called once per frame), returns. It holds later commands
while `drive::pending` is true, and calls `drive::finish` before closing.
Reads answer at once. Input verbs run one at a time, one step per pass,
and answer one pass after their last step. At most 32 input requests can
be queued. The [driving guide](../docs/drive.md) shows them in use.

### Strings, thumbnails, layer surfaces

- `strings`: `Strings::new(ftl)` wraps an app's Fluent catalogue
  (`i18n/en/<app>.ftl`). It provides `get`, `with` (with arguments) and
  `has`. A missing key renders as the key itself, so a gap shows on screen
  instead of a blank. The toolkit's own labels, such as window buttons and
  the Theme menu, are in `i18n/en/toolkit.ftl`.
- `thumbnail`: decodes cached thumbnail PNGs within explicit size limits
  on a `Decoder` thread, ready to upload as textures. Making the
  thumbnails is the job of [thumbs](../thumbs/README.md).
- `layer` (feature `layer`): egui programs on wlr-layer-shell surfaces,
  such as wallpapers, desktops and docks. `layer::run(options, program)`
  creates one surface per output (`Outputs::Each`), or a single surface
  the compositor places. Each surface has its own egui context and draws
  through wgpu's GL backend on its `wl_surface`. It follows fractional
  scale and takes pointer, scroll and optional keyboard input. It redraws
  only when something changes, so an idle surface costs nothing.

## Features and dependencies

The default build has no features. `layer` adds smithay-client-toolkit
0.19.2 (the version eframe's winit already links), wayland-backend, wgpu,
egui-wgpu and xkbcommon. Workspace dependencies: `assets`, `config`,
`design`, `settings` (data types only, no Bus) and `thumbs`.

## Assets and licences

`assets/fonts/` holds static Inter (Light, Regular, Medium, SemiBold) and
JetBrains Mono (Light, Regular, Medium) faces with their OFL texts.
`assets/icons/` holds the Lucide SVGs with `LICENSE-lucide.txt`, and
mixos.dev uses the same icons. `assets/licenses.conf.mix` pins each notice
by SHA-256. This includes the notices for egui's own fallback fonts, which
are in `assets/fonts/fallbacks/`.
`scripts/install_licenses.mix` installs them under
`<share>/licenses/toolkit` (default share `/opt/mixos/share`). `--check`
validates only the sources, and `--verify` checks an installed tree. See
the [bundled font licences decision](../docs/decisions/2026-10-13-bundled-font-licences.md).

## Source layout

| Path | What |
|---|---|
| `src/lib.rs` | module list and `install` |
| `src/theme.rs`, `theme_menu.rs`, `session_look.rs`, `style.rs`, `chrome.rs` | the look |
| `src/fonts.rs`, `icons.rs` | embedded faces and Lucide icons |
| `src/titlebar.rs`, `menu.rs` | window chrome |
| `src/bars.rs` … `src/dialog.rs` | chrome components |
| `src/command.rs`, `executed.rs`, `keymap.rs` | commands, executed records, chords |
| `src/drive.rs` | the drive verbs |
| `src/strings.rs`, `thumbnail.rs` | Fluent strings, thumbnail decoding |
| `src/layer.rs`, `src/layer/` | layer-shell surfaces (feature `layer`) |
| `i18n/en/toolkit.ftl` | the toolkit's own strings |
| `assets/`, `scripts/install_licenses.mix` | fonts, icons, licence notices |

## Testing

Unit tests sit in the modules; for example `menu::Nav` is tested as plain
data. The integration tests in `tests/` drive real frames through
`egui_kittest`:

- `components.rs`, `menu.rs`, `titlebar.rs`, `chrome.rs`: component, menu
  and title-bar behaviour under pointer and keyboard;
- `drive.rs`: the drive verbs, requested inside frames the way an app's
  shell requests them;
- `snapshot.rs`, `components_snapshot.rs`: offscreen wgpu snapshots across
  schemes and modes, compared with `tests/snapshots/*.png`. Regenerate them
  with `UPDATE_SNAPSHOTS=1`, and look at the images before committing;
- `spec_renders.rs`: when `CHROME_RENDER_DIR` is set, it renders the
  chrome scenes for an out-of-tree comparison against the chrome
  specification's evidence. When it is unset, it does nothing;
- `asset_licenses_test.mix`: install, drift and verify checks for the
  licence installer.

On an authorised build worker at a pushed commit:

```
cargo test --locked -p toolkit
cargo test --locked -p toolkit --features layer
```
