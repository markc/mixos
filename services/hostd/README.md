# hostd

The host service: information about the host, and privileged host changes
through closed, typed verbs behind standing grants, receipts and rollback
([decision](../../docs/decisions/2026-10-12-hostd.md),
[manual](../../docs/hostd.md)). One per host, run as root, socket-activated.

```text
hostd serve                      # local socket, plus the Bus when noded is up
hostd serve --no-bus             # local socket only
hostd call host.info             # one request to the local socket
hostd call net.link.apply '{"link":"eth1","config":{"mode":"dhcp"}}'
```

`hostd serve` refuses to take over a socket another hostd is serving.

## Layout

| File | What |
|---|---|
| `src/local.rs` | the local socket: NUL-framed JSON, per-uid and per-connection limits |
| `src/caller.rs` | caller identity from `SO_PEERCRED`, a held pidfd, cgroup and namespaces, read per request |
| `src/service.rs` | the verb table, tiers, dispatch and the `authorise` gate |
| `src/policy.rs` | grants: loading (one `O_NOFOLLOW` open, root-only), matching, rate limits |
| `src/freeze.rs` | the kill switch, failing closed |
| `src/receipt.rs` | receipts, durable atomic writes, restore, the circuit breaker |
| `src/link.rs` | `net.link.apply`: validation, rendering, the transaction, undo |
| `src/networkd.rs` | networkd over D-Bus and Varlink, the event-driven health wait |
| `src/net.rs`, `src/info.rs` | the read verbs |
| `units/` | `hostd.socket`, `hostd.service` (sandboxed), `sysusers.conf` |

## Adding a write verb

1. A typed request with `deny_unknown_fields`, validated into a plan before
   anything else happens.
2. `service::authorise(host, verb, origin, limits)` with the allow-list keys
   the verb checks.
3. The transaction inside `service::write` (writer lock, own task): receipt
   first, then the change, a health check, rollback on failure,
   `receipt::record` for the breaker.
4. An undo path, or the verb doesn't ship.
5. The paths it writes added to `ReadWritePaths=` in `units/hostd.service`.
6. Registered in `docs/spec/bus/verbs.conf.mix` (class `intent`), in the
   manual, and reviewed on its own.

## Tests

`cargo test -p hostd`: no test needs root. The live gate for network writes
runs as root on a disposable networkd host, on dummy links only, and removes
everything it made.
