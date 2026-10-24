---
title: Decision — Bus commands and app actions
description: A dotted name a service answers is a Bus command and an app's registry entry is an action; "verb" is retired, and each kind of unknown name has one refusal.
---
<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->

# Decision — Bus commands and app actions

Status: accepted 2026-10-24. Amends [verb classes](2026-10-11-verb-classes.md),
[instance verb families](2026-10-13-instance-verb-families.md),
[executed commands](2026-10-11-executed-commands.md),
[egui applications](2026-10-09-egui-applications.md) and
[commands take parameters](2026-10-20-commands-take-parameters.md).

## The decision

- **A dotted name a service answers is a Bus command.** The command
  registry, command classes and command families keep the meaning the verb
  registry, classes and families had. Payloads say `command`, as the frame
  header already does.
- **An app's registry entry is an action.** `<app>.actions` lists them,
  `<app>.execute` runs one, `<app>.action.executed` publishes each run, and
  the registry crate is `libs/action`. The MCP edge tools are `app_actions`
  and `app_execute`.
- **One refusal per kind:** rc 10 with `UNKNOWN_COMMAND` for an unknown Bus
  command, from every component; `UNKNOWN_ACTION` for an unknown action.
- **"Verb" is retired** outside history (superseded decisions and
  changelogs). `tools/terminology_gate.mix` refuses it elsewhere.

## Why

Most command names are not verbs, the wire and Mix already say "command",
and ARexx, whose model Mix follows, sends commands to ports. Readers who
know HTTP take "verb" to mean a closed method set, which in MixOS is the
command class. Agents read these docs cold, so one concept gets one word,
and one word means one concept.

## Consequences

- Command registry schema 5: [`commands.conf.mix`](../spec/bus/commands.conf.mix),
  gate `tools/command_registry.mix`.
- Every component's unknown-name refusal is renamed; tests and pages follow.
- Names other systems own keep their words: the generic bridge protocol's
  `engine.commands` and `engine.execute`, and polkit's `action.lookup("verb")`.
- Stored state moves with the code: the props store renames its events
  column on open, and an install migrates hostd's standing grants
  (`commands:`) and receipts (`"command"`).
- No aliases: the change is wire-breaking and ships in one release.
