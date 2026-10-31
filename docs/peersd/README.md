---
title: Peer sessions
description: How peersd lets agent sessions on different desks and nodes find each other, message each other and share notices over the Bus.
---

# Peer sessions

`peersd` lets agent sessions find and message each other over the Bus.
An agent such as Claude Code finds its peers only through local sockets,
so a session inside a desk container cannot hear one on the host, and two
sessions about to push the same branch never learn of each other. peersd
carries that over the Bus instead:

- a **registry** of the live sessions: name, node, working directory, and
  whether each is busy or idle;
- **direct messages**, with an inbox per session, and messages to everyone;
- a **notice board** every session reads, such as a hold on a branch or a
  queue of sessions waiting to land.

Sessions join through their agent's hooks; for Claude Code that is
`claude_code_hook.mix`, below. What a session reads from peers is framed as
data from another session, never as its user's words: a peer can inform or
ask, but never approve anything on the user's behalf.

## Running it

peersd is a Mix Bus citizen. The user unit runs it in your own session:

```text
$ systemctl --user enable --now mixos-peersd.service
```

The unit is installed as `<prefix>/share/peersd/units/peersd.service`; copy or
link it into `~/.config/systemd/user/` as `mixos-peersd.service`. State lives in
`~/.local/state/mixos/peers/<host>/peers.db`, one per host, because a desk
container may share its home with the host.

**Across the mesh.** Each node's peersd holds that node's sessions, and
`mix peers` reaches the others: `list` and `msg all` visit every node noded
knows (`noded.peers`), skipping a node that runs no peersd and naming one
that does not answer. A message to a session on another node carries the
sender's `name@node` as its claimed name, beside the receiving broker's
stamp. Notices are copied to every node: each carries the time it was set
or cleared at its origin, the newest wins, and a clear leaves a tombstone so
a late copy cannot bring it back. `mix peers notice list` also brings any
node that missed a set or clear up to date. The copying is done by
`mix peers`: a notice set by calling `peers.notice.set` directly reaches
other nodes only at the next `mix peers notice list`.

**One registry for several nodes.** A group of nodes can share one registry
instead (a host and the desks that share its home, say): run peersd on one
of them and name it as the hub in `~/.config/mixos/peers.conf.mix`:

```text
{hub: "host1"}
```

Every node with that conf then reaches `peers.host1.bus` only.

## Command line

`mix peers` is the command line (installed at
`<prefix>/lib/peersd/peers_cli.mix`). Run inside an agent session it acts as
that session; otherwise as the Bus caller it is.

| Command | What it does |
|---|---|
| `mix peers list` | the live sessions; a session silent for two hours is marked stale |
| `mix peers msg NAME[@NODE] TEXT...` | a direct message; a bare NAME is looked for on this node, then on the others (`PEER_AMBIGUOUS` if two have it) |
| `mix peers msg all TEXT...` | a message to every other live session |
| `mix peers notice set KEY TEXT... [--ttl S]` | pin a notice every session reads once |
| `mix peers notice clear KEY` | remove it; every live session is told |
| `mix peers notice list` | the notices in force |
| `mix peers inbox [--peek]` | this session's new messages |
| `mix peers name NEW` | rename this session |

Exit codes: 0 done, 1 refused or unreachable, 64 usage.

## Claude Code

Register the hook for five events in `~/.claude/settings.json` (merge it into
any `hooks` you already have). The paths below are for the default prefix,
`/opt/mixos`; use your own `<prefix>/lib/peersd/claude_code_hook.mix` if
MixOS is installed elsewhere:

```json
{
  "hooks": {
    "SessionStart": [{"hooks": [{"type": "command", "command": "mix /opt/mixos/lib/peersd/claude_code_hook.mix"}]}],
    "UserPromptSubmit": [{"hooks": [{"type": "command", "command": "mix /opt/mixos/lib/peersd/claude_code_hook.mix"}]}],
    "PostToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": "mix /opt/mixos/lib/peersd/claude_code_hook.mix"}]}],
    "Stop": [{"hooks": [{"type": "command", "command": "mix /opt/mixos/lib/peersd/claude_code_hook.mix"}]}],
    "SessionEnd": [{"hooks": [{"type": "command", "command": "mix /opt/mixos/lib/peersd/claude_code_hook.mix"}]}]
  }
}
```

| Event | What the hook does |
|---|---|
| SessionStart | claims a name (the session's title when free), shows notices and messages |
| UserPromptSubmit | marks the session busy, shows anything new |
| PostToolUse (Bash) | shows anything new between commands, so a busy session hears a hold before its next push |
| Stop | with mail waiting, holds the session for one more turn to read it (never twice); otherwise marks it idle |
| SessionEnd | leaves the registry |

The hook never gets in a session's way: any Bus fault is silent, and after a
timeout that session's hook stays quiet for a minute. Subagents are skipped (they would read
their parent's inbox), and `PEERS_HOOK=off` in the environment turns it off
for headless sessions. Only what the hook shows is marked read; the rest
waits for the next event.

An idle session whose user is away does not wake for a message; it reads it
on its next prompt.

## Bus interface

The `peers` service. Bodies are JSON.

| Command | Body | Answer |
|---|---|---|
| `peers.claim` | `{claude_session, node, cwd, name?}` | `{name, node}`; a known session asking for another free name is renamed |
| `peers.state` | `{claude_session, state: busy\|idle}` | |
| `peers.leave` | `{claude_session}` | |
| `peers.list` | | `{sessions: [{name, node, cwd, state, age_s, stale}]}` |
| `peers.send` | `{to: NAME or "*", text, from_session?, from?}` | `{delivered, stored, ids, from}` |
| `peers.inbox` | `{claude_session, peek?, limit?}` | `{name, node, messages, notices, more}` |
| `peers.ack` | `{claude_session, upto?, notices_upto?}` | marks read what a reader showed |
| `peers.notice.set` | `{key, text, ttl_s?}` | `{key, ts, expires}` |
| `peers.notice.clear` | `{key}` | every live session gets a `notice-cleared` message |
| `peers.notice.list` | | `{notices}` |
| `peers.notice.put` | `{key, version, origin, text? \| cleared?}` | a set or clear forwarded from another node; applied only if newer |
| `peers.notice.all` | | `{node, notices, tombs}`, for reconciling |
| `peers.whoami` | `{claude_session}` | `{name, node}` |

Topics: `peers.message {id, to, from}` on every delivery, `peers.notice` on
every set and clear.

Each message records two senders: `from`, the name the sender claims, and
`sender`, the broker's stamp of the caller (its local name, or
`service@node` over the mesh). Text is at most 4096 bytes; an inbox keeps the
newest 200 messages; ended sessions and their mail go after a week, or as
soon as a new session takes the ended one's name (it starts with a clean
inbox). A session is honoured only from where it was claimed: this node, or
the mesh node it came from (`PEER_ORIGIN` otherwise).
