# tools

Developer tooling that never ships: gates, generators and build helpers. Every
script is Mix (`*.mix`) and runs from the repository root with
`/opt/mixos/bin/mix tools/<name>.mix`.

| Script | What it checks or does |
|---|---|
| `component_index.mix` | Validates component ownership and writes the public component map. `--check` compares without writing. |
| `verb_registry_gate.mix` | Checks the Bus verb registry (`docs/spec/bus/verbs.conf.mix`, AGENTS.md §5) against the verbs the code answers. |
| `verb_registry_gate_test.mix` | Regression fixtures for the verb gate: 35 cases run against the mini trees in `fixtures/verb_gate/`. |

## verb_registry_gate.mix

Usage: `/opt/mixos/bin/mix tools/verb_registry_gate.mix [--root DIR] [--registry FILE]`

- `--root DIR` is the tree to check (default: the repository this file lives in).
- `--registry FILE` is the registry, absolute or relative to `--root` (default
  `docs/spec/bus/verbs.conf.mix`).

The registry is schema 3: `verbs` entries `{name, owner, version, status,
note?, declared_by?}` and `families` entries `{pattern, owner, version,
status, note?}`. `<app>` in a pattern takes the first segment; `*` takes
exactly one later segment.

**Registry checks:** `registry-shape` (top-level keys, schema 3, `declared_by`
only on verbs), `registry-entry` (names, patterns, owners, versions and
statuses well formed), `registry-protocol` (no protocol word registered),
`registry-duplicate`, `registry-overlap` (a concrete verb also matched by a
family) and `family-overlap` (two families that can match one name).

**Code checks,** for every registration found:

- `bad-name`: a registration literal that is not a valid name. Nothing is
  dropped silently; protocol words are exempt.
- `unregistered`: a code verb, prefix or suffix matching no registry entry.
- `double-registered`: a code verb matching more than one entry. Exactly one is
  required.
- `retired-in-code`: a retired name or pattern still answered.
- `not-in-code`: a registry entry with no code registration.
- `not-answered`: a concrete verb declared in a `const VERBS` table that no code
  answers.
- `owner-mismatch`: the answering component is not the registered owner, or
  the declaring component is not `declared_by` (or the owner, when absent).
- `owner-conflict`: two components answer the same verb.
- `ambiguous-arm`: a parse arm (see below) constructs more than one
  `SessionCommand::X` variant. One arm answers one variant.

**Registration shapes** (the gate reads nothing else):

- `decl`: an entry of a `const VERBS` table, in any file. The declaring
  component is the file's directory under `apps/`, `services/`, `libs/` or `cli/`.
- `dispatch`: an arm of a `match` whose scrutinee is `verb`, `command`, `cmd`,
  `method` or `command_name()` (optionally `.as_str()`, `.as_ref()`,
  `.as_deref()`, or a field such as `cli.command`). The `match` may sit anywhere
  on its line, such as `Some(match verb {`. Arm heads may start with `|` or `(`.
  A tuple scrutinee is dispatch when any component is.
- `guard`: a bare `verb == "x"`, `command == "x"`, `cmd == ...` or `method ==`
  comparison. A receive-side `msg.command == ...` is not counted.
- `prefix`: a guard arm `c if c.starts_with("p.")`, answering every verb under `p.`.
- `on`: a shipped `.mix` `on <name>` handler, outside tests.
- `suffix`: an entry of `SUFFIX_TABLES` (`libs/toolkit/src/drive.rs`): a suffix under `<app>.`.
- `parse`: an arm of a `dispatch` match in a `PARSE_FILES` file
  (`libs/bus/src/native_session/bootstrap.rs`). It declares the verb, with the
  declaring component as the file's directory (`bus`), and names the
  `SessionCommand::X` variant it parses to. It answers nothing. The variant is
  read from the lexically stripped text of the arm, so a comment or literal that
  names a variant does not count as a construction.
- `variant`: an arm head `SessionCommand::X =>` in a dispatch match. It answers
  a `parse` verb whose variant is `X`. A `parse` verb with no owner variant arm
  is `not-answered`, and an owner other than the answering component is
  `owner-mismatch`. The session verbs are `owner noded`, `declared_by bus`.

**Method:** a `match method` scrutinee or `method ==` guard is read like `verb`,
but HTTP method tokens (`GET`, `HEAD`, `POST`, ...) are not verbs there. The
real tree's `libs/mix/src/builtins.rs` compares `method == "HEAD"` for an HTTP
request.

**Lexical stripper.** Every scanned `.rs` file is read through one Rust lexical
stripper (`strip_rust`), which returns two views with the same columns:

- `code`, for all structure: brace counting, `const VERBS` tables, the
  `#[cfg(test)]` module, arm heads, `=>`, `SessionCommand::X` variants and the
  dispatch scrutinee. It blanks `//` comments, nested `/* */` comments, string,
  byte-string and raw-string contents (`r"..", r#".."#`, any number of `#`),
  multiline strings and char literals. A string's quote marks stay, so an arm
  head that starts with a literal still reads as one.
- `keep`, for names: comments are blanked and literal text is kept. A name is
  read from `keep` only where `code` shows a real string at the same columns
  (`literal_names`), or where the structure matched in `code` also matches the
  name pattern over `keep` (`structured`: the `starts_with("p.")` prefix arm and
  the `verb == "x"` guard). An arm's head is the text up to the `=>` found in
  `code`. So a name inside a raw string, which `code` blanks, is never a
  registration or an answer.

`.mix` files get the same two views (`strip_mix`): `--` comments and `"..."`
strings, which may span lines. An `on <name>` handler is read from `code`, so an
`on` line inside a string or comment is not a handler.

Every line and column is kept, and lifetimes (`'a`) stay code. So a commented-out
arm, in a line or a block comment, is neither an arm nor an answer, and a brace,
arm or variant inside a literal is never read as code. A file that ends inside a
comment or literal fails closed (exit 2).

**Skipped:** `tests/`, `fuzz/`, `benches/` and `target/` directories, `*_test.rs`,
`*_tests.rs`, `*_test.mix`, and the body of an inline `#[cfg(test)] mod x {`
(found by brace balance on the stripped text: scanning resumes after the
module's closing brace). Comment lines are not read.

**Exclusions.** `NOT_DISPATCH` files keep their tables, but their `match` blocks
and guards are not Bus dispatch: the native event matches in `libs/mix`, the
interactive shell's builtins (`shell_handler.rs`) and the `mix stats`
subcommands (`stats_io.rs`). `EVENTS` names (`noded.observe.event`,
`noded.session.lifecycle.gap`) are received events, matched on the receive side,
so a guard with one of those names is a consumer, not a registration.

**Known limit:** the suffixes that `libs/props` `dispatch_props` answers
(`get`, `list`, `describe`) are not enumerated statically. The gate checks the
`noded.props.` prefix arm and that each registered `noded.props.*` name falls
under it.

Exit codes:

| Code | Meaning |
|---|---|
| 0 | Clean: no findings. |
| 1 | Violations. One line per finding, then a summary line. |
| 2 | Could not check. Fails closed: an unreadable or unlistable file or directory, a missing source root, a missing suffix table, no registrations found, an unloadable registry, a `#[cfg(test)]` module that never closes before end of file, a file that ends inside a block comment or string literal, or bad arguments. |

## verb_registry_gate_test.mix

Runs the gate against `fixtures/verb_gate/` and asserts each case's exit code,
violation codes and report text. Usage: `/opt/mixos/bin/mix tools/verb_registry_gate_test.mix`.

- `trees/` holds the mini source trees. Each has `apps/`, `services/`, `libs/`
  and `cli/` roots; `.gitkeep` marks an empty root.
- `registries/` holds the registries that go with them.
- Cases: the clean tree (every registration form, `declared_by`, skipped tests)
  exits 0; the missed dispatch forms, invalid literal, family and registry overlap,
  double registration, owner mismatch, `declared_by` missing, owner conflict,
  retired name, unanswered and unbacked entries, registry shape, a production
  handler after a test module, a `method` match and guard with an unregistered
  literal, a parse arm with no owner variant arm, a comment naming a variant
  before the real construction, the owner's Hello arm commented out with `//` or
  inside a `/* */` block (each a fixture tree), and a parse arm constructing two
  variants all exit 1 with the expected codes (not-answered, or ambiguous-arm);
  a raw string naming a declared-but-unanswered verb in a guard and a prefix arm
  (`raw-guard`), and an `on` line inside a multi-line Mix string (`raw-on`), each
  exit 1 not-answered; the parse arm answered by its owner exits 0, and
  so does a raw string naming a variant; a block comment with a nested brace in a
  test module leaves the later handler read; an unloadable registry, missing
  `--root`, `--registry`, `cli` root, suffix table, empty tree, unreadable
  directory, a test module that never closes, a file ending inside a comment or
  literal, and bad arguments exit 2. The `absent` field asserts text that must
  not appear (a test-only literal, an HTTP token, an ambiguous arm where none is).
- The unreadable case chmods `trees/unreadable/libs/locked` to 000 for the run and
  restores 755 afterwards. As root the case is skipped, since mode bits do not bind.
