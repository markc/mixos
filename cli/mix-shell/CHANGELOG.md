# Changelog

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
