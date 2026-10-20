# Changelog

## 0.117.0

### Changed

- `mix man` reads the manual built into the binary: every `docs/mix/*.md`
  page is packed into one zstd frame at build time, so a binary copied alone,
  with no checkout and no network, has its whole manual. With no
  `MIXOS_MAN_*` variable set the built-in page wins (it documents exactly this
  binary), then files on disk, then the online copy. `MIXOS_MAN_SOURCE=local`
  puts files on disk first, then the built-in manual, and never uses the
  network. `MIXOS_MAN_SOURCE=auto` or `MIXOS_MAN_URL` keeps the online-first
  order, with the built-in manual before a stale cached copy. `mix doctor`
  and `mix apropos` count and search the built-in pages. The compiled-in
  path to the build checkout's `docs/mix` is no longer a lookup candidate.

### Fixed

- A foreground job stopped (^Z) just after its exec is now stopped, not
  reported as "job stopped before exec acknowledgement" and aborted. The
  launch checked for the stop before reading the exec acknowledgement that
  was already waiting. This was the intermittent failure of the
  `owned_editor_pty` test
  `foreground_job_ctrl_z_fg_and_nested_editor_stop_preserve_draft`.

## 0.116.0

### Added

- Released with mix 0.116.0: the remote mix binary for `ssh_mix` and the
  other remote-mix builtins comes from the `mix_bin` option, else
  `MIX_REMOTE_BIN`, else the old install path's mix (`mix man remote`).

## 0.115.0

### Added

- Released with mix 0.115.0: `unix_connect()` opens a local unix-domain
  stream socket, with NUL-terminated frames for Varlink and hostd
  (`mix man system`).

## 0.114.0

### Changed

- Usage statistics are an append-only JSONL store. A run's exit appends one
  record to `runs/YYYY-MM-DD.jsonl` with a single write and does nothing
  else, so recording costs nothing measurable: `mix -c 1` went from 151 ms
  to 52 ms on a host with a busy history, the same as with stats off. Raw
  day files are never rewritten; `cache/` holds disposable per-day
  aggregates checked against the exact raw file; reports filter by each
  bucket's own date. `mix stats query` runs SQL over the old `mix.db` tables
  built in memory, so on a large history it now takes about a second.
- `clear` and `reset` append an operation anchored to storage positions, not
  a clock, under `ops.lock`. They refuse while the old weekly documents
  (`current.json`, `YYYY-WNN.json`) exist; those are read in place, never
  moved, and their `stats.lock` is never taken, so an older mix still writing
  them keeps working. `mix man stats` documents the store.

### Fixed

- A session task no longer inherits file descriptors the shell itself
  inherited without close-on-exec.

## 0.113.1

### Fixed

- Built on mix 0.113.1: `on dopus-test.command.executed` (a hyphenated
  handler name) now fires; before, it parsed and never did. `mix man bus`
  documents it.

## 0.113.0

### Added

- Built on mix 0.113.0 with the `mime` feature: `mime_type()` names a
  file's type as the desktop does. `mix man io` documents it.

## 0.112.1

### Fixed

- Built on mix 0.112.1: SIGTERM and Ctrl-C end a blocking `tcp_accept`,
  `tcp_recv`, `tcp_recv_line` or `ws_recv` at once, `tcp_send` takes a
  whole-send `{timeout}` (`TCP_SEND_TIMEOUT`), and receives serve buffered
  data first while traffic that delivers nothing counts against the
  deadline. `mix man system` documents them.

## 0.112.0

### Added

- Built on mix 0.112.0: server-side TCP (`tcp_listen`, `tcp_accept`,
  `tcp_local_addr`, `tcp_on` on a listener) and `on @.verb`, which registers
  a handler under the `--serve` name so one generic script serves any
  instance. `mix man serve` and `mix man system` document both.

### Fixed

- The serve reload lifecycle test's TERM-ignoring fixture creates the trace
  file itself. The citizen spawned it before the first trace line existed,
  so a fast start panicked on open and the grace test timed out.

## 0.111.0

### Fixed

- An interrupted run exits with status 130 (128+SIGINT) derived from the signal
  actually received, never from error text: an ordinary error whose message
  mentions `interrupted` now fails with its own diagnostic instead of exiting 0.
- SIGINT/SIGTERM are honoured during `--serve` startup (stalled broker
  registration, CPU-bound preludes, reloads) and during `-c`, script and
  `.mixrc` prelude loading. A caught interrupt cannot end in success, and
  `--result-fd` always reports it as an error frame.
- `script_path()` is resolved before the prelude runs in script and serve modes.

## 0.109.4

Initial MixOS shell transplant. Language semantics retain the source version.
Application transport is native ABP through noded. Runtime directories use the
shared config rule; installed manuals are available offline. Cross-process
native-session handoff bytes retain the frozen compatibility contract.
