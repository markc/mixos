# S8a migration map: followed settings onto `Look`

Working note for S8a. Deleted when S8a ends. It maps every read of the
followed profile in the shells and in Prefs to its key-space v2 view.

Design: `_doc/2026-10-31-settings-key-space-v2.md` §1.4 (delivery), §5.1
(topics), §6 (S8a row). Today's follower is `libs/settings/src/follow.rs`
(`Followed::Look` has eight members). `Look` is S4b's, not yet pushed.

## 1. What a shell reads today

Every plain-Bus shell follows the profile the same way: `Follower` (a
summary hint, then `settings.get {view: summary}`), then a `Followed::Look`
handled by `looked()`. The fields it uses:

| Field of `Followed::Look` | Type (v1) | Used by | What the shell does with it |
|---|---|---|---|
| `appearance` | `settings::Appearance` (v1) | all 8 shells | `session_look::follow` (theme: scheme, style, mode, contrast, framing, menus, band, overlay); kept as `self.look` |
| `custom_source` | `bool` | all 8 shells | selects the design: `follow(look, custom_source, file_theme)`; kept as `self.custom_source` |
| `ui` | `CommonUi` (`density`, `text_scale`, `reduced_motion`) | all 8 shells | `self.common_ui`, then `Theme::with_ui` (the density and motion tokens); term also calls `set_text_scale(text_scale)` |
| `keymaps` | `keymap::Keymaps` | dopus, ced, email, monitor, screenshot, workshop (not term) | `keymaps.get(APP)` into `set_keymap` |
| `terminal` | `terminal::Terminal` | term | `terminal.font_family(&appearance)` and `apply_terminal` |
| `email` | `email::Email` | email | `state.set_settings(*email)` |
| `ced` | `content::Ced` | ced | `app.set_editor_style(ced.editor)` and `editor_palette()` |
| `revision` | `u64` | none (the follower keeps the revision) | not read by any shell |

Shell by shell (`looked()` in each, destructuring with `..` for the rest):

- **dopus** (`apps/dopus/src/shell.rs:522-533`): appearance, ui, custom_source, keymaps.
- **email** (`apps/email/src/shell.rs:2216-2228`): appearance, ui, custom_source, keymaps, email. Also matches `Followed::Unchanged` (settings_unchanged) and `Missing`.
- **ced** (`apps/ced/src/shell.rs:606-620`): appearance, ui, custom_source, keymaps, ced.
- **monitor** (`apps/monitor/src/shell.rs:572-583`): appearance, ui, custom_source, keymaps.
- **screenshot** (`apps/screenshot/src/shell.rs:622-635`): appearance, ui, custom_source, keymaps.
- **term** (`apps/term/src/shell.rs:465-482`): appearance, ui, custom_source, terminal. Also `font.rs:608` (`usable` on Look or Unchanged). Term does not read keymaps.
- **workshop** (`apps/workshop/src/shell.rs:1071-1085`): appearance, ui, custom_source, keymaps.
- **prefs**: does not use `Followed`. Its `Appearance` (`apps/prefs/crates/preferences/src/appearance.rs:1081, 1466-1483, 972-979`) sends `settings.get {view: summary}` itself and reads the `Summary` directly: `summary.appearance` (via `Look::from_stored`), `summary.ui`, `summary.keymaps`, `summary.terminal`, `summary.ced`, `summary.email`, `summary.custom_source`, `summary.incarnation`, `summary.revision`, `summary.appearance.font_weight` and `font_size` (interface font). Its own `Look` (`appearance.rs:82-100`) mirrors the v1 appearance plus `text_scale`, `density`, `reduced_motion`.

Prefs also reads the look for its own preview in `apps/prefs/src/shell.rs:201-203, 593-603, 627-629, 676-677` (`custom_source`, `keymaps.get(APP)`, `Theme::for_context`, `framed`, `with_menus`, `with_band`, and `with_ui`).

## 2. The v1 fields and their v2 targets

Each v1 read maps to a view already on main unless marked S4b. The
`Followed` shape change is the S4b `Look` view with the follower on
`settingsd.desktop.look.<profile>`.

| v1 read | v2 target | Status |
|---|---|---|
| `appearance.scheme`, `mode`, `contrast`, `style` | `view::Appearance` (`mode`, `scheme`, `style`, `contrast`) | on main; the follower must deliver it (S4b `Look`) |
| `appearance.decorations`, `caption_side` (`captions`), `window_style`, `menu_style`, `menu_side`, `actions_place`, `actions_align`, `title_align` | `view::Window::framing` (`Framing`: decorations, caption_side, style, menu_style, menu_side, actions_place, actions_align, title_align) | on main; keys move `appearance.*` to `window.*` (§1.5 #5) |
| `appearance.font_family`, `mono_family`, `font_weight`, `font_size`, `icon_stroke` | `view::Appearance` (the same five fields) | on main; the follower must deliver them (S4b `Look`) |
| `appearance.accent` (v1 sidecar), `window_bg`, `container_bg`, `text_tint`, `control_tint` | `view::Appearance::accent`, `window_bg`, `container_bg`, `text_tint`, `control_tint` | on main as fields; the derived accent is S4b (sealed in `Effective`) |
| `appearance.source` (always `None` in the summary) and `custom_source` | `custom_source` stays a follower flag; `view::Appearance::source` | on main; the flag keeps its meaning |
| `appearance.menu_style` and friends via `session_look::band` | `view::Window::framing` | on main |
| `ui.density` (f64, 0.85 / 1.0 / 1.2 today) | `view::Appearance::density` (`Density`: Compact, Comfortable, Spacious) | on main; the density step replaces the multiplier (F3). `Look` exposes the step; no consumer reads the number |
| `ui.text_scale` | `view::Accessibility::text_scale` | on main |
| `ui.reduced_motion` | `view::Accessibility::reduced_motion` | on main |
| `keymaps` (per app) | `view::Keymaps` (`keymaps.*`, §5.1 topic `keymaps`) | on main as a view; the follower must deliver it on its own topic, not `Look` |
| `terminal`, `email`, `ced` | `view::AppDomains` (`terminal`, `email`, `ced` members), keys `terminal.*`, `email.*`, `ced.*` (§5.1 topics `terminal`, `email`, `ced`) | on main; the follower must deliver them on their own topics |
| `revision`, `incarnation` | the follower's fence; `Look` identity | S4b |
| `source_digest` | `Look` identity | S4b |

## 3. What can move now

The ui fields (`ui.text_scale`, `ui.reduced_motion`, `ui.density`) and the
framing fields are already typed on main, but they arrive inside v1
`CommonUi` and v1 `Appearance` from `Followed::Look`. Reading them from the
views needs the view built from `Values`, which the follower does not
deliver yet. So no read can move to a view before S4b's `Look` and the
follower change land, except where a consumer builds the view from a value
it already holds (none of the eight shells does).

Blocked on S4b (the `Followed` shape change):

1. `Followed::Look` drops `ui`, `appearance`, `custom_source`, `keymaps`, `terminal`, `email`, `ced` in favour of the `Look` view plus per-prefix keymap, terminal, email and ced views.
2. `toolkit::session_look::follow(&Appearance, custom_source, &Theme)` takes `&Look`; the framing reads move to `view::Window::framing`, and the `band`, `menus` and `overlay` builders read from it.
3. `theme::Theme::with_ui(CommonUi)` takes the `Look`'s accessibility and density step.
4. Prefs' `Look` (`preferences/src/appearance.rs:82`) is rebuilt from `view::Look` in S8c or stage 5, not in S8a.

## 4. Shared reads in the toolkit

- `libs/toolkit/src/session_look.rs`: `theme`, `theme_or`, `follow`, `band` (all read `Appearance` v1 fields).
- `libs/toolkit/src/theme.rs:144, 485-490`: the `ui: CommonUi` member and `with_ui` / `ui()`.
- `libs/toolkit/src/ui_scale.rs:4, 18, 60`: `CommonUi` re-export, `clamped`, `tokens`.
- `libs/toolkit/src/style.rs:493`: `with_ui(crate::ui_scale::CommonUi {...})`.

## 5. Plan

- Now: this map. No code moves until S4b pushes `Look`.
- After S4b lands (rebase `work/s8a-apps` onto it): port the eight shells in one pattern, `looked()` destructuring `look` (and `keymaps`, `terminal`, `email`, `ced` only where the shell reads them today).
- The test that proves no drift: each shell's existing look tests (the `custom_source` and `revision` tests in `email/src/shell.rs:2588-2620`, the term font test in `term/src/font.rs:585-620`) stay green.
