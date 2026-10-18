# mixos-mcp

MCP stdio bridge to the MixOS Bus. See the [manual](../../docs/mixos-mcp/README.md)
for connection settings and the complete tool list.

Build and test with `cargo test -p mixos-mcp`; the integration test embeds the
real noded using `test-broker`, with isolated socket, config and home paths.
