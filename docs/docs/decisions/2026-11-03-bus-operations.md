---
title: Decision — long-running work on the Bus is an operation
description: A command whose work can outlast a request answers at once with an operation id, publishes one op.done when it ends, and answers op.get and op.cancel, so a caller can stop waiting and come back for the outcome.
---

# Decision — long-running work on the Bus is an operation

Status: accepted 2026-11-03.

## Why

Agents spend most of their time waiting: on a build, a merge, a reply from
a person. When a Bus request stays open for the whole wait, the caller has to
stay alive and connected for it, a broker restart or a lost connection turns
the outcome into a guess, and the caller cannot do anything else meanwhile.
The Bus broker is deliberately not durable, so the fix is not to make the
broker hold the wait. The fix is for the service doing the work to hand back
an id at once and keep the outcome itself.

## The decision

A service command whose work can outlast a request (minutes rather than
seconds) is an **operation**:

- **Start.** The command answers at once with `{op}`, a string id unique to
  that service. A caller may supply the id itself so that a retry after a
  lost reply joins the same work instead of starting it twice.
- **States.** An operation is in one of `queued`, `running`, `done`,
  `failed`, `cancelled`, `lost` or `unknown`. `done`, `failed`, `cancelled`
  and `lost` are terminal and final. `unknown` means the service has never
  seen the id; it is not terminal, because a caller may register interest
  in an id before it submits the work.
- **`<svc>.op.done`.** When an operation reaches a terminal state, the
  service publishes one `<svc>.op.done` topic, not retained, carrying
  `{op, state, rc?, log?, reason?}` and any service-specific fields. Every
  terminal state uses this one topic, `cancelled` included.
  Publication is authenticated: noded reserves the suffix to the registered
  owning service `<svc>`, while subscriptions remain public. Deliveries carry
  the broker-owned `broker_service: <svc>` header; caller-supplied identities
  and directed commands cannot manufacture it. Consumers may accept the
  authenticated terminal envelope even if the publisher exits immediately.
  Local operation consumers require an absent or empty `broker_peer`, the
  matching `broker_service` and the actual matching `topic` header. An admitted
  remote service of the same name is not authority for a local operation id;
  remote or directed traffic is validated by querying the awaited target.
  On older brokers without this header, consumers must validate event hints
  with a bounded `op.get` query. If that query fails, the await stays pending
  until another event, reconnect or deadline; broker upgrade removes this
  legacy limitation without polling.
- **`<svc>.op.get {op}`.** Answers `{op, state, ...}` with the same envelope,
  at any time. A caller that was not listening when `op.done` went out uses
  it to catch up.
- **`<svc>.op.cancel {op}`.** Asks for the operation to stop and answers
  `{op, cancelled, state}`, where `cancelled` says whether the request
  changed anything.
- **Retention.** The service keeps a terminal outcome for at least as long as
  it documents (days, for a build queue), so `op.get` still answers after the
  caller has been away.

A caller watches `<svc>.op.done` and calls `<svc>.op.get` when it starts
waiting and after every reconnect, because `op.done` is not retained and can
be missed. The broker stores nothing for any of this.

## Consequences

- An agent session can register interest in an operation, end its turn and
  give up its process; the agent supervisor resumes it with the outcome when
  `op.done` arrives (see *ACP agent sessions on the Bus*). The supervisor
  itself publishes `agent.op.done` when a session's job ends, so a parent
  waits on a child the same way.
- A caller that restarts, or a broker restart, no longer loses an outcome:
  it is one `op.get` away.
- Services adopt this as their long commands are touched. A service whose
  only long command already has its own status query may answer
  `op.get`/`op.cancel` as aliases over it.

## Not doing

- No durable queue, timers or delivery guarantees in the broker.
- No generic topic-pattern waits: interest is always in one operation id.
