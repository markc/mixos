# Changelog

## Unreleased

### Added

- `date_format(ts[, fmt][, {utc: true}])` formats in UTC instead of local
  time, so an ISO 8601 `Z` stamp is no longer local wall-clock time with a
  UTC label (the only workaround was `TZ=UTC` around the whole process).
  `fmt` nil selects the default pattern; an unknown option key raises
  `OPTION_INVALID`, a non-bool `utc` raises `TYPE_MISMATCH`.
- `idle_timeout` on `http_get`, `http_post` and `http_request`, independent
  of the existing whole-call `timeout` (0 disables that cap).
- `http_stream(method, url, on_line[, body, headers, opts])` delivers SSE
  lines during the response; returning false closes promptly. Scoped async
  reads release the response on callback errors, cancellation and evaluator
  retirement. Failures return status 0, received byte counts and partial
  bytes, with `HTTP_IDLE_TIMEOUT`, `HTTP_STREAM_CUT` or `HTTP_TIMEOUT`.

### Changed

- A `fn` or `function` named after a builtin is refused at its definition
  with `FN_SHADOWS_BUILTIN`. A top-level definition (in a script or a
  `require`d module) is refused before the first statement runs; a nested
  one when control reaches it. The builtin always won at the call site, so
  the function could never be called by name, and `mix --check` does not run
  lint: a script defining `fn run($argv)` ran on and failed with
  `run(): argument 1 (cmd) must be string`. `--serve` extension names such
  as `context` are not refused (lint still warns, MIX-W2313). A sweep of
  about 24,000 definitions across the hub, the MixOS tree and the installed
  scenes found no live script affected.

### Fixed

- HTTP idle/stream responses now negotiate gzip only, regenerate Host on
  redirects, and match ureq's literal URL-userinfo authentication. Buffered
  successes keep their existing keys; `stopped` is stream-only and received
  counts are limited to streams and buffered failures. Preserve bare CR
  bytes in an unterminated line. TLS classification inspects IO payloads.

- MIX-E1303 now covers the higher-order builtins (`count`, `map`, `filter`,
  `sort_by`, …). `fn count($x)` linted clean and was silently dead. Lint and
  runtime share one name set, `builtins::is_reserved_callable`.

## 0.117.0 — 2026-10-21

### Added

- `analyzer::Analysis.functions` reports named function declarations with
  `name`, `file` and `line`, including nested and duplicate definitions but
  excluding quoted and remote source. `mix lint --json` and `--data` expose
  this list in schema 3; every existing key and diagnostic retains its meaning.
- `mix lint` reads in-file pragmas: `-- lint: globals a, b` declares
  injected names, `-- lint: ignore CODE` suppresses CODE on the next line
  holding code (or on its own line when written after code) and
  `-- lint: ignore-file CODE` suppresses it file-wide. A bare or unknown
  pragma is MIX-E1601; a suppression that matches nothing is the note
  MIX-D3019. An `ssh_mix` body reads its own pragmas.
- MIX-W2312: a bareword `null`, `None`, `undefined`, `NULL` or `nullptr`
  used as a value is the string "null"; the warning carries a quick fix to
  `nil` where the word is surely a value. `mix lint --fix` applies quick
  fixes; `--json` reports each as `fix`. `Diagnostic` gains a `fix` field.
- MIX-W2313: a function named after one of the `mix` binary's shell
  extensions (`ai`, `ai_diagnose`, `context`). Lint now knows those
  extensions (`MIX_SHELL_EXTENSIONS`, `AnalyzerConfig::extensions`), so
  calling one is no longer MIX-E1102.
- MIX-W2314 / MIX-W2315: member calls into a module bound once to a literal
  `require()` path are checked against its exports, for arity and for a
  missing name. Warnings in this release, while the check is new.
- The parser names the fix for `let i = 0`, `var`/`const`, and a bareword
  `i = …` ("Mix has no `let`: write `$i = …`").
- Add the `archive` feature with the `tar_list`, `tar_unpack` and `tar_pack`
  builtins. Archives use zstd (default), gzip or no codec. `tar_unpack`
  validates every member, extracts into a staging directory and renames it
  into place only after the stream verifies. See `mix man tar`.
- Harden archive builtins: count trailing padding against the shared decoded
  stream budget, reject oversized declared files before creation and bound
  copying, cap GNU/PAX metadata payloads before buffering, and refuse unread
  gzip input or additional members. Packing captures capabilities only with
  `keep_special_bits:true` and walks pinned directory/file descriptors without
  following replacement symlinks. Rollback restores directory access and
  reports cleanup failures. Empty/truncated zstd headers now raise
  `ARCHIVE_STREAM_CORRUPT`. GNU sparse entries are refused; `tar_list` also
  accepts `max_entries`. Each fix has targeted archive regression coverage.
- Archive review fixes. `tar_unpack` restores only `user.*` xattrs by
  default: `security.capability` needs `keep_special_bits:true`, and
  `trusted.*`, POSIX ACLs (`system.*`) and the rest of `security.*` need the
  new `privileged_xattrs:true`. Retained xattr bytes are bounded across the
  archive by the new `max_xattr_bytes` (16 MiB). Staging has an unpredictable
  name and is held by descriptor; members are created one O_NOFOLLOW
  component at a time relative to it, and the commit refuses to replace
  anything that appeared at `dest` (RENAME_NOREPLACE). Owner, mode and mtime
  failures are no longer discarded: an owner that cannot be applied is listed
  in the receipt's new `warnings`, and then its setuid/setgid bits and
  capability are dropped; mode and mtime failures raise. PAX global headers
  (`git archive`) are accepted as metadata (receipt `metadata`, list kind
  `"metadata"`). A bare relative dest or output name means the current
  directory. `tar_pack` refuses an output inside its source and raises when
  a file changes size while it is packed.
- Archive review, round 2. `tar_unpack` refuses a dest parent that another
  user could write (group/other-writable without the sticky bit, or owned by
  neither the caller nor root) before creating anything, and checks the new
  staging directory is the caller's, `0700` and empty. Without
  `RENAME_NOREPLACE` the commit now fails closed and rolls back. Implicitly
  created parent directories count against `max_entries` (receipt
  `implicit_dirs`). zstd content checksums are verified on unpack and list,
  and `tar_pack` now writes one.
- `analyzer::Analysis` carries `handlers`: one `HandlerDeclaration` (`command`,
  `file`, `line`) per `on <command>` statement in the parsed AST, in source
  order, with duplicates kept. `analyze()` collects them with the existing
  statement walker. Remote `ssh_mix` bodies are not included. `mix lint --json`
  and `--data` report the same list as an additive `handlers` field. The data
  is structural: it does not prove that a handler runs or is reachable.
- `mix_tokens(source[, {comments: true}])` returns the real lexer's token
  stream as `[{kind, text, line, col}]`, with the exact source text of each
  token and, on request, the `#` and `--` comments. A tool that reads Mix
  source no longer re-derives the grammar. Source that does not lex raises
  `MIX_SOURCE_LEX` with `details {line, col}`. Backed by the new
  `Lexer::source_tokens`. See `mix man syntax`.
- `fsync(path[, {recursive}])` makes a file, a directory (its entries, after a
  `rename` or `remove`) or a whole tree durable, without `sync -f`'s flush of
  the whole filesystem. It returns the number of inodes synced; a failure
  raises `FSYNC_FAILED` naming the path.
- `dns_query(name, type[, {server, port, timeout, tcp, recurse}])` asks one
  DNS server one question and returns the rcode, the `aa`/`ra`/`tc` flags and
  every section with TTLs. Timeouts and unreachable servers come back as
  `{ok: false, error_code}`. It is the new `dns` feature (hickory-proto, the
  version mail-auth already uses); the `mix` binary turns it on.
- `walk(dir, {strict: true})` raises `WALK_UNREADABLE`, naming the first
  directory it cannot read or entry it cannot stat, instead of skipping it.
- `chown(path, uid, gid, {follow_symlinks: false})` changes a symlink itself
  (`lchown`).
- `flock` and `fcntl_lock` take `create: false`, which refuses a missing lock
  file with `LOCK_FILE_MISSING` and creates nothing.
- `tar_unpack` and `tar_list` accept GNU tar's `./` member prefix (`tar -C
  dir .`); a `./` directory entry's metadata is applied to `dest`. Relative
  symlinks may climb with leading `..` while they stay inside `dest`, judged
  from the link's own directory (`demo/lib/alias -> ../bin/demo`). Gzip input
  may have several members, as `pigz` and `cat a.gz b.gz` write.
- `mix man sqlopen` documents the SQLite builtins and their modes.

### Changed

- `tar_unpack`'s `numeric_owner` now defaults to `true` only for root, as GNU
  tar does. An ordinary user's unpacked files keep their own uid and gid; a
  non-root chown could still give a file any of the caller's groups. Pass
  `numeric_owner: true` for the old behaviour. A setuid/setgid bit or
  capability is now kept only where the file ends up with the owner the
  archive named, also when owners are not restored.
- A symlink target may no longer pass through another symlink the archive
  creates, in either order, and `..` may only lead a target: `a/../b` is
  refused.
- A shared `flock` or `fcntl_lock` opens an existing file read-only, so it no
  longer needs write access to the lock file.
- `sqlopen` raises `OPTION_INVALID` for a mode other than `"ro"`, `"rw"` or
  `"rwc"` (`"rwc"` is new: read-write, creating the file, the same as `"rw"`).
  An unknown mode used to open read-only, so `"rwc"` could not create a
  database. A read-only open of a missing file raises `SQL_NO_DATABASE`
  naming `"rwc"`.
- A member call `$m.name(...)` on a map holding a function member calls
  that member in every mode, `--serve` included; an extension (the shell's
  `context()`) no longer wins there.
- MIX-W2304 names the result shape and suggests the field a bash, Python or
  Node spelling means (`code`, `rc`, `status` → `exit_code`).
- The default remote mix binary is now `/opt/mixos/bin/mix` (was the legacy
  install path's mix). It applies to `ssh_mix`, `ssh_mix_many`, `ssh_exec`,
  `ssh_run`'s `env_transport: "mix"` and `send_mail`'s `host` hop whenever
  neither `mix_bin` nor `MIX_REMOTE_BIN` is set. For a host still on
  the old install path, pass `mix_bin` or set `MIX_REMOTE_BIN` to its mix.

### Fixed

- A `send` after a failed Bus connection retries within its timeout,
  re-reading `MIXOS_NODED_URL` and node configuration. Without `timeout=`,
  each lane's connect is bounded by 5 seconds (up to 10 seconds if both the
  verified Unix and anonymous TCP lanes stall). A stale probe cannot
  replace a newer connection, and reconnecting does not hold the state lock.
- SIGINT still ends a non-interactive evaluation when a `catch` keeps
  computing: after a cleanup grace the interrupt becomes standing. Nested
  catches and `finally` control flow cannot swallow it. A served citizen
  keeps its graceful deregistration and exit status 0.
- `read_file`, `write_file` and `append_file` yield while waiting on a FIFO,
  so served citizens still answer other commands and `QUIT`. Reads honour the
  string limit, and the FIFO path keeps the normal builtin arity rules.
- `fs_watch` recovery (after a lost or overflowed event stream) re-arms each
  directory watch in place instead of unwatching and watching it again, so a
  change that lands during the repair (the close-write of a save racing the
  overflow) is still reported.
- `mix lint` no longer reports MIX-E1101 for a variable assigned in a piped
  statement (`$v = 1 | cat`).
- `mix lint` MIX-W2307 (send result never checked) now counts a status read
  inside an `if` condition, a map value, a nested call argument or a string
  interpolation. A correctly checked send no longer warns.
- MIX-W2307 treats every writer of `$rc`, `$result` and `$reply` as an
  overwrite: `publish()`, `sh`, `$(...)`, pipes to external commands, user
  assignments, address-block calls and calls to functions that send. A send
  followed by one of these, and no read before it, now warns.
- MIX-W2307 is conservative by construction: a construct whose effect on the
  status the analyser cannot prove absent counts as an overwrite. A call is
  proven non-writing only for a function defined in the file whose body and
  parameter defaults do not write, or for one of the pure builtins (no I/O,
  process, bus, network, clock or function-valued argument). Any other builtin,
  value call (`$f()`), method call, `include` or `source`, and any name
  defined nowhere in the file, now overwrites. A name that is also bound as a
  variable or parameter is a barrier, because a callable binding runs before a
  named function. A bare word that names a function is a call. A function that
  can `exit`, `panic`, `die` or fail uncaught ends its caller's path, so a read
  after that call does not discharge a send. String and heredoc `${...}`
  fallbacks (`${x ?? sh "..."}`) are analysed as the program they are, and an
  index is evaluated before its object. This can add warnings for code that is
  correct but calls a non-pure builtin between a send and its check.

## 0.116.0

### Added

- The remote mix binary is configurable. Every builtin that drives a
  remote mix (`ssh_mix`, `ssh_mix_many`, `ssh_exec`, `ssh_run`'s
  `env_transport: "mix"`, `send_mail`'s `host` hop) resolves it in one
  place: the `mix_bin` option, else the local env var `MIX_REMOTE_BIN`
  (empty counts as unset), else the old install path's mix, the unchanged
  default. The value is interpolated into the remote command string, so
  it must be an absolute path or a bare name using only `[A-Za-z0-9_./+-]`;
  anything else raises `OPTION_INVALID` locally before ssh runs. A host with
  mix only at `/opt/mixos/bin/mix` used to fail with exit 127.

### Changed

- `ssh_exec`'s `remote_mix` option is now validated the same way (it was
  interpolated unchecked) and is an alias of `mix_bin`; passing both
  raises.

## 0.115.0

### Added

- `unix_connect(path[, {timeout, frame}])`: a client for a local
  unix-domain stream socket, in `tcp_connect`'s handle space, so
  `tcp_send`, `tcp_recv`, `tcp_recv_line`, `tcp_on` and `tcp_close` take it.
  The connect is bounded by `timeout` and fails at once on a full backlog
  (`UNIX_CONNECT_BUSY`). `frame: "nul"` makes a line a NUL-terminated
  frame, the framing of systemd's Varlink services and of hostd, for
  `tcp_recv_line` and `tcp_on {frame: "line"}` alike. Refusals carry
  `UNIX_CONNECT_*` codes. `mix man system` documents it.

### Changed

- One frame splitter serves `tcp_recv_line` (blocking and Class C) and
  `tcp_on` line mode, keyed to the connection's delimiter. One trailing CR
  is still stripped from an LF line.

### Fixed

- `max` now caps a complete line too. A line longer than `max` whose
  terminator arrived in the same read used to be delivered, so whether it
  was refused depended on packet timing. All three paths (`tcp_recv_line`
  blocking and Class C, `tcp_on` line mode) now refuse it the same way.

## 0.114.0

### Changed

- Released with mix-shell 0.114.0, whose usage-statistics store is now an
  append-only JSONL store (`mix man stats`). The language and its builtins
  are unchanged.

## 0.113.1

### Fixed

- An `on` header reads a hyphenated name whole, like a `send` target:
  `on dopus-test.command.executed` handles that topic. Before, the header
  stopped at the hyphen, so the handler registered as `dopus` with
  `-test.command.executed` as its body: it parsed, linted clean and never
  fired. Leading-zero segments (`on svc-01.ping`) lex as part of the name,
  as they do after `send`.

## 0.113.0

### Added

- `mime_type(path[, {content}])`: a file's type from the freedesktop
  shared-mime-info database, decided by GLib's rules, so it agrees with
  `gio info` and the file manager (checked against GIO on 5,047 files).
  Returns `{mime, by, uncertain, exists, parents}`. Behind the new `mime`
  feature, through the new `mimedb` crate. `mix man io` documents it.

## 0.112.1

### Added

- `tcp_send(handle, payload[, {timeout}])`: one deadline bounds the whole
  send, however many partial writes a slow reader forces (seconds, 0 = wait
  forever). On expiry it raises `TCP_SEND_TIMEOUT` with
  `details: {written, total}` and retires the handle; bytes already written
  stay written. The wait honours SIGTERM and Ctrl-C. On a `tcp_on` handle
  the timeout is the receipt deadline. Without the option `tcp_send` is
  unchanged.

### Fixed

- SIGTERM now ends a blocking `tcp_accept`, `tcp_recv`, `tcp_recv_line` or
  `ws_recv` at once, as it ends `sleep()`. On unix these numeric waits now
  wait on the async runtime instead of blocking the evaluator's thread, so
  the shell's SIGTERM and Ctrl-C handling can cancel them. Before, a script
  looping on `tcp_accept($l, {timeout: 0.05})` ignored SIGTERM until the
  15 s backstop forced it out. Options, results and error messages are
  unchanged.
- A receive returns data already buffered before it judges the deadline, so
  a very short timeout no longer returns nil while a line sits in the buffer.
  Traffic that delivers nothing (a WebSocket Pong flood, a line trickled with
  no newline) now counts against the deadline and the interrupt flag, so a
  finite timeout ends on time and SIGTERM still ends an untimed wait.
- A very large receive timeout in a Class C `tcp_recv`/`ws_recv` waits
  forever instead of overflowing the deadline.

## 0.112.0

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
- `on @.command` registers a handler under the serve name: the unquoted `@` is
  replaced by the `--serve` name (the value `serve_name()` returns) when the
  handler registers, so one generic script run as `--name photo` answers
  `photo.command` and a second instance as `--name other` answers `other.command`.
  Dispatch, HELP and handler listings show the resolved name; the AST keeps
  `@.command`. Outside `mix --serve` the statement raises
  `SERVE_PREFIX_OUTSIDE_SERVE`. Each segment after `@.` must be a bare name,
  quoted or not. `on a.@.b`, `on @`, `on @x.y`, a quoted `on "@.x"`, a quoted
  segment that isn't a bare name (`on @."a.b"`) and `@` in an expression are
  parse errors (an unquoted `@` was a lexer error before). Positions that read
  raw source text now accept `@` as an ordinary character where it used to
  fail lexing: a bareword `source`/`include` path (`source ./foo@bar.mix`) and
  the command after `|` (`print(1) | cat @foo`).

### Fixed

- HTTP 1xx, 204 and 304 responses are bodyless on every method, not only
  HEAD. A conditional GET answered 304 with `Content-Encoding: gzip` no longer
  fails with `HTTP_BODY`, so ETag polling works.

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
installed Mix path used by existing predecessor nodes during the migration.
