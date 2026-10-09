# Changelog

## Unreleased

### Fixed

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

## 0.109.1

Initial MixOS transplant of the current language library and regression corpus.
Language syntax, builtin contracts and evaluator behaviour retain the source
version. Environment and project roots use MixOS names. SSH helpers retain the
installed Mix path used by existing Cosmix nodes during the migration.
