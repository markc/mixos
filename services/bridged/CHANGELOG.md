# bridged Bus contract

## 0.1.0

First version. A Bus bridge to an external application's line-delimited JSON
control channel, one instance per application (`bridged@<app>`), answering
`<app>.call`, `.commands`, `.execute`, `.inspect`, `.screenshot` and
`.status` with rc 0 (the app's result), 10 (the app's error), 11 (connect,
auth, transport or timeout) or 12 (a bad request).

- Instance config 0.1.0 (`docs/spec/bridge/`): `<etc>/bridge/<app>.conf.mix`,
  `<etc>` = `$MIXOS_ETC` or `/etc/mixos`, or `$BRIDGED_CONFIG`; keys `host`
  (loopback only), `port`, `auth` (`token` or `none`), `token_file`,
  `timeout_s`, `connect_timeout_s`, `backoff_s`, `max_line`, `instance`.
- Start checks, also as `mix bridged.mix --check <app>`: the Bus service-name
  rule, and no name that a registered verb already starts with (read from the
  installed verb registry, failing closed). A refusal exits 2 with
  `NAME_INVALID`, `NAME_RESERVED`, `REGISTRY`, `CONFIG` or `TOKEN`.
- Verb registration in `docs/spec/bus/verbs.conf.mix` is pending the registry
  gate's instance-family mapping.

Ported from a private prototype (a serve citizen and its core, version
0.3.0), with the instance-config path rule, the serve-name guard and the
`--check` mode added, and its tests moved from a socat stand-in to one
written in Mix on `tcp_listen`.
