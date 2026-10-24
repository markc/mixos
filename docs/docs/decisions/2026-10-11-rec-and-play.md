---
title: Decision — rec and play
description: One command-line tool, recplay, installed as rec and play. play opens anything with the handler the desktop already knows, performs scripts and draws saved state. rec writes what happens on the desktop as an editable Mix script. rec and play are accepted name collisions.
---

# Decision — rec and play

Status: accepted 2026-10-11. Amends [the source layout](2026-10-04-source-layout.md)
(AGENTS.md §3.4).

## The decision

- **One Rust command-line tool, `cli/recplay`, installed as `rec` and
  `play`.** The binary is `recplay`. `rec` and `play` are links to it,
  added at install time in `/opt/mixos/bin`, and it chooses its behaviour
  from the name it was started as. Run as `recplay`, it prints help for both.
- **`play` puts something on the desktop.**
  - A file or URI opens with the handler the desktop already knows. That
    means shared-mime-info for the type, desktop entries' `MimeType=` and
    `mimeapps.list` for the handler. MixOS ships no table of its own and
    names no application.
  - A `.mix` program is performed when the person runs `play x.mix`
    themselves.
  - An `.mx` document is read by its schema and never executed.
  - `play` has **no media decoders.** It describes and previews a foreign
    format only through a handler's own command-line tool, named in a
    per-user document that ships empty.
- **`play` is the one resolver.**
  - `play --serve` answers `play.open`, `play.resolve`, `play.info`,
    `play.handlers` and `play.perform` on the Bus. The file manager and the portal's OpenURI
    use it, so every way of opening a file reaches the same handler.
  - Opening a `.mix` over the Bus shows it in the editor. Only
    `play.perform` runs it, and every perform is logged.
- **`rec` writes an editable Mix script** from what happens on the desktop.
  It builds on [executed commands](2026-10-11-executed-commands.md), [command
  classes](2026-10-11-verb-classes.md) and the broker's observation stream.
  Runs of lines to one application are grouped in `address … end`, and a
  script never waits with a sleep.
- **A recording spec comes first.** `docs/docs/spec/recording/` is written and
  accepted before `rec` is built. It settles what these sources alone
  cannot:
  - **Replay arguments.** The broker does not show the payloads of a native
    session's calls, and a command event carries no arguments. Each
    application declares a recording surface: the committed actions it
    publishes with the arguments needed to repeat them, such as the path a
    dialog returned.
  - **One record per action.** A Bus call that runs a command shows up both
    in observation and as a command event. A causal identifier, or a rule
    for which source is authoritative, keeps exactly one.
  - **Per-command replay rules.** Targets are resolved stably instead of being
    dropped (a tab, a buffer). Fencing values are made fresh on replay
    (revisions, operation ids). Deliberate waits such as `ced.wait` are kept
    as conditions, never turned into sleeps.
  - **Sensitivity.** Each owner marks what is sensitive, including text
    embedded in other values. A recording that lacks a required argument
    says so and is marked incomplete. It never guesses.
- **`rec` and `play` are accepted name collisions.** Both names belong to
  sox, which installs `/usr/bin/rec` and `/usr/bin/play` (checked
  2026-10-11 in Arch Linux and Debian trixie, where sox is the only owner).
  - MixOS installs to `/opt/mixos/bin`, which comes first in its PATH, so a
    MixOS session's `play` is this one.
  - sox's commands stay reachable by full path.
  - MixOS's own units and scripts call `/opt/mixos/bin/play` by absolute
    path.
  - `recplay` itself collides with nothing.

## Why

Opening a file and recording what a person does are two halves of the same
contract, the desktop's commands. One resolver means the file manager, the
portal, the command line and agents never disagree about which application
opens a file. Using the freedesktop data that applications already install
means MixOS learns a new format the moment an application that handles it
is installed, without a MixOS change.

A recorded script is ordinary Mix. It can be read, edited, shared and
performed on another machine, and it shows exactly how the work was done.

The two names are the plain words for the two actions. sox's tools of the
same names are audio utilities that a MixOS session does not ship. The cost,
calling them by full path, falls on few people and is easy to pay.

## Consequences

- AGENTS.md §3.4 lists the accepted collisions: `mix` (Elixir), and `rec`
  and `play` (sox).
- `play`'s Bus commands, the per-user handler document and the Bus snapshot
  document schema are registered as contracts when they land.
- The file manager stops spawning `xdg-open` and calls `play.open`.
- Rejected: a MixOS MIME-to-application table. It duplicates what every
  application already installs, and it would have to name applications.
