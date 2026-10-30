---
title: Decision — coding agents as Bus sessions, over ACP
description: agentd runs ACP coding agents as Bus sessions; ACP is a vocabulary carried on ordinary Bus commands and topics, not a second wire format, and every session's agent reaches the Bus back through mixos-mcp.
---

# Decision — coding agents as Bus sessions, over ACP

Status: accepted 2026-10-30.

## Context

Coding agents (Claude Code, Gemini CLI, Codex, OpenCode) were driven by a
separate hand-made path each, and none was a Bus citizen. A running turn
could not be watched, steered, answered or stopped from the Bus. Agents
already reached *into* MixOS through [mixos-mcp](../../mixos-mcp/README.md);
the missing half was MixOS reaching *out* to them.

The [Agent Client Protocol](https://agentclientprotocol.com) (ACP, Apache-2.0)
is the open standard for that: JSON-RPC 2.0 over an agent's stdio, spoken by
all of those agents directly or through a small adapter.

## The decision

1. **ACP is a vocabulary, not a wire format.** The Bus keeps its own
   framing. ACP messages never travel raw in a Bus body; one gateway maps
   them onto ordinary Bus commands and topics.
2. **One gateway: [agentd](../../agentd/README.md)**, a Mix Bus citizen
   (Bus name `agent`), one per user session. It is the only process that
   holds ACP pipes, runs each agent as a child on a two-way pipe, and adds
   no transport or addressing of its own. It starts as a Mix script, per
   "Mix before another Rust daemon".
3. **The loop: ACP → Bus ← MCP.** agentd speaks ACP to the agent and
   republishes its updates on `agent.session.updated`; it gives every
   session a mixos-mcp server, so the agent acts back on the Bus as
   `mcp-s-<session>-…`.
4. **The client side of ACP is MixOS's.**
   - Files (`fs/*`) are confined to the session's `cwd` and
     `additional_dirs`, which must lie under the instance's configured
     roots (paths resolved, no symlink or `..` escape).
   - Terminals (`terminal/*`) run on pipes with a bounded output tail on
     the headless path; on a desk they are to move onto Term.
   - Permission requests go to a human through notifyd, or to an instance
     policy that may answer only `allow_once`. No answer is never yes, and
     a cancel answers every open request `cancelled`.
   - This confines what the agent asks agentd for. The agent's own tools
     (its shell, its built-in file tools) are not confined by agentd, and
     the decision does not claim a sandbox.
5. **Agent rules still bind.** Agents run with the user's real `HOME` and
   normal configuration, so their own hooks and instructions apply.
6. **The broker owns the sender.** agentd recognises a session's own agent
   by the Bus name the broker stamps on its requests. A session's own agent
   cannot act on that session (`AGENT_SELF`); a session started by an agent
   is a child, and a child cannot start or resume sessions (`AGENT_DEPTH`);
   a hard cap on live sessions (`AGENT_LIMIT`) holds for everyone.
7. **Every `agent.*` command answers every caller**, per the
   [open Bus](2026-10-26-open-bus-for-agents.md). What makes a call safe is
   the instance: its roots, its permission policy, its caps, and the log.
   Capability tokens were considered and rejected: they refuse honest
   callers and leave self-approval one hop away.
8. **Durable intent.** agentd writes every acting command and every mapped
   update to an append-only SQLite log before it runs or publishes it.
   Command ids are scoped per session; a retry with the same id gets the
   first reply and never runs again. Sessions outlive agentd in the log,
   and `agent.session.resume` brings one back through the agent's own
   `session/resume` or `session/load`.
9. **Versions.** ACP v1 is required; v2 is to be negotiated when both sides
   offer it, behind the same Bus vocabulary.
10. **Foreign code.** ACP agents are external programs at the edge, called
    through Mix process primitives. No ACP SDK enters `mix`.

## Known residuals

- An agent with a shell can open a Bus client of its own, or register under
  another name. The sender check catches the honest MCP path only; the cap
  and the human in the loop catch the rest.
- The asking agent can reach notifyd through mixos-mcp and answer its own
  permission question: an accepted consequence of the open Bus.
- agentd checks a path and then reads or writes it in a separate step; an
  agent that swaps the path for a symlink in between can lead it out, but
  doing so takes the agent's own tools, which reach the same files anyway.

## Not doing

- Replacing the Bus wire format with JSON-RPC, or speaking ACP to outside
  editors directly. An ACP server face for editors could come later, on
  top of agentd, as its own decision.
- Merging mixos-mcp into agentd: mixos-mcp serves outside agents too.

## Where it stands

agentd, its session log, resume, `fs/*`, headless terminals and the
[`mix agent`](../../mix/cli.md) command line are in place, tested with
Claude (`claude-agent-acp`), Gemini CLI, OpenCode and Codex (`codex-acp`).
Open: terminals through Term on a desk, ACP v2, and a Workshop front end.
