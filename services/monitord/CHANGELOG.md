# monitord contract changes

## 0.1.0 (draft)

- Add system-wide TCP snapshot and eleven durable rate/gauge history series;
  sockstat counts stay in the live snapshot.
- Add `monitord.connections`: unprivileged IPv4/IPv6 inet_diag, live rates,
  RTT, retransmits and owning user, on request with a 30 s hold. Peer
  addresses are open on the Bus. No connection history or capability.

- Read commands `monitord.status`, `monitord.snapshot`, `monitord.processes`,
  `monitord.process`, `monitord.series` and `monitord.history`, all
  queries.
- Retained topic `monitord.summary`, every 5 s.
- History: every sample for 10 minutes in memory; minute rollups for 7 days
  and hour rollups for a year in SQLite (`/var/lib/monitord/history.db`,
  schema 1).
- Command line: `serve`, `snapshot`, `ps`, `history`, and the local actions
  `signal` and `renice`, which run as the caller.
