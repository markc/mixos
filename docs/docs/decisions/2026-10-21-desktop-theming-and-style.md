---
title: Decision — desktop theming and style
description: Every MixOS egui application takes its whole look from one install call and reads it back from the context; every style token and chrome colour is readable and tunable over the Bus for the session, one application or one window, and the terminal's colours for the session, under any design source; later tokens are data, never a migration; the compositor's title bars follow each window's look; the scroll bar, the form row and the fields are toolkit widgets; a gate refuses literals and raw widgets in applications.
---

# Decision — desktop theming and style

Status: accepted 2026-10-21. Amends [egui applications](2026-10-09-egui-applications.md),
[style tokens as data](2026-10-10-style-tokens-as-data.md),
[scheme and style axes](2026-10-10-scheme-and-style-axes.md),
[the chrome family](2026-10-09-chrome-family-and-schemes.md),
[session icon stroke and type](2026-10-14-session-icon-stroke-and-type.md),
[settings authority](2026-10-07-settings-authority.md) and
[settings summary reads](2026-10-11-settings-summary-reads.md).
Settings contract 0.9.0 (additive), store format 4.

## The decision

- **One look per process.** An application installs its theme once with
  `toolkit::install` and reads every colour, length, radius, stroke and
  font back from the egui context. Nothing is threaded through view
  functions; no application sets an egui style, visuals or fonts itself.
- **One canonical icon stroke.** Desktop icons use the context's default
  stroke of 1.5 on the 24-unit icon grid. A fixture that previously passed
  2.0 explicitly moves to that canonical default. The existing
  `appearance.icon_stroke` choice keeps its contract and still owns an
  explicit session override; views have no independent stroke choice.
- **Every painted or interacted number is a token, and later tokens are
  data.** The style family's forty original tokens are the required set;
  every later token is defaulted from an append-only compatibility table
  when a design omits it, through an immutable fallback rule that covers
  every authored style name, so every sealed profile keeps compiling and
  adding a token never needs a store migration. There are no read-only
  values: the behaviour values (resize edges, scrub rates, hit tolerance,
  menu scroll speed, double-click time, the icon mask) are tokens with
  floors that keep the interaction possible.
- **Applications add no literal lengths and no literal colours.**
- **Session tunables.** The profile's `tune` maps `style.<token>` or
  `chrome.<role>` to a value, validated against a registry the design
  crate publishes and `settings.describe` lists; `apps.<id>.tune` does the
  same for one application; `view.look.preview` does it for one window.
  Style and chrome tunes therefore have session, application and window
  scope. Relations between tokens are constraints validated on the final
  layered values and refused with both sides named. Unknown and retired
  names in a stored record are kept and reported, never dropped; aliases
  are canonicalised within each layer. The design is never patched.
- **Resolution order:** session axes, the application's own axis
  override, the window's choice; the profile's pinned design source
  compiled with its defaults; the legacy session choices (icon stroke,
  fonts); session tune; application tune; window preview; then
  accessibility (`ui.reduced_motion` zeroes animation). Every value
  reports its source and what it shadows. Legacy choices own their
  targets.
- **Every application compiles the profile's pinned design source.** When
  the profile's source digest differs from the design the binary carries,
  the application fetches the source once with `settings.source`,
  verifies it by digest, caches it per user and compiles it; until it
  arrives the embedded design is drawn and reported. Custom design sources
  are therefore ordinary profiles: the session look, tunes and previews
  apply to them.
- **Tuned colours are exact and unchecked;** writes warn below 4.5:1 and
  the look report marks them. The field focus border stays and its width
  and colour role are tokens.
- **Read, preview and choose over the Bus.** Every application answers
  `<app>.look` and publishes `<app>.look.changed`: authority identity, the
  window's look generation, which source was compiled, axes, every token
  and role with value, default, source and shadowed layers, the egui slot
  each role owns, the fonts actually drawn, content regions with the
  palette they draw with. `view.look.preview {tune, expect}` is fenced on
  the authority's incarnation and revision and on the window's look
  generation, bounded, one per window, cleared by every look change except
  the arrival of the window's pinned design source, which keeps it.
  `view.theme.set` is the window's axis choice; older per-application
  theme verbs are deprecated aliases. Writes use `settings.validate`,
  `settings.apply` (fenced; `null` resets), `settings.reset` and
  `prefs.appearance.set`. Undo is the caller's held before-values applied
  as an inverse fenced on the accepted receipt.
- **One scroll bar.** `toolkit::scroll` wraps egui's bar with the style's
  tokens; the handle takes the state's text colour at a resting opacity,
  the track shows only while hot. The document canvas keeps its own bars,
  from tokens.
- **One form row** (`toolkit::form`) and one set of fields.
- **Chrome and content are two scopes.** A light content region draws the
  window's design and overlays with only the mode forced to light; other
  regions inherit the window. **The terminal's colours are a settings
  domain with session scope,** `terminal.colors`: sixteen ANSI entries,
  foreground, background and cursor, each a colour or null for the
  design's default for the session's scheme and mode (the cursor default
  is a policy, invert or a colour); a running program's palette changes
  sit over them until reset; explicit RGB cells are drawn as sent.
- **The compositor's title bars follow each window's look.** A
  server-decorated window's bar is drawn from that window's resolved look,
  read with `<app>.look` and refreshed on `<app>.look.changed`, so a
  window's own scheme choice and its preview show in its title bar. The
  window's application is attributed through the broker's own record of
  the service's connection, never through self-reported identity; a
  window the compositor cannot attribute, or an extra window of a
  multi-window application that carries no toplevel tag, draws the
  persistent session and application look. The wallpaper follows the same
  look.
- **Two new chrome schemes, `breeze` and `adwaita`,** each light and dark,
  with their own styles and terminal colour tables; a design that lacks
  them takes them from the compatibility table.
- **Rollout and rollback.** Every reader ships before settingsd writes the
  new fields; `settingsd downgrade --to 3 --target <previous binary>`
  checks eligibility in process and then with the previous binary itself
  on each candidate, and only then re-seals the store for it, primary
  first; it refuses otherwise, naming the reason.
- **Gates.** Workspace clippy disallows the raw egui widgets, every style
  setter and theme threading by resolved path; legitimate uses outside
  applications carry an `expect` with a reason that fails when stale;
  `tools/style_gate.mix` refuses spacing and colour literals and unlisted
  constants in every application GUI crate, and holds the toolkit to its
  token inventory.

## Why

A desktop is one surface. The tokens, the settings authority and the
toolkit already exist; what leaked was the last mile inside each
application, where a stroke was threaded by hand, a gap was a number and a
scroll bar was whatever egui gave. Closing those leaks once, and letting a
session tune any token over the Bus, means the next hundred style
adjustments are data and a `settings.apply`, not a release and not a
migration.

## Consequences

- Every application changes mechanically once; new knobs never need a
  schema field, a store migration or a Prefs change again.
- Prefs builds its Appearance controls from the registry, for the session
  and per application, each showing the design default, the inherited
  session value and the application's override, and gains a terminal
  Colours group.
- An agent can read exactly what any window draws and why, and change
  any style token or chrome colour for the session, one application or
  one window, and the terminal's colours for the session, under any
  design source.

## Amendment (2026-10-23): removing tokens is a clean break

A token that no code reads any more is deleted outright: from the
declared set, the shipped designs, the compatibility table and its
fixtures. It is not kept as a retired name. A saved design that still
authors a deleted token is refused as an unknown token, so check saved
designs before deleting one. The first use removed the panel tokens left
behind when panel groups became plain sections.
