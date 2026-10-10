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
- The name may not start with `mixos-` (mix --serve strips it) or be one of
  the broker's (`noded`, `noded-*`, the session-name shape).
- The token is redacted from every error, result and `last_error`; the token
  file must be `0600`. `localhost` connects as `127.0.0.1`.
- A reply without a boolean `ok` drops the connection (rc 11, then backoff).
- The unit runs `bridged.mix --start <app>`: the start checks in process,
  then `mix --serve` as a child (Mix has no exec), exiting with the child's
  code. A documented refusal exits 2 and is never restarted
  (`RestartPreventExitStatus=2`); an unexpected start error exits 3, and it,
  a crash or a broken install is a rate-limited failure.
- Known gap: request writes are not bounded until Mix 0.112.1's `tcp_send`
  timeout lands (pending).
- Verb registration in `docs/spec/bus/verbs.conf.mix` is pending the registry
  gate's instance-family mapping.

Ported from a private prototype (a serve citizen and its core, version
0.3.0), with the instance-config path rule, the serve-name guard and the
`--check` mode added, and its tests moved from a socat stand-in to one
written in Mix on `tcp_listen`.
