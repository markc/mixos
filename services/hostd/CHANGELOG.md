# hostd contract changes

## 0.1.0 (draft)

- Read verbs `host.info`, `net.state`, `hostd.status`, `hostd.receipt` on the
  local socket and the Bus; retained `hostd.info` topic.
- Control verbs `hostd.freeze` (root on the host, or a grant) and
  `hostd.thaw` (root on the host only; also lifts grant suspensions), local
  only.
- Write verbs `net.link.apply` and `hostd.undo`, local only, behind standing
  grants in `/etc/hostd/policy.mx` (schema 1): caller by uid and verified
  cgroup, verb families, argument allow-lists, expiry, per-caller rate with a
  grant-wide cap. Receipts, health-checked rollback, and a circuit breaker
  that suspends a grant after two rollbacks in a row.
- Local socket: NUL-framed JSON, kernel caller identity read per request,
  per-uid connection cap, connection lifetime, read and write deadlines.
- `hostd.status` adds `held_grants`: each grant the caller holds, in full.
- Write verb `net.wg.apply`: networkd-native WireGuard, key made and kept by
  hostd (0640 root:systemd-network, never returned), the handshake dead-man
  check. The unit gains `CAP_CHOWN` and `CAP_NET_ADMIN` for it.
