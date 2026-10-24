# mixos-mcp

MCP stdio bridge to the MixOS Bus. See the [manual](../../docs/mixos-mcp/README.md)
for connection settings and the complete tool list.

Application tools are `app_describe`, `app_actions` and `app_execute`.
`app_actions {service}` lists the application's registry rows;
`app_execute {service, id, params?}` sends the command id and optional
parameter object to the application's execute command.

Build and test with `cargo test -p mixos-mcp`; the integration test embeds the
real noded using `test-broker`, with isolated socket, config and home paths.
