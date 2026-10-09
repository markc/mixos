# Changelog

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
