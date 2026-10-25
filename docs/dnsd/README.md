---
title: dnsd
description: authoritative mesh DNS for the zones in zones.mix
---

# dnsd — authoritative mesh DNS

`dnsd` answers authoritative DNS queries over UDP and TCP for the zones in
an explicit strict-data `zones.mix` file. It refuses names outside its
authority and does not recurse. The private `dns-core` crate owns the zone
model, parsing, snapshots, persistence, resolver and wire codec.

```text
<prefix>/bin/dnsd --zones <prefix>/etc/dnsd/zones.mix --state <state>/dnsd/state --listen 127.0.0.1:5353
```

All three arguments are required. Repeat `--listen` to bind more than one
address. Wildcard binds are refused. The default `mixos` feature reads
`node.conf.mix` using `config` (including `MIXOS_ETC` and
`MIXOS_NODE_CONFIG`) and accepts the configured WireGuard IP or loopback.
If node configuration is absent or invalid, non-loopback binds require
`--allow-non-loopback-listen`, with a warning. The standalone build
(`--no-default-features`) always uses this explicit-flag posture.
The citizen retries unavailable bind addresses for up to 30 seconds.

The example zone schema is in
`services/dnsd/tests/fixtures/zones.mix`. Operators supply their own zones;
the fixture is test data. `SIGHUP` reloads the file, preserving the last
good snapshot on failure. Persisted serial floors retain their existing
v1 format, so a binary rollback can read state written by this port.

The `dnsd` Bus service exposes two read-only queries:

| Command | Reply |
|---|---|
| `dnsd.zone.snapshot` | Serial-excluded configuration hash (decimal string), sorted zone names and count |
| `dnsd.stats` | Response counters by DNS rcode |

DNS serving continues while the broker is unavailable. Registration
retries with bounded exponential backoff. Mutation commands are rejected.

`services/dnsd/units/dnsd.service` runs `<prefix>/bin/dnsd` as `dnsd`,
with UID and GID 506, `CAP_NET_BIND_SERVICE` and the original hardening.
Its config root is `MIXOS_ETC=<prefix>/etc`, with `dnsd/zones.mix` and
`dnsd/service.env` (`LISTEN=192.0.2.10:53`). Its state root is
`MIXOS_VAR=<state>`, with `dnsd/state`. The unit sets those roots
explicitly; changing the roots also requires overriding its argument and
environment-file paths. Node identity comes from
`<prefix>/etc/node.conf.mix`. The sysusers and tmpfiles fragments are
owned by the component. Installing these files does not activate DNS.

```text
cargo test --locked -p dnsd
cargo test --locked -p dns-core
cargo test --locked -p dnsd --no-default-features
cargo build --release --locked -p dnsd
```

Run Rust commands on the configured build worker. The integration prober
starts a temporary loopback daemon, drives real UDP and TCP queries and
retains the source load guard for timing assertions. `--version --json`
reports the package, version, build commit and timestamp.
