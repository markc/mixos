# MixOS GUI app style guide

---

## 1. The three rules

1. **One look per window context.** Install the theme once per egui
   context (one for a single-window app). Read everything back from the
   context. Thread nothing.
2. **Tokens and widgets, never literals.** A length is a token. A colour is
   a role. A control is a toolkit widget.
3. **The look is on the Bus.** Every app answers `<app>.look`, publishes
   `<app>.look.changed`, runs `view.look.preview` and `view.theme.set`, and
   follows the session's tunables through settingsd without a restart and
   without polling.

## 2. Crates and dependencies

An app is two crates: an engine crate under `apps/<app>/crates/<name>` that
knows nothing about egui, and the egui shell crate `apps/<app>`. The shell
crate's `Cargo.toml` lists, as busviewer's does:

```toml
[dependencies]
eframe.workspace = true      # the window
egui.workspace = true        # labels and layouts the toolkit does not wrap
toolkit.workspace = true     # theme, chrome, widgets, commands, drive verbs
settings.workspace = true    # the session look follower
citizen.workspace = true     # the Bus connection
bus = { workspace = true, features = ["native"] }   # the native client citizen is built on
futures.workspace = true     # the connection's delivery stream
serde_json.workspace = true  # verb bodies

[dev-dependencies]
egui_kittest.workspace = true  # offscreen snapshots and accessibility queries
```

The toolkit does not re-export egui or eframe. The engine crate depends on
none of these; the layering table (`apps/layers.conf.mix`) refuses egui,
eframe, egui-wgpu, winit and rfd in its closure. A nested GUI crate (a
renderer that draws into a texture) is held to this guide and its gate like
the shell.

## 3. The minimal app

The full example is `libs/toolkit/examples/guide_app.rs`. Its shape, which
every shipped app shares:

- **Start:** `citizen::start_with_topics(app, service, url, topics)` with
  the settings summary topic gives the Bus handle and a stream of
  deliveries; `Theme::load()` gives the theme file or the embedded design;
  `toolkit::install(ctx, &theme)` and `drive::install(ctx, app)` are called
  once in the eframe creator; the app's strings are `Strings::new` over its
  embedded Fluent catalogue.
- **Forwarder:** one thread drains the delivery stream into the shell's
  inbox and calls `ctx.request_repaint()` for every event, so an idle
  window wakes for a settings hint, a Bus command or a disconnection.
- **`App::logic`** (eframe's per-frame hook before drawing) drains the
  inbox: `Connected`, `Changed` and a summary `Topic` hint ask the follower
  for a read; `Disconnected` resets the follower (so the next connection
  reads afresh and gets an identity-bearing look even at an unchanged
  revision), voids retries and clears the authority and any preview;
  `Command` is served (`<app>.look` and `<app>.execute` through the look
  layer, then the shipped drive layer's `request`, whose queued answers come
  back from `drive::logic` each frame); a read's answer goes to `looked`,
  whose returned `next` read is sent exactly as returned; a transient fault
  arms one bounded retry.
- **`install_theme`** is the only place the look changes: the follower's
  inputs for this app (or the theme file) with the window's own choice laid
  over, installed on the context; every call bumps the window's look
  generation and publishes `<app>.look.changed`; it clears a preview,
  except when it is the arrival of the window's pinned source, which keeps
  the preview. Before installing, it checks the window's final token pair
  against the design's constraints; a violating pair is not installed, the
  last valid look stays, and `<app>.look` reports the violation.
- **`App::ui`** draws the title bar on the root `Ui` with
  `toolkit::titlebar::show` (no stroke argument; it reads the context) and
  the content inside `egui::Frame::central_panel`.

**What the lifecycle guarantees.** The first read happens at start; every
later read is caused by a summary delivery, a connection event or a
retry; a delivery is never trusted on its own; an older revision never
replaces a newer one (the follower checks); without settingsd the theme
file stands. There is no timer.

**Which design the app draws.** The profile's pinned design source. When
the summary's source digest (BLAKE3 of the source bytes,
`settings::source_digest`) equals that of the design the binary carries,
the app compiles that; otherwise it fetches the pinned source once with
`settings.source`, verifies it with the same digest, caches it per user in
a private directory it has checked it owns, and compiles it for the
window's own axes, drawing the embedded design until it arrives. A reply
activates only for a window that still wants that digest, resolving the
window's current inputs and keeping its current preview; a late or
superseded one only fills the cache. One fetched source serves every
window and light region of the app without another fetch. `<app>.look`
reports which was compiled and the acquisition state. A profile with its
own design source is therefore an ordinary profile: the session look,
tunes and previews apply. An app connects to the broker over the local
Unix endpoint when one is named, which is what lets the compositor
attribute its windows; over TCP it works the same but its windows draw the
session look under server decorations.

**Reading the look in view code.**

| need | read |
|---|---|
| a colour | `Chrome::of(ui.ctx()).palette.<role>` |
| a chrome length | `Chrome::of(ui.ctx()).metrics.<name>` |
| a style form (cards or strips, switch or checkbox, bevels) | `Chrome::of(ui.ctx()).style.<token>` |
| the icon stroke | `toolkit::icons::stroke_of(ui.ctx())`, or just use `IconButton` |
| a gap | `toolkit::space::of(ui.ctx()).md` (xs, sm, md, lg, xl) |
| field radius, padding and focus | `toolkit::field::Style::of(ui.ctx())`, or just use the field widgets |
| dim or faint text | `toolkit::text::dim(s)`, `toolkit::text::faint(s)`, `toolkit::text::role(s, role)` |
| a behaviour value (double-click time, scrub rate, hit tolerance) | `Chrome::of(ui.ctx()).style.<token>` |

Never pass any of these into a function as a parameter.

Desktop icons share the context's canonical 1.5 stroke on the 24-unit
icon grid. Use `icons::image_of` or the toolkit icon widgets. Older test
fixtures that passed 2.0 explicitly use the canonical default too. The
existing `appearance.icon_stroke` session choice remains supported.

Local style overlays are fallible: handle `Theme::with_overlay` and
`Theme::with_choice` errors before installing a look. A refused overlay
must leave the installed look intact. `canvas_dot_pitch` accepts 4 to
1000 points, preventing zero or impractically dense drawing loops.
For a colour tunable with a numeric range, the bounds apply to each
normalised colour component. The value itself remains a hexadecimal
colour; `icon_mask` requires every channel to be at least 0.1.

```rust
fn view(ui: &mut egui::Ui, engine: &Engine) {
    let space = toolkit::space::of(ui.ctx());
    let p = Chrome::of(ui.ctx()).palette;
    ui.label(format!("Count: {}", engine.count));
    ui.add_space(space.md);
    ui.add(toolkit::button::IconButton::new(Icon::Settings).tooltip("Settings"));
    ui.add(toolkit::text::dim("dim text"));
    let _ = p.accent;                               // read where you paint
}
```

## 4. Which toolkit widget for each need

| need | use | not |
|---|---|---|
| a text input, one line | `field::text(ui, &mut s, hint, width)` | `egui::TextEdit::singleline` |
| a search box | `field::search(...)` | a text edit with an icon beside it |
| a multi-line editor | `field::multiline(...)` | `egui::TextEdit::multiline` |
| a number with a unit, scrubbing, arithmetic | `field::ValueField::new(&mut v, range).unit("px")` | `egui::DragValue`, `egui::Slider` |
| a labelled form | `form::Form` rows (§5) | `egui::Grid` |
| a default or secondary action | `PushButton::primary(label)`, `PushButton::secondary(label)` | `ui.button`, `egui::Button::new` |
| an icon button | `IconButton::new(Icon::X).tooltip(...)` | `ImageButton` |
| a link with an icon | `button::Link` | a hyperlink label |
| an on/off | `toggle::Toggle` | `ui.checkbox` |
| a list of choices | `combo::show` | `egui::ComboBox` |
| a slider or a slider row | `slider::Slider`, `slider::row` | `egui::Slider` |
| a scrolling region | `scroll::Area::vertical(id).show(ui, ...)` | `egui::ScrollArea` (any constructor) |
| a table | `datatable::DataTable` | `egui_extras::TableBuilder` |
| a dock group with tabs | `panel::group`, `panel::group_with_actions` | a `Frame` with a header |
| a section heading | `panel::section_label` | a bold label |
| a bar | `bars::options`, `bars::status`, `bars::tools`, `bars::rail` | a `TopBottomPanel` with a fill |
| a hairline or divider | `bars::hairline`, `bars::divider` | `ui.separator()` |
| a modal | `dialog::Dialog` with `Choice`s | `egui::Window` |
| a tooltip | `.tooltip(...)`, `tooltip::text` | ad-hoc `on_hover_text` wording |
| menus, context menus, the Theme menu | the registry's `menu_bar`, `menus`, `context_menu`, `theme_menu::register` | `egui::menu`, a theme verb |
| a selectable row | `toolkit::selectable` | `selectable_label` |
| dim, faint or coloured text | `toolkit::text::{dim, faint, role}` | `RichText::color`, `::font`, `::size` |
| rendered markdown | `markdown::View` | your own layout |
| a document canvas with scroll bars | `canvas::surround`, `canvas::scrollbars` | your own |
| a prose region that may be light | `content::surface(ui, &content, ...)` | `ui.set_style`, `style_mut` |
| a thumbnail | `thumbnail::Decoder` | your own decoder |

A plain `ui.label` is fine. **When no widget fits,** write it in the
toolkit, not the app: read its colours and lengths from `Chrome`, name the
spec section, snapshot it, then use it.

## 5. The form row

Labels outside the field in one right-aligned column as wide as the widest
label the form can show; `form_label_gap` to the field; trailing controls
right to left `form_trailing_gap` apart; the field takes the remainder;
rows `form_row_gap` apart; the label's id is the field's accessible name.

```rust
let form = toolkit::form::Form::new(ui, &["From", "To", "Cc", "Subject"]);
form.row(ui, "To", |ui, label_id| {
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        if ui.add(IconButton::new(Icon::Copy).tooltip(cc_hint)).clicked() { /* ... */ }
        field::text(ui, &mut to, "", ui.available_width()).labelled_by(label_id)
    })
});
```

A row taller than a field uses `row_of` with its height.

## 6. Spacing

| token | shipped | use for |
|---|---|---|
| `xs` | 2 | hairline clearance, icon to its text |
| `sm` | 4 | between related controls in a row |
| `md` | 8 | between groups in a row, panel inner padding |
| `lg` | 16 | between sections |
| `xl` | 24 | one control height; dialog outer margin |

`ui.add_space(space.md)` is the approved form; the gate refuses a literal
argument, not the method. An app never sets `item_spacing`,
`button_padding` or `interact_size`, and never calls `spacing_mut`,
`visuals_mut`, `style_mut` or any `Context` style setter.

A `const` is allowed only for content geometry (a column width, a thumbnail
edge, a glyph size in a custom painter), named in capitals, commented, and
listed in the app's `style-allow.conf.mix` with the reason.

## 7. Scroll areas

```rust
toolkit::scroll::Area::vertical("messages")      // the id salt: stable, unique in the window
    .max_height(h)
    .stick_to_bottom(true)
    .show(ui, |ui| { /* rows */ });
```

Also `Area::horizontal`, `Area::both`, `.min_scrolled_height`,
`.scroll_to(rect)`. It wraps egui's scroll area, so drag, page click,
wheel, keyboard, nesting and two-axis behaviour are egui's; the toolkit
supplies the defaults (no auto-shrink) and the style's bar: thin floating
bars 4 pt at rest and 10 pt when hot, or solid 6 pt in a bevelled style,
the handle in the state's text colour at the resting opacity, the track
only while hot, all from tokens. Clippy refuses every `egui::ScrollArea`
constructor by path.

## 8. Icons

Lucide, from the toolkit's `Icon` enum. A new icon is added to the toolkit
(copy its SVG from the pinned Lucide release into
`libs/toolkit/assets/icons/`, list it in `icons.rs`), never embedded in the
app. Icons take the stroke installed on the context and the colour of the
text around them. Sizes: `IconButton` is the `icon_button` token (28) with
the icon at 52 %; an inline icon uses `space.lg` or an allowlisted content
constant through `paint_icon`.

## 9. Menus, context menus, the palette, the Theme menu

Every action is a registry command with a stable id, a label key, a menu
path, an optional default chord and typed parameters. The toolkit generates
the menu bar, menus, context menus and palette from the registry. Right-click
rows are 24 pt and flush. The View › Theme submenu is
`toolkit::theme_menu::register`: its rows are commands, and
`view.theme.set {scheme?, style?, mode?, decorations?, captions?}` is the
same choice for agents. An app writes no theme verb of its own. The
window's choice reaches the compositor too: a server-decorated window's
title bar is drawn from the window's own look, attributed through the
broker's record of the app's connection. A multi-window app's extra
windows are matched by their toplevel tag once the toolkit sets one; until
then they draw the session look under server decorations.

## 10. Dialogs

`dialog::Dialog::new(id, title)` with `Choice`s carrying a `Role` so button
order, Enter and Esc follow the platform rule. Wide dialogs use `.width`;
resizable ones `.resizable(default, min)`. A destructive prompt opens with
its default focused and ringed.

## 11. Content regions

Chrome is what the toolkit paints from `Chrome`. Content is what the person
reads or edits. Content keeps its own colours on purpose, but it is on the
look:

- a prose region drawn with `content::surface` takes the person's content
  settings (padding, font, light). A region with `light: true` draws **the
  installed window's** design, scheme, style and overlays (tunes and
  preview included) with only the mode forced to light; one with
  `light: false` inherits the window as it is;
- an editor palette is built from the region's theme
  (`editor::Palette::from_theme`), never from literals;
- a terminal grid draws the `terminal.colors` settings over the design's
  colour table for the session's scheme and mode, with the running
  program's palette changes over them until it resets them, and explicit
  RGB cells as sent.

`<app>.look` reports every content region under `regions`: a prose or
editor region with the palette and font it draws with; a terminal region
with its design defaults, the settings entries, the runtime overrides in
force, the effective palette and its cursor rule.

## 12. What is forbidden in an app (and what checks it)

Clippy, by resolved path (an alias or a `use` cannot dodge it), and
`tools/style_gate.mix`, for literals and shapes, refuse these in every
egui-dependent crate under `apps/`, tests excluded:

| refused | use instead |
|---|---|
| `egui::TextEdit::singleline`, `::multiline` | `field::text`, `field::multiline`, `field::search` |
| `egui::ScrollArea::vertical`, `::horizontal`, `::both`, `::new` | `toolkit::scroll::Area` |
| `egui::Grid::new` | `toolkit::form`, `datatable` |
| `egui::Ui::button`, `egui::Button::new` | `PushButton`, `IconButton`, `Link` |
| `egui::Ui::checkbox` | `toggle::Toggle` |
| `egui::ComboBox::from_id_salt`, `::from_label` | `combo::show` |
| `egui::Slider::new`, `egui::DragValue::new` | `slider::Slider`, `field::ValueField` |
| `egui::Ui::style_mut`, `set_style`, `spacing_mut`, `visuals_mut`; `egui::Context::set_style`, `set_global_style`, `set_visuals`, `set_fonts`, `global_style_mut`, `all_styles_mut`, `style_mut_of`, `set_style_of` | `toolkit::install`; `content::surface` for a region |
| `egui::RichText::font`, `::size`, `::color` | `toolkit::text::{dim, faint, role}`; the heading style |
| `toolkit::icons::stroke_width`, `Chrome::for_theme`, `style::apply`, `chrome::install` | `icons::stroke_of(ctx)`, `Chrome::of(ctx)`, `toolkit::install` |
| `ui.add_space(<number>)`; `vec2(<number>, <number>)` or `Vec2::splat(<number>)` as spacing | the `space` tokens |
| any assignment to `item_spacing`, `button_padding`, `interact_size` | nothing: the style owns them |
| `Color32::from_rgb`, `from_gray`, `from_hex`, `from_rgba_*`, `WHITE`, `BLACK` | a `Chrome` role |
| a `stroke: f32`, `theme: &Theme`, `chrome: &Chrome` or `palette:` parameter on a view function | read from the context |
| an unlisted `const NAME: f32` | list it with a reason, or use a token |
| `#[allow(...)]` or `#[expect(...)]` on a `disallowed_*` lint inside an app | there is none; use the replacement |
| `selectable_label`, `selectable_value` | `toolkit::selectable` |

Each refusal prints the file, the line and the replacement. The example
app and the snippets in §3 to §7 are the gate's passing fixture; one
adversarial fixture exists per refusal. The gate runs with the other
repository gates and in CI.

## 13. What the app exposes on the Bus for its look

The toolkit provides these; the app lists them in `app.describe` with its
drive verbs and registers them in the verb registry:

- `<app>.look` (query; `{context?}` for apps with several contexts): the
  authority identity (binding, incarnation, revision, source digest, or
  null while settingsd is missing), the window's look `generation`, which
  source was compiled and how it was obtained, the axes with sources
  (`session`, `app`, `window`), every style token and chrome role with
  `value`, `default`, `source` (`design`, `design-default`, `legacy`,
  `session`, `app`, `preview`, `accessibility`) and `shadowed`, each role's
  owning egui slots, each colour's `checked` flag, the fonts actually drawn,
  `tune.unknown`, the preview state, and `regions`.
- `<app>.look.changed` (topic): published on every install with the
  window's context and generation. The compositor follows it for
  server-decorated title bars.
- `view.look.preview` (command, intent): params
  `{tune: {"style.field_radius": 3}, expect: {incarnation, revision, generation}}`
  overlay this window only; `{tune: {}}` clears; refused `stale_preview`
  unless all three fence values match what the window draws now; at most
  4096 bytes; cleared by every look change except the arrival of the
  window's pinned design source, which keeps the preview and re-applies it
  over the refined look. Run through `<app>.execute`, journaled like any
  command.
- `view.theme.set` (command, intent): the window's own axis choice.

To change the session for every app, a caller writes settingsd:
`settings.validate` to try, `settings.apply` with
`"tune.style.field_radius": 4` (fenced on the current revision; `null`
resets; `"tune": null` resets all), `settings.reset`, or Preferences'
`prefs.appearance.set`. Per app: `"apps.<id>.tune.<family>.<name>"`.
Terminal colours, session scope only: `"terminal.colors.ansi.1": "#ff0000"`,
`"terminal.colors.foreground"`, `.background`, `.cursor`. Style and chrome
tunes have session, app and window scope; terminal colours have session
scope. A relation between two tokens (a resize corner at least as large as
the edge) is a constraint checked on the final layered values and refused
with both sides named. Every running app follows within one summary
delivery, under any design source. Undo is the
caller's held before-values applied as an inverse fenced on the accepted
receipt's incarnation and revision; a `conflict` means someone else wrote
in between.

## 14. Snapshots and tests

- Every view has an offscreen snapshot in the app's `tests/`, rendered in at
  least Studio dark and one light scheme. Look at the image.
- Rows are found by accessible name, never by position.
- A registry test checks the verbs the app describes are the ones it
  registers, including `<app>.look` and the `<app>.look.changed` topic.
- A theme-follow test feeds a look delivery with a `tune` and asserts
  `Chrome::of(ctx)` changed; one with `ui.reduced_motion` asserts the
  animation time is zero; one with a stale preview fence asserts refusal;
  one with a pinned source digest that differs asserts the embedded design
  first and the acquired source after; the lifecycle tests the example app
  carries run against the shell.

## 15. Review checklist

- [ ] `toolkit::install` is the only look installer; no view function takes
      a stroke, theme, chrome or palette parameter.
- [ ] The shell follows the example app's lifecycle: forwarder wakes on
      every event, the follower's returned read is the one sent, a
      disconnect resets the follower and clears authority and preview, a
      fault retries once, a differing source digest is acquired.
- [ ] No raw egui widget or style setter from §12; clippy and the gate are
      green with no lint exception inside the app.
- [ ] No spacing literal; every `const` is allowlisted with a reason.
- [ ] No colour literal; coloured text goes through `toolkit::text`.
- [ ] Forms use `toolkit::form`; labels are the fields' accessible names.
- [ ] Scrolling uses `toolkit::scroll::Area` with a stable id salt.
- [ ] Icons come from `Icon`; new ones were added to the toolkit.
- [ ] Every action is a registry command; menus and the Theme menu come
      from the registry; no app theme verb.
- [ ] `<app>.look`, `<app>.look.changed`, `view.look.preview` and
      `view.theme.set` are described and registered.
- [ ] Content regions use `content::surface` and are reported under
      `regions`.
- [ ] Snapshots exist for the changed view in a dark and a light scheme, and
      were looked at.
- [ ] Nothing names a host, an operator path or a reference application.

---
