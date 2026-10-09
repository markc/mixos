# Changelog

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
