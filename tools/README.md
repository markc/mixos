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

## Scope, threat model and known limits (round 29)

**What the gate is for.** It checks that the code and the Bus registry agree for honest code: every
registered verb is answered by code, every answer is registered, each answer is by its registered
owner, and each name is well formed. It catches drift: a verb renamed in one place only, an answer
added without a registry row, a row whose owner moved, a misspelt literal.

**Threat model.** The adversary is a careless or rushed change, not a determined author. The gate
fails closed: when it meets a construct it cannot resolve, it reports the site as `unreadable`
(exit 1) and never treats it as a pass. It does not expand macros, evaluate `cfg`, or model full
Rust name resolution. Deliberately obfuscated dispatch (a verb name assembled at run time, a
dispatch table built dynamically, generated code) is out of scope. Code review is the backstop for
that, and the gate says so rather than pretending to see it.

**Macros (round 29).** A Rust file whose macro use the gate cannot vouch for resolves no constant
at all, so every bare, `Self::`, `Type::` and `crate::` name in it is unreadable. Only live code is
read: lines inside a `#[cfg(test)]` body are skipped, as const resolution skips them, because a
test module cannot change what production code resolves to. The file is unsafe when its live code
has:

- a `macro_rules!` definition (its name can shadow a trusted macro);
- a `#[macro_use]` attribute;
- a macro invocation at item or statement position whose path is not trusted. A path is trusted
  when its prefix is empty, `std::`, `core::`, `alloc::`, `tracing::` or `log::`, and its name is in
  the reviewed `trusted_macros` list (`tools/verb_gate_exemptions.conf.mix`). Each name has a
  reason. Adding a name is a reviewer decision, made after the real-tree check shows what is
  unreadable;
- a `use` whose first segment is not `std`, `core`, `alloc`, `tracing` or `log`, and which names a
  trusted macro word (`use some_crate::thing as println`). That is always unsafe: the import rebinds
  the name outright;
- a glob `use` (`use crate::names::*;`) with such an untrusted first segment, but only when the live
  code of the same file invokes a trusted-name macro, in any position (expression position too,
  since the name is only looked up). Why: a glob can import a macro with the same name as a prelude
  macro, and a glob shadows the prelude. So a glob could replace the `println!` the file calls. A
  file that invokes no trusted macro has nothing a glob could shadow, so its constants still
  resolve (round-24 behaviour). Test bodies are not read for this rule, so a glob `use super::*;`
  in a `#[cfg(test)]` module never fires it;
- an `extern crate` naming a trusted macro word.

**Known limits, in one place.**

1. Macros are read by name, not expanded. A trusted macro is trusted by its name in the reviewed
   list, not by its expansion.
2. Macro invocations in expression position (`let v = m!(z);`) are not read. A macro can expand to an
   item inside a block expression, and the gate will not see it. Pinned by `rs-macro-expr-position`.
3. `cfg` is not evaluated. Every `cfg` branch is read as if compiled.
4. Name resolution is partial. A constant resolves through the file's own items, its `use` lines,
   `crate::` paths into module files, and the impl block of `Self::` or `Type::`. Anything else is
   unreadable (fail closed), not followed. `super::` and other crates' paths are not followed.
5. Trait defaults are not resolved. `Self::NAME` and `Type::NAME` read impl-level definitions only.
6. An inherent impl of the same type in another file is not read.
7. A glob import (`use x::*`) makes the bare names of its file unreadable unless the file defines
   them locally.
8. `include!`, `include_str!`, `build.rs` output and any generated file are not read.
9. A comparison with a runtime value is reviewed by hand (the exemptions file); the gate cannot key
   it to a verb.
10. The suffixes that `libs/props` `dispatch_props` answers (`get`, `list`, `describe`) are not
    enumerated statically (see the code-check notes above).
11. Over-matching is a cost, not a hole. A `]` or `}` before a name and `!(` can read a non-statement
    as a statement-position invocation, which makes its file unsafe. A trusted word such as `error`,
    `debug`, `info`, `write`, `format` or `log` that appears in a `use` of an untrusted path makes
    its file fail closed. A trusted word followed by `!` anywhere in live code (`log !=` included)
    arms the glob clause, so a file with an untrusted glob and such a word fails closed too. None of
    this reads test bodies. All of it shows as `unreadable`, resolved by a reviewer, not by widening
    the list.
12. Caveats of the glob rule. The trusted-word check is textual: a trusted word followed by `!` in
    live text (a string literal such as `"log !"`, or `log != x`) arms it too. That only ever fails
    closed. Not modelled: a glob that brings in a macro under a trusted name from a module the gate
    cannot see. A macro defined in another file is seen only through its `use` line, which is
    checked like any other `use`.

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
