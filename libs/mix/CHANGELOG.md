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
  `TCP_ACCEPT_HANDLE`, `TCP_HANDLE_LIMIT`, `TCP_LISTENER`).
- The handle limit is server-side: at most 1024 listeners and accepted
  connections are live at once, each holding a slot until it closes, and
  `tcp_listen` and `tcp_accept` refuse with `TCP_HANDLE_LIMIT` past it.
  `tcp_connect` is not limited and behaves as before.
- A blocking `tcp_accept` wakes promptly on Ctrl-C through a SIGINT wake
  socket that `interrupt::init` installs, and rechecks the interrupt flag at
  least every 250 ms, so no waiter misses it.
- `on @.verb` registers a handler under the serve name: the unquoted `@` is
  replaced by the `--serve` name (the value `serve_name()` returns) when the
  handler registers, so one generic script run as `--name photo` answers
  `photo.verb` and a second instance as `--name other` answers `other.verb`.
  Dispatch, HELP and handler listings show the resolved name; the AST keeps
  `@.verb`. Outside `mix --serve` the statement raises
  `SERVE_PREFIX_OUTSIDE_SERVE`. Each segment after `@.` must be a bare name,
  quoted or not. `on a.@.b`, `on @`, `on @x.y`, a quoted `on "@.x"`, a quoted
  segment that isn't a bare name (`on @."a.b"`) and `@` in an expression are
  parse errors (an unquoted `@` was a lexer error before). Positions that read
  raw source text now accept `@` as an ordinary character where it used to
  fail lexing: a bareword `source`/`include` path (`source ./foo@bar.mix`) and
  the command after `|` (`print(1) | cat @foo`).

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
