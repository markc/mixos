---
title: Decision — scene strings until Mix reads Fluent
description: Mix Scenes content (share/scenes) keeps its user-visible English strings in one table per scene library until Mix can read Fluent catalogues; then each table becomes that scene's en catalogue and its lookup reads the catalogue.
---

# Decision — scene strings until Mix reads Fluent

Status: accepted 2026-10-12. Bounds AGENTS.md §4's i18n rule for Mix Scenes content until the gap below is closed.

## The problem

AGENTS.md §4 says user-visible strings come from Fluent catalogues
(`<component>/i18n/<lang>/<component>.ftl`) and that new GUI code does not
hard-code them. Mix Scenes content, the panel, launcher, calendar, notes
and settings scenes in `share/scenes`, is written in Mix, and **Mix has no
Fluent loader**. A scene cannot read a catalogue today. Its strings ("Apps",
the calendar's month names, the launcher's category labels, and the
panel's hover tips) are English literals in Mix.

That is a gap in Mix, not a reason to work around it (AGENTS.md §0).

## The decision

- **Until Mix reads Fluent, a scene library keeps its user-visible strings
  in one table** — the English source — and builds every string through one
  lookup. `share/scenes/lib/models.mix` does this for the panel's hover
  tips: `$TEXT` holds them, `text(key, vars)` fills `{name}` placeholders
  and `counted(n, none, one, many)` chooses the plural form.
- **No new scattered literals.** New user-visible scene strings go into
  such a table, never inline in the model code.
- **The table is the seam.** When Mix gains a Fluent reader, each table
  becomes `share/scenes/i18n/en/<scene-library>.ftl`, keyed exactly as the
  table is, and the lookup reads the session's language with English as
  the fallback. The callers do not change.
- **Older literals** ("Apps" in the panel template, `CATEGORY_LABELS` and
  the month names in `lib/data.mix`) move into tables when their scenes are
  next edited, not as a drive-by.

## Closing the gap

The Mix work is a Fluent reader (parse `.ftl`, resolve a message with
arguments and plural selectors, choose a locale with fallback), added to
`libs/mix` as a builtin or a standard module. This decision is superseded
when that lands and the scene tables are converted.
