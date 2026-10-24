---
title: hostd — changelog
description: Release notes of the hostd component, mirrored from services/hostd/CHANGELOG.md.
generated: by docs/_build/gen-site.mix from services/hostd/CHANGELOG.md; do not edit
---

# hostd contract changes

> Mirrored from [`services/hostd/CHANGELOG.md`](https://github.com/markc/mixos/blob/main/services/hostd/CHANGELOG.md), where releases update it. The component's page is [hostd](README.md). This page describes the current `main` branch of MixOS.

## 0.1.0 (draft)

- Read commands `host.info`, `net.state`, `hostd.status`, `hostd.receipt` on the
  local socket and the Bus; retained `hostd.info` topic.
- Control commands `hostd.freeze` (root on the host, or a grant) and
  `hostd.thaw` (root on the host only; also lifts grant suspensions), local
  only.
- Write commands `net.link.apply` and `hostd.undo`, local only, behind standing
  grants in `/etc/hostd/policy.mx` (schema 1): caller by uid and verified
  cgroup, command families, argument allow-lists, expiry, per-caller rate with a
  grant-wide cap. Receipts, health-checked rollback, and a circuit breaker
  that suspends a grant after two rollbacks in a row.
- Local socket: NUL-framed JSON, kernel caller identity read per request,
  per-uid connection cap, connection lifetime, read and write deadlines.
- `hostd.status` adds `held_grants`: each grant the caller holds, in full.
- Write command `net.wg.apply`: networkd-native WireGuard, key made and kept by
  hostd (0640 root:systemd-network, never returned), the handshake dead-man
  check. The unit gains `CAP_CHOWN` and `CAP_NET_ADMIN` for it.
- Write commands `net.wifi.scan` and `net.wifi.connect` through iwd: no files,
  the passphrase only through a one-connect agent, put back by forgetting
  and reconnecting.
