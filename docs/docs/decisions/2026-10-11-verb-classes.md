---
title: Decision — verb classes
description: Every registered Bus verb declares a class (intent, query, input or system), so tools that watch the Bus can tell a person's or agent's intent from reads, synthetic input and plumbing without guessing.
---

# Decision — verb classes

Status: accepted 2026-10-11. Amends AGENTS.md §5 (Bus verbs are wire
contracts).

## The decision

- **The verb registry moves to schema 2.** Every entry in
  `docs/docs/spec/bus/verbs.conf.mix` gains a `class`, one of:

  | class | Meaning | Examples |
  |---|---|---|
  | `intent` | Changes what an application or service holds, or what another caller would see, at someone's request | `<app>.open`, `<app>.execute`, `<app>.refresh`, `settings.apply`, `releases.notes` (it fetches when nothing is cached), `<app>.quit`, `<app>.window` |
  | `query` | Changes nothing another caller could observe. The one allowance: it may write the artefact it was asked for and returns, such as a screenshot | `<app>.ping`, `.info`, `.state`, `.commands`, `.ui.tree`, `.ui.capture`, `.window.state`, `settings.get`, `ced.wait` |
  | `input` | Synthetic low-level input, standing in for a person's hands | `<app>.ui.click`, `.ui.pointer`, `.ui.scroll`, `.ui.key`, `.ui.type` |
  | `system` | Session plumbing. The single-instance activation verb `<app>.show` is `system` even when something calls it directly | `<app>.show` |

- **The class describes the verb, not one call.** A verb that both reads and
  changes state is `intent`.
- **When in doubt, it is `intent`.** Dropping a real action is worse than
  keeping a read.
- **A verb an application describes as `read_only` is a `query`.** Each
  application's registry test checks this. The reverse need not hold:
  `.ui.capture` writes a file, so it is not read-only, but it is still a
  query.
- **The registry gate** (`tools/verb_registry.mix`) refuses:
  - a schema other than 2;
  - an entry that is not in the canonical one-line form;
  - an unknown or missing field;
  - a bad name, owner, status or version;
  - a duplicate;
  - a class outside the four.

## Why

Tools that watch the Bus need to know what a verb is for:

- a recorder keeps intent and drops reads;
- a live traffic view colours them differently;
- an agent asked "what changed?" looks only at intent.

Without a declared class, each tool guesses from verb names, and the guesses
drift apart. The registry already declares every verb's owner, version and
status, so its purpose belongs there too.

## Consequences

- Every existing entry is classified in one change, together with the gate.
- A new verb is registered with its class from its first commit.
- Classes are about purpose, not replay. A recorder still needs each owner's
  replay rules ([rec and play](2026-10-11-rec-and-play.md)), for example
  that `ced.wait` is a deliberate ordering point.
