---
title: Decision — ACP clients of native agent sessions
description: ACP editors use a local Mix adapter while agentd retains ownership of processes, operations, permissions and history.
---

# Decision — ACP clients of native agent sessions

Status: accepted 2026-11-04. The operator requested an ACP editor interface
to the native mesh, followed by implementation and installation.

The [agent sessions decision](2026-10-30-acp-agent-sessions.md) permits a
future editor-facing ACP interface. That interface is now a pair of Mix
scripts owned by agentd: a blocking stdin reader and a Bus citizen that
projects native state onto the client's ACP connection. Neither launches
an agent or owns a scheduler. agentd remains the sole owner of agent pipes,
native command receipts, permission state and the durable session log.

ACP JSON-RPC terminates at the reader. It sends ordinary native operations
to the projection citizen; JSON-RPC envelopes do not travel on the Bus.
The citizen is the only protocol stdout writer. A dropped editor connection
does not cancel the agent. A new connection loads native history and current
permission cards, then follows sequenced events. There is no polling transport.

Native prompt and setting outcomes are correlated to the accepted command,
including queued cancellation and await-origin turns. Client request IDs
belong to a connection; a new connection resumes by native session ID.
Acceptance is not completion, and malformed child completion is an error,
never an invented successful ACP stop reason.

The existing open-Bus permission policy is unchanged. ACP is another view
of a native pending permission, and cannot widen a permanent grant or
override notification expiry. Other authorised native callers still act
through the same service.

Caller MCP definitions supplement the built-in mixos-mcp bridge. Validated
definitions live in private attachment files; only references enter native
receipts. Reopening a session refreshes client-scoped attachments without
copying provider credentials. The first interface is co-located with the
editor server and workspace. It does not proxy remote filesystem access,
terminals or checkpoints. Native build scheduling remains with its owner.
