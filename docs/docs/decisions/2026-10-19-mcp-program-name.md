# MCP bridge program name

Status: accepted.

The MCP stdio bridge is the CLI component `cli/mixos-mcp`, package and
binary `mixos-mcp`. It is the single exception to the ban on a `mixos-`
program prefix. All other component naming rules remain in force.

Clients register the server as `mixos`, so qualified tool names are
`mcp__mixos__<tool>`. The bridge uses the native Bus client and the same node
configuration discovery as other MixOS clients. It ships in the server
prefix archive, alongside the system binary.
