---
title: Decision — executed commands are published
description: Every MixOS application publishes each command it runs as a Bus topic, so what a person does by hand can be watched and recorded like what an agent does over the Bus.
---

# Decision — executed commands are published

Status: accepted 2026-10-11. Amends [egui applications](2026-10-09-egui-applications.md)
(AGENTS.md §4.1).

Amended 2026-10-24 by [Bus commands and app actions](2026-10-24-bus-commands-and-actions.md): the names below are now command, command class, command family and action.

## The decision

Amended 2026-10-20 by [commands take parameters and return results](2026-10-20-commands-take-parameters.md). The body is v2; v1 events are no longer emitted.

- **Every application publishes each command it runs.**
  - The topic is `<app>.action.executed`, where `<app>` is the
    application's Bus service name.
  - An event means the command registry accepted the invocation and the
    command's handler returned.
  - A command that is unknown or disabled is refused, and nothing is
    published.
  - The event does not report the outcome of work the handler queued, such
    as a save that later fails. Outcomes travel by the application's own
    commands and topics.
- **The body:**

  ```text
  {
    v: 2,
    run: "5f0c…",           -- this process: a UUID drawn once at start
    seq: 812,                -- per run, from 1, assigned in execution order
    at_ms: 1760160000123,    -- integer ms since the Unix epoch, at handler return
    id: "file.open",         -- the command's stable id
    origin: "menu",          -- menu | shortcut | palette | ui | bus | macro
    params: null,            -- accepted body, null for a nullary run or note()
    result: null,            -- returned value; note() records null
    dropped: 0               -- events this run lost since the last one published
  }
  ```

- **`seq` and `dropped`:**
  - **Numbering.** `seq` is assigned when the event is created, before it
    is queued, so the published stream may have gaps.
  - **Loss.** When the queue is full the oldest queued event is dropped and
    a loss counter grows. The next event that is actually published carries
    the counter in `dropped`, and the counter resets. A gap in `seq` always
    equals the sum of the `dropped` values that explain it.
  - **Ordering.** `seq`, not `at_ms`, orders a run's events: the wall clock
    can move.
  - **A new run.** A different `run` means the application restarted, and
    `seq` starts again at 1. Consumers key a sequence by the publishing
    service (and node) together with `run`; `run` is opaque.
  - **Reconnects.** Events that could not be published while the Bus was
    away count as dropped.
  - **Unknown outcomes.** A publish whose outcome is unknown (the
    connection failed after the frame went out, or no answer came) may have
    reached subscribers, so it is assumed delivered and not counted. If it
    was in fact lost, a subscriber sees a gap that `dropped` does not
    explain, the same as its own missed deliveries. Exact accounting there
    would need a broker receipt per event.
  - **Subscriber loss.** A subscriber that misses deliveries detects it from
    `seq` gaps that `dropped` does not explain.
- **`origin` is the mechanism that reached the registry**, not an earlier
  cause:
  - `menu`: a menu row, including one chosen through synthetic input;
  - `shortcut`: a key binding, including a synthetic key press;
  - `palette`: the command palette or Help search;
  - `ui`: any other control in the window, such as a toolbar button;
  - `bus`: `<app>.execute` (or another command whose documentation says it runs
    a registry command);
  - `macro`: the shared macro runner. This value is reserved until
    applications have a Macros menu.
- **Parameters and result:** the event carries the accepted params and the
  returned result, or a digest above a bound. Each field is measured using
  compact JSON (`serde_json::to_vec`); a value above 4096 bytes becomes
  `{ bytes, sha256 }`, with a lowercase SHA-256 digest of those bytes. The
  accepted params are preserved verbatim; the registry cannot reserialise
  defaults applied inside a handler. Nullary params and both `note()` fields
  are null.
- **Publishing never blocks the application.**
  - Events go through a bounded queue on the application's existing effect
    path.
  - The topic is not retained.
  - Events still queued when the application exits are lost, and nothing
    counts them: the next run starts afresh.
- **It will be implemented once.** The UI-free command registry (`libs/command`) will take
  the origin at dispatch and record each run. The shared Bus connection (the
  `citizen` library, or an application's own connection until it moves to
  `citizen`) will publish the records. An application will not write its own
  publisher.

## Why

Every action is already a command with a stable id, and agents already reach
every command over the Bus. Until now the commands a person ran by hand
stayed inside the process. Nothing could watch them, record them or learn
from them. Publishing each run closes that gap with one hook, and it is the
first half of how [`rec`](2026-10-11-rec-and-play.md) writes a person's work
as a script.

## Consequences

- AGENTS.md §4.1 gains the rule.
- `Registry::execute` gains an origin, and the existing call sites state
  theirs.
- Each application gets a test proving that a menu command and a Bus command
  are both published with the right origin and consecutive `seq`, and that
  a refused command publishes nothing.
