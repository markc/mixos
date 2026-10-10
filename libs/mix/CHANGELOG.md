# Changelog

## Unreleased

### Added

- Server-side TCP. `tcp_listen(host, port[, {backlog}])` binds and listens
  and returns a listener handle; `host` is required (`"127.0.0.1"` is the
  normal choice) and port 0 takes an ephemeral port. `tcp_local_addr(handle)`
  returns `{host, port}` for a listener or connection.
  `tcp_accept(listener[, {timeout}])` returns an ordinary connected handle,
  or nil on timeout; in a Class C body it waits on native readiness.
  `tcp_on` on a listener delivers one `accepted: {handle, peer}` event per
  connection, and `tcp_accept(source)` reads that source outside serve mode.
  `tcp_close` closes a listener. Listeners and the connections they accept
  belong to the evaluator generation and close when it retires. Errors
  carry stable codes (`TCP_LISTEN_ADDR_IN_USE`, `TCP_LISTEN_PERMISSION`,
  `TCP_LISTEN_ADDR_UNAVAILABLE`, `TCP_LISTEN_ADDRESS`, `TCP_LISTEN_ARGUMENT`,
  `TCP_ACCEPT_HANDLE`, `TCP_HANDLE_LIMIT`, `TCP_LISTENER`); new listeners
  and accepts refuse past 1024 live TCP handles.

## 0.111.0

### Added

- `script_path()` returns the entry script's absolute, symlink-resolved path,
  the same answer as `realpath($0)`, fixed when the script starts (a later
  `chdir()`, including one in the prelude, does not move it). It is nil under
  `mix -c`, in the REPL, for `mix -` and when the path cannot be resolved at
  start. A `require`d module, even during its top-level init, gets the entry
  script's path.

### Fixed

- Lambdas created inside a served `on` handler keep the handler's local
  variables when they run later (they faulted with `undefined variable`). A
  module first required inside a handler still reads its globals live.
- `password_hash` sha512-crypt output is now verifiable by glibc `crypt(3)` and
  Dovecot.
- `chr()` and `ord()` structured error codes are pinned by tests.

## 0.109.1

Initial MixOS transplant of the current language library and regression corpus.
Language syntax, builtin contracts and evaluator behaviour retain the source
version. Environment and project roots use MixOS names. SSH helpers retain the
installed Mix path used by existing Cosmix nodes during the migration.
