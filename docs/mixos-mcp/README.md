---
title: mixos-mcp
description: MCP stdio access to the native MixOS Bus.
---

# mixos-mcp

`mixos-mcp` is an MCP stdio server for agents. Install the binary at
`<prefix>/bin/mixos-mcp` and register the MCP server as `mixos`.
It ships in the server prefix archive. Its package, binary and component
are the [single naming exception](../docs/decisions/2026-10-19-mcp-program-name.md).

## Connection

The server starts without a Bus connection. The first Bus tool connects
using `libs/bus`. `MIXOS_NODED_URL` overrides the node configuration URL;
otherwise the native client's node config discovery applies, including
`MIXOS_NODE_CONFIG`, `MIXOS_CONFIG` and the user/system defaults.

Set `MIXOS_BROKER_ACCOUNT` to the broker's operating-system account and
`MIXOS_BUS_SOCKET` to the session's Unix socket. With either set, verified
Unix ingress is required and connection failures never fall back to TCP.
Without an explicit socket, the node config or MixOS runtime directory
provides the endpoint. Without either variable, the native WebSocket client
uses the discovered URL. Calls are never replayed after reconnecting.

```json
{
  "mcpServers": {
    "mixos": {
      "command": "<prefix>/bin/mixos-mcp",
      "env": {
        "MIXOS_NODED_URL": "ws://192.0.2.10:4200/ws",
        "MIXOS_BROKER_ACCOUNT": "user",
        "MIXOS_BUS_SOCKET": "<runtime>/session/4/noded/bus.sock"
      }
    }
  }
}
```

## Tools

Each name below is exposed as `mcp__mixos__<name>` in Claude Code.

- `bus_call`, `bus_list_services`, `bus_node_info`, `bus_list_peers`, `noded_ping`
- `term_list`, `term_snapshot`, `term_type`, `term_tab`, `term_pane`
- `log_tail`, `log_search`
- `app_describe`, `app_actions`, `app_execute`
- `desktop_windows`, `desktop_window_wait`
- `context_search`, `index_workspace`, `knowledge_digest`, `knowledge_brief`
- `skills_retrieve`, `skills_store`, `skills_refine`, `skills_graduate`,
  `skills_list`, `skills_delete`
- `docs_feedback`, `journal_feedback`, `memory_feedback`, `journal_supersede`
- `mix_execute`, `mcp_status`

### Application commands

`app_describe {service}` calls `app.describe` on that Bus service.
`app_actions {service}` calls `<service>.actions`. Its `result` contains
the application's command rows unchanged: `id`, `label`, `menu`, `shortcut`,
`more` (when present), `enabled` and `checked`, plus any further metadata.

`app_execute {service, id, params?}` calls `<service>.execute` once with
`{id}` or `{id, params}`. `service` is a Bus service name such as
`"workshop"`, `id` is a dotted command id such as `"file.refresh"`, and
`params` is an optional JSON object. Parameters are forwarded unchanged as
a sibling of `id`; the application owns validation and may refuse unsupported
parameters. Successful calls return `{service, command, result}`; application
error messages from the Bus client pass through unchanged as tool errors.
The same error-text pass-through and prefix removal apply to `app_describe`,
`desktop_windows` and `desktop_window_wait` through their shared native helper.

Retired tools: `app_actions_list`, `app_action_invoke`, `app_controls_list`,
`app_control_get`, `app_control_set` and `app_control_wait` have been removed
without aliases. Use `app_actions` and `app_execute` for registry commands,
and `app_describe` to discover an application's state commands. These retired
tool names must not be reused.

### Knowledge tools

Knowledge tools retain their ranking, indexing, feedback and skill-learning
behaviour. By default they send `indexd.<action>` requests to `indexd` on
the selected Bus. An absent backend returns an explicit tool error.
Skill refinement also needs a configured LLM backend. Settings are MixOS strict-data files:
`knowledge.conf.mix`, `domains.conf.mix`, `skills.conf.mix` and `llm.conf.mix`.
LLM Bus providers use the same broker. `api_key_cmd` is Mix source, executed
by `<prefix>/bin/mix -c`.

### Host indexd from a desk

When a desk cannot reach the host's Bus, select SSH in
`$MIXOS_CONFIG/knowledge.conf.mix` (normally `~/.config/mixos/knowledge.conf.mix`):

```text
indexd: {
    transport: "ssh",
    target: "indexd",
    ssh_host: "user@host.example",
    ssh_command: ["<prefix>/bin/indexd", "bridge"],
    call_timeout_secs: 120
}
```

Install indexd on the host and configure key-based SSH access for the desk's
user. The client runs `ssh -T -o BatchMode=yes -o ServerAliveInterval=30`
with the configured host and command. The host command runs in the remote
login shell; use trusted command arguments. `indexd bridge` uses the host's
local Bus; `target` selects the address only in Bus mode.

One bridge is shared by knowledge tools for the MCP server's lifetime,
with one request in flight at a time. Each call has a deadline, including
queueing and retries. A broken pipe, exited child or bridge transport failure
gets one restart and retry; an application error is returned immediately.
A timeout discards the child and returns an error without replaying the call.
A retried mutation may already have executed, so retry is not an exactly-once
guarantee. Normal MCP shutdown closes stdin and reaps the bridge.
Restart the MCP server after changing transport settings.

Missing settings default to `transport: "bus"`, `target: "indexd"`, the
command above, an empty `ssh_host` and a 120-second call deadline. SSH mode
requires `ssh_host`. When a Bus route is available, select
`transport: "bus", target: "indexd.host.example.bus"` instead. Presence is
checked with `indexd.stats` at the configured target, including routed targets.

The default client domain map expands the user's home for `~/Projects/mixos`
(`mixos`), `~/.mix` (`mix`), `~/.ns` (`ns`) and `~/.mc` (`mc`). Custom
`domains.conf.mix` maps override these defaults. `index_workspace` expands
only a leading `~` or `~/`, resolves the workspace to an absolute path, and
sends absolute file paths and content to indexd.

Terminal tools preserve explicit pane selection and instance checks. Native
application tools require an exact service and semantic target. Read-only
tools do not invoke mutations; arbitrary `bus_call` and `mix_execute` can.
Mix execution runs in an isolated worker with bounded output.

## Logging and tests

Stdout is reserved for MCP. Logs go to stderr, journald and
`~/.local/log/mixos/mixos-mcp.log`. Arguments and results are redacted by
default; `RUST_LOG=mixos_mcp=trace` explicitly enables full arguments.
`--version --json` reports build provenance without starting MCP or the Bus.

Package tests cover parameter translation, target guards, output schemas,
redaction and provider decoding. The stdio integration test uses the real
fixture broker, including verified Unix ingress and lazy reconnection.
An in-process fake application checks command listing, execute request bodies
with and without parameters, and unchanged application error messages.
It also checks configured knowledge targets, paginated re-indexing and
absolute file metadata. SSH client tests use a local Mix fake bridge through
a test-only command hook; no SSH connection or indexd process is required.
