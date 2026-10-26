---
title: Decision — the Bus is open to every caller
description: Every Bus command in every app and service on every node answers every caller; refusals by caller identity or origin are removed, integrity checks stay.
---

# Decision — the Bus is open to every caller

Status: accepted 2026-10-26. Replaces *Macro-file write authority*
(2026-10-24) and amends *hostd, the host service* (2026-10-12).

## The decision

- **Every Bus command answers every caller.** An agent can do over the Bus
  anything a person can do at the keyboard, in every app and service, on
  every node. No command refuses a request because of who sent it (service
  name, account, uid, grant, operator list, allowlist) or where it came
  from (local or mesh, Bus or keyboard, drive input or a real click).
- **There is no strict mode.** `MIXOS_MESH_OPEN=0` and every posture it
  switched are removed. The open posture is the only one.
- **Macro files are ordinary Bus state.** `<app>.macros.put`, `remove`,
  `move`, `copy` and `new` accept any caller, global and cross-app writes
  included. Workshop's `workshop.macros.*`, `workshop.execute` and
  `workshop.call` forward them without a gate.
- **Workshop and the apps' windows add nothing a caller lacks.** Workshop's
  window is a convenience, not an authority.
- **File managers and panels act on Bus requests.** Dopus runs `file.*`
  actions and accepts its dialogs from the Bus and from drive input, as
  from the keyboard. The volume and network panels take changes from any
  caller.
- **Node-local answers are gone.** notifyd answers and dismisses, and
  portald answers screen-share consent, for callers on any node.
- **Operator lists are gone.** maild and webd commands that named operators
  (vtokens, retention, Bayesian rebuild, listeners, TLS reload) accept any
  caller; property namespaces grant every capability to every caller.
- **The broker opens its own commands.** `noded.observe.start` and
  `noded.tap` accept any caller on any node; native-session traffic may
  cross nodes.

## hostd

hostd's writes are open to **every caller on the same host**, over the
local Bus or its Unix socket, with no grant file. Writes from another node
stay refused: a compromised node must not be able to rewrite every host's
network. hostd keeps what makes a write safe rather than who may make it:
typed commands, no generic exec, health checks with automatic revert,
receipts for `hostd.undo`, rate limits and `freeze`.

## What stays

These checks prove where a request came from or keep state consistent.
They refuse no honest caller:

- The broker stamps every request with its origin and sender; a request
  with no stamp is malformed.
- A local request may not carry mesh provenance fields (forged origin).
- A topic is published by the service that owns it, so a settings change on
  the Bus is always settingsd's.
- A session record is changed by the account that owns it.
- A node joins the mesh only through admission.
- Argument validation, size and rate limits, freshness and replay checks,
  path containment, and read-only built-in files.

## Why

An LLM has no hands. Every job an agent performs, it performs through the
Bus, so every gate that tells an agent "only a person, only Workshop, only
this node" is a job the agent cannot do. MixOS is agentic-first and
default-open (ADR §1.9). Gates that defended against other processes of the
same user, or against the user's own mesh, protected nothing and blocked
the agents.
