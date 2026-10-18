---
title: mixos-mcp
description: MCP stdio access to the native MixOS Bus.
---

# mixos-mcp

`mixos-mcp` is an MCP stdio server for agents. Install the binary at
`/opt/mixos/bin/mixos-mcp` and register the MCP server as `mixos`.
It ships in the server prefix archive. Its package, binary and component
are the [single naming exception](../docs/decisions/2026-10-19-mcp-program-name.md).

## Connection

The server starts without a Bus connection. The first Bus tool connects
using `libs/bus`. `MIXOS_NODED_URL` overrides the node configuration URL;
otherwise the native client's node config discovery applies, including
`MIXOS_NODE_CONFIG`, `MIXOS_ETC` and the user/system defaults.

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
      "command": "/opt/mixos/bin/mixos-mcp",
      "env": {
        "MIXOS_NODED_URL": "ws://192.0.2.10:4200/ws",
        "MIXOS_BROKER_ACCOUNT": "user",
        "MIXOS_BUS_SOCKET": "/run/mixos/session/4/noded/bus.sock"
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
- `app_describe`, `app_controls_list`, `app_control_get`, `app_control_set`,
  `app_actions_list`, `app_action_invoke`, `app_control_wait`
- `desktop_windows`, `desktop_window_wait`
- `context_search`, `index_workspace`, `knowledge_digest`, `knowledge_brief`
- `skills_retrieve`, `skills_store`, `skills_refine`, `skills_graduate`,
  `skills_list`, `skills_delete`
- `docs_feedback`, `journal_feedback`, `memory_feedback`, `journal_supersede`
- `mix_execute`, `mcp_status`

Knowledge tools retain their ranking, indexing, feedback and skill-learning
behaviour. They require an `indexd` service on the selected Bus and send
`indexd.<action>` requests through that broker. This bridge does not install
indexd; an absent backend returns an explicit tool error. Skill refinement
also needs a configured LLM backend. Settings are MixOS strict-data files:
`knowledge.conf.mix`, `domains.conf.mix`, `skills.conf.mix` and `llm.conf.mix`.
LLM Bus providers use the same broker. `api_key_cmd` is Mix source, executed
by `/opt/mixos/bin/mix -c`.

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
