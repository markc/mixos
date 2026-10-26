# hostd contract changes

## 0.1.0 (draft)

- Read commands `host.info`, `net.state`, `hostd.status`, `hostd.receipt` on the
  local socket and the Bus; retained `hostd.info` topic.
- Control commands `hostd.freeze` and `hostd.thaw` accept any local caller.
- Write commands accept every same-host Bus or Unix-socket caller, with
  receipts, health-checked rollback and per-caller and host-wide rate limits.
  Mesh-origin writes stay refused.
- Desk power commands take an explicit `{desk}` argument. Unit control takes
  `{unit, action}`. Socket access is open to all local accounts (mode 0666).
- Local socket: NUL-framed JSON, kernel caller identity read per request,
  per-uid connection cap, connection lifetime, read and write deadlines.
- `hostd.status` reports freeze state and caller origin.
- Write command `net.wg.apply`: networkd-native WireGuard, key made and kept by
  hostd (0640 root:systemd-network, never returned), the handshake dead-man
  check. The unit gains `CAP_CHOWN` and `CAP_NET_ADMIN` for it.
- Write commands `net.wifi.scan` and `net.wifi.connect` through iwd: no files,
  the passphrase only through a one-connect agent, put back by forgetting
  and reconnecting.
