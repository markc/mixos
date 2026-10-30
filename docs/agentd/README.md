---
title: Agent sessions
description: How agentd runs coding agents (Claude, Codex, Gemini, OpenCode and other ACP agents) as Bus sessions that anything on the Bus can start, watch, steer and answer.
---

# Agent sessions

`agentd` runs coding agents as Bus sessions. Anything on the Bus can start
one: Workshop, a terminal, a Mix script, or another agent. Each running
agent's output is published as Bus events, so any number of watchers can
follow a turn. When the agent asks permission to do something, the
question goes to the human on the desk they are looking at, through
[notifyd](../notifyd/README.md).

agentd speaks the [Agent Client Protocol](https://agentclientprotocol.com)
(ACP) to the agent. ACP is the open standard that most coding agents
already support, so one integration covers them all. agentd adds what ACP
lacks: addressing across the mesh, many watchers, and a human in the loop.
Every session also gets [mixos-mcp](../mixos-mcp/README.md), so the agent
can use the Bus in turn. The rules are in the
[decision](../docs/decisions/2026-10-30-acp-agent-sessions.md).

```text
 Bus callers ── agent.* commands, agent.session.* topics ──► agentd
                                                              │ ACP over the agent's stdio
                                                              ▼
                                                     agent (claude-agent-acp, …)
                                                              │ MCP
                                                              ▼
                                                 mixos-mcp ──► back onto the Bus
```

Run it as a session service: `mix --serve agentd.mix --name agent`. It
needs a `mix` with `process_watch` stdin pipes (`stdin: "pipe"`,
`process_write`).

## Configuration

agentd reads `$MIXOS_AGENTD_CONF`, else `<etc>/agent.conf.mix`. The file is
strict data:

```mix
max_live: 4
agents: {
  claude: {argv: ["claude-agent-acp"], roots: ["/home/user/src"]},
  gemini: {argv: ["gemini", "--acp", "--skip-trust"], roots: ["/home/user/src"]},
  opencode: {argv: ["opencode", "acp"], roots: ["/home/user/src"], policy: "ask"}
}
```

Tested agents (2026-10-30), each with mixos-mcp reaching the Bus:

| Agent | argv | Notes |
|---|---|---|
| Claude | `claude-agent-acp` (npm `@agentclientprotocol/claude-agent-acp`) | Uses its own file and shell tools, so Claude Code hooks such as a PreToolUse gate still apply. |
| Gemini CLI | `gemini --acp --skip-trust` | Without `--skip-trust` Gemini treats the folder as untrusted and starts no MCP server. The flag trusts the session's `cwd` for that run only, so roots should hold only trusted code. Reads go through `fs/read_text_file`. Its write tool cannot create a new file through any ACP client: it checks for a missing file with `instanceof Error`, which a JSON-RPC error is not, so it falls back to its own shell. |
| Codex | `codex-acp` (npm `@agentclientprotocol/codex-acp`) | Uses the normal Codex login and `CODEX_HOME`. It starts MCP servers with a bare environment, so agentd passes its own broker and layout variables (`MIXOS_NODED_URL`, `MIXOS_BUS_SOCKET`, `MIXOS_RUN`, …) in the mixos-mcp entry. |
| OpenCode | `opencode acp` | The model is a config option: `agent.session.set {session, option: "model", value: "deepseek/deepseek-v4-pro"}`. |

- **argv**: the agent's ACP command, run directly (no shell).
- **roots**: the directories a session's `cwd` and `additional_dirs` must
  lie under. Paths are resolved first, so a symlink cannot escape. This
  confines only what the agent asks agentd for. The agent's own tools, such
  as a shell, are not sandboxed by agentd.
- **policy**:
  - `ask` (the default) sends every permission request to the human.
  - `allow_once` lets agentd answer `allow_once` by itself.

  agentd never answers `allow_always`.
- **permission_timeout_s** (default 300): with no answer by then, the
  request is rejected. No answer is never yes.
- **max_live** (default 4): live sessions at once. It is a hard cap, and
  agent-started sessions count against it.
- **log_keep_days** (default 30): how long the session log keeps a session
  after it ends.

## Commands

Bodies are JSON objects.

| Command | Body | Reply |
|---|---|---|
| `agent.agents` | | the configured agents, live count and cap |
| `agent.session.new` | `{agent, cwd, mode?, additional_dirs?}` | `{session, status}` at once; `ready` follows as an event |
| `agent.session.prompt` | `{session, content}` | `{session, turn}`; the turn streams as events |
| `agent.session.cancel` | `{session}` | cancels the running turn |
| `agent.session.close` | `{session}` | ends the agent (stdin closes; an agent still running 10 s later is stopped); the session stays in the log and can be resumed |
| `agent.session.resume` | `{session}` | `{session, status, via}` at once: brings back an ended session (see Resuming) |
| `agent.session.set` | `{session, mode}` or `{session, option, value}` | sets a mode, or a config option such as the model |
| `agent.session.list` | | live and recently ended sessions |
| `agent.session.read` | `{session, since?, limit?}` | the session and its logged events after `seq`: `{session, events, more, next}` |
| `agent.permission.respond` | `{session, request, option}` | answers a permission request |

`content` is a string, or a list of ACP content blocks.

A read returns at most `limit` events (default and maximum 2000) and about
32 MiB. When `more` is true, read again with `since` set to `next`.

The acting commands (`new`, `prompt`, `cancel`, `close`, `set`, `respond`)
take an optional `id` of 1-128 characters from `[A-Za-z0-9._:-]`. Ids are
scoped to the session (to `new` for `agent.session.new`). A retry with the
same id gets the first reply and never runs again, so a caller that lost a
reply can safely send the command again. If agentd stopped after logging a
command but before replying, the retry gets `COMMAND_INTERRUPTED`; the
command is not run again.

Refusals carry `{error_code, message}`:

| rc | error_code |
|---|---|
| 10 | `USAGE`, `OUTSIDE_ROOTS` or `UNKNOWN_AGENT` |
| 14 | `UNKNOWN_SESSION` |
| 15 | `AGENT_RUNNING` (a turn is already running), `AGENT_NOT_READY`, `COMMAND_INTERRUPTED` or `AGENT_NOT_RESUMABLE` |
| 16 | `AGENT_LIMIT` or `AGENT_DEPTH` |
| 17 | `AGENT_SELF` |

## Events

| Topic | Body |
|---|---|
| `agent.session.updated` | `{session, seq, kind, …}` |
| `agent.session.ended` | `{session, exit_code, signal}` |
| `agent.permission.requested` | `{session, request, title, options}` |

`kind` on `agent.session.updated` is one of:

- `initialized` or `ready`;
- an ACP update kind (`agent_message_chunk`, `tool_call`, `plan`, …), with
  the update unchanged in `update`;
- `stop`, with `stop` set to `end_turn`, `max_tokens`, `max_turn_requests`,
  `refusal` or `cancelled`;
- `permission_answered`;
- `error`;
- `fs_read`, `fs_write` or `fs_refused` (see Files);
- `terminal_created` or `terminal_exited` (see Terminals);
- `ended`, logged (not published) when a restart ends a session;
- `resumed` (with `via`, the agent's info and capabilities) or
  `resume_failed` (see Resuming).

`seq` rises by one per event in a session. A watcher that missed events
reads them back with `agent.session.read {since}`.

## Resuming

`agent.session.resume {session}` brings back a session that ended, whether
it was closed, its agent exited, or agentd restarted. Nothing resumes by
itself. The session keeps its id, its `mcp-s-<session>-` Bus name and its
log, and `seq` carries on. agentd starts the agent again and asks it to
reopen its own record of the session:

- `session/resume` when the agent offered it at `initialize` (no replay);
- otherwise `session/load`. The agent then replays the conversation, which
  the log already holds, so replayed messages, tool calls and plans are
  dropped; current state (mode, commands, config options) is kept.

The current configuration decides: the agent must still be configured and
the session's directories must still lie under its roots. A resume counts
against `max_live`, and like `new` it is refused to a session's own agent
(`AGENT_SELF`) and to a child session (`AGENT_DEPTH`). An agent that offers
neither method gets `AGENT_NOT_RESUMABLE` before anything starts. If the
agent refuses (it no longer has the session) or exits first, the session
goes back to ended exactly as it was, with a `resume_failed` event. Gemini
CLI answers `session/load` first and replays after, so after a load the
session stays `starting` (a prompt is queued) until the replay has been
quiet for a second, and replayed history is dropped until then.

agentd does not restore the mode or model (set them again) or a permission
question that was open when the session ended (prompt again). Advertised
at `initialize`: Claude, OpenCode and Codex offer `session/resume`; Gemini
offers `session/load`.

## Command line

`mix agent` drives agentd from a shell or the Mix prompt. Each command is one
Bus call, except `send --wait`, which follows the turn on the topics (it runs
as a short-lived Bus citizen, so nothing polls) and streams the agent's text.

| Command | Does |
|---|---|
| `mix agent agents` | the configured agents |
| `mix agent list` | live and recent sessions |
| `mix agent start AGENT [--cwd DIR] [--dir DIR]... [--mode M] [--id ID]` | starts a session; prints its id (cwd defaults to the current directory) |
| `mix agent send SESSION TEXT... [--wait] [--timeout S] [--id ID]` | sends a prompt; with `--wait`, streams the turn and exits with its outcome |
| `mix agent read SESSION [--since N] [--json]` | the session's events, one per line |
| `mix agent answer SESSION REQUEST OPTION` | answers a permission question |
| `mix agent cancel\|close\|resume SESSION [--id ID]` | as the Bus commands |
| `mix agent set SESSION --mode M` or `--option NAME --value V` | sets a mode or a config option |

Exit codes: 0 done (`end_turn`), 1 refused, failed, or a turn that stopped
another way, 2 the turn is waiting on a human (a permission question:
`answer` it, then `send` or `read` again), 64 usage, 124 `--wait` timed out
(the turn goes on). Exit 2 means a question is still open: one an
`allow_once` policy answers at once does not stop `--wait`. `--id` makes a
retried command safe: agentd runs it once and replays its first answer.

## Session log

agentd writes every event and every acting command to a SQLite log before
it publishes the event or runs the command: `$MIXOS_AGENTD_DB`, else
`<var>/agentd/sessions.db`. The log outlives agentd. After a restart:

- ended sessions and their events can still be listed and read;
- command receipts still answer retries;
- the agents that were running are gone (agentd owned them), so their
  sessions are ended with a last `ended` event whose `reason` is
  `agentd_restart`.

Sessions are dropped from the log `log_keep_days` after they end.

If the log cannot be written (a full disk), commands are refused, but the
agent's events are still published and the gap is reported on stderr. A
watcher that follows `seq` should therefore also check the session's
`status` in a read: after such a gap and a restart, the `ended` event can
carry a `seq` the watcher has already seen.

## Files

agentd serves ACP's `fs/read_text_file` (with `line` and `limit`) and
`fs/write_text_file`, inside the session's `cwd` and `additional_dirs`
only. Paths must be absolute and are resolved first, so a symlink or `..`
cannot lead out; a new file's parent directory must already exist. A read
is limited to 8 MiB. Refusals are JSON-RPC errors: `-32002` for a missing
file, `-32602` for a path outside the session or a bad request. Every read,
write and refusal is an event (`fs_read`, `fs_write`, `fs_refused`).

This confines what the agent asks agentd to do. An agent's own tools, such
as its shell or Claude's built-in file tools, do not go through agentd.
The path check and the read or write are separate steps, so an agent that
replaces a path with a symlink between them can still lead one out; that
takes the agent's own tools, which reach the same files anyway. A missing
path outside the session is answered as outside, so the errors do not tell
the agent what exists elsewhere.

## Terminals

agentd serves ACP's `terminal/*` on the headless path: the command runs
directly on pipes (no shell, no PTY), in the session's `cwd` or a `cwd`
inside its directories, with the instance `env` plus any the agent asks
for. agentd keeps the newest 5,000 lines of stdout and stderr together, up
to `outputByteLimit` bytes (at most 8 MiB), dropping from the front.
Terminal query sequences (cursor and device status requests, colour
queries) are removed, since nothing is there to answer them. A session has
at most 8 terminals at once, and they end with it. `terminal/kill` stops
the command (TERM, then KILL) and reports `SIGTERM`, even when KILL was
needed. Each terminal's start
and exit are events (`terminal_created`, `terminal_exited`).

On a desk, terminals are to move onto Term, so a human can watch them.

## Agents calling back

agentd runs every agent with `MIXOS_AGENT_SESSION` set to its session id,
and that agent's mixos-mcp registers on the Bus as
`mcp-s-<session>-<suffix>`. The broker stamps that name as the sender of
everything the bridge sends, so agentd recognises a session's own agent:

- It cannot act on its own session (`AGENT_SELF`): no answering its own
  permission requests, no prompting or closing itself.
- A session it starts is a child, and a child cannot start sessions
  (`AGENT_DEPTH`).
- The `max_live` cap holds for everyone.

This catches the agent's MCP path. An agent with a shell can still open a
Bus client of its own, which is why the cap and the human answer matter.
