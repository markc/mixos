# mixos-mcp

MCP stdio bridge to the MixOS Bus. See the [manual](../../docs/mixos-mcp/README.md)
for connection settings and the complete tool list.

Connections require verified Unix ingress whenever a socket candidate exists.
Only `MIXOS_MCP_ALLOW_TCP=1` permits plain TCP when all Unix candidates are
missing; existing socket verification failures remain fatal.

Application tools are `app_describe`, `app_actions` and `app_execute`.
`app_actions {service}` lists the application's registry rows;
`app_execute {service, id, params?}` sends the command id and optional
parameter object to the application's execute command.

The skills loop is retired. At deployment, remove `trust_weight_graduated`,
`trust_weight_skill` and any old `skills`/`llm` sections from `knowledge.conf.mix`,
and remove the unused `skills.conf.mix` and `llm.conf.mix` files from the MCP
config directory (`$MIXOS_ETC`, normally `~/.config/mixos`). The config loader
tolerates unknown keys, but they have no effect; no aliases are provided.
Generated knowledge defaults omit them. Existing configs are not rewritten.

`knowledge_digest {}` returns global statistics. It refuses a non-null `domain`
argument because indexd cannot scope these queries. Stale-query failures preserve
statistics with `stale: null` and a `stale_error` message; see the manual for the
explicit thresholds and the observation purge procedure.

Build and test with `cargo test -p mixos-mcp`; the integration test embeds the
real noded using `test-broker`, with isolated socket, config and home paths.
