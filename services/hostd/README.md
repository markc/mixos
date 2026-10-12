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
| `src/txn.rs` | the transaction every write runs through: receipt first, conclude (keep or put back, breaker, durable outcome), recovery, undo |
| `src/link.rs` | `net.link.apply`: validation, rendering, the change, its proof |
| `src/wg.rs` | `net.wg.apply`: WireGuard netdev, network and key files, the handshake proof |
| `src/wgnl.rs` | WireGuard generic netlink (peer handshakes) and deleting a link |
| `src/networkd.rs` | networkd over D-Bus and Varlink, the event-driven health wait |
| `src/net.rs`, `src/info.rs` | the read verbs |
| `units/` | `hostd.socket`, `hostd.service` (sandboxed), `sysusers.conf` |

## Adding a write verb

1. A typed request with `deny_unknown_fields`, validated into a plan before
   anything else happens. Anything the plan reads from the host (a key, a
   file) is read inside the writer lock.
2. `service::write(host, origin, peer, verb, limits, ...)` with the
   allow-list keys the verb checks: it gates the call, takes the writer
   lock, gates it again and runs the change in its own task.
3. The change through `txn`: `FileChange::backup` for every file (with the
   mode and group a new file needs), `txn::begin`, the change and its proof,
   then `txn::conclude`. Never order the breaker or the receipt by hand.
4. A `reapply` for the verb in `txn::put_back`, so rollback, recovery and
   undo can make the host use the restored files. Without it the verb
   doesn't ship.
5. The paths it writes added to `ReadWritePaths=`, and any capability it
   needs to `CapabilityBoundingSet=`, in `units/hostd.service`, with the
   reason beside it.
6. Registered in `docs/spec/bus/verbs.conf.mix` (class `intent`), in the
   manual, and reviewed on its own.

## Tests

`cargo test -p hostd`: no test needs root. The live gate for network writes
runs as root on a disposable networkd host, on dummy links only, and removes
everything it made.
