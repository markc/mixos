# Instance verb families (schema 3)

Status: accepted, 2026-10-13.

Amended 2026-10-24 by [Bus commands and app actions](2026-10-24-bus-commands-and-actions.md): the names below are now command, command class, command family and action.

## Context

Some Bus verbs are not one name. A bridge serves one service per instance it
runs, so a verb such as `<instance>.status` exists once for each running
instance, and the instances are known only at runtime. The verb registry
(`docs/docs/spec/bus/verbs.conf.mix`, checked by `tools/verb_registry.mix`) listed
only concrete names, so there was no place to record these verbs or to check
that a name could not be taken by an instance.

## Decision

1. **Schema 3 is additive.** The `verbs` list is unchanged from schema 2, and
   its entries stay concrete in this migration. A new `families` list follows
   it. A family does not stand in for a concrete verb, in the registry or in
   code. This decision does not settle whether a concrete verb may later be
   expressed as a family; that would take its own decision.

2. **Schema 2 takes no families.** A schema 2 file with a `families` key is
   refused, empty or not, so an unvalidated list cannot pass as legacy data.
   Schema 1 stays refused. Schema 3 requires a `families` list, even an empty
   one.

3. **Family fields** are exactly `pattern`, `owner`, `members`, `version`,
   `status`, `class`, in that order, one entry per line, written
   `pattern:"…"` (never `name:"…"`). The fields are validated like the verb
   fields: release versions, the statuses and classes below, and component
   names for the owner.

4. **Pattern.** It is `<app>.<suffix>`, where the literal `<app>` stands for
   the instance name and the suffix is one or more lower identifier segments
   (`call`, `ui.tree`). Catch-all patterns are refused, and each pattern is
   unique across all owners.

5. **Members.** `members` is either `"instances"`, meaning the members are
   runtime instances and are not listed, or a nonempty list of unique Bus
   service names.
   - Families of one owner may share a listed member, so one service can
     implement several families (for example `<app>.call` and `<app>.status`).
   - A listed member used by families of two owners is refused.
   - A listed member may equal the first segment of concrete verbs only when
     every such verb has the family's owner, and the expanded wire name
     (`member.suffix`) is not itself a concrete verb. An exact concrete
     duplicate is always refused.
   - Any other collision is refused, not resolved.

6. **Class semantics** are those of
   `docs/docs/decisions/2026-10-11-verb-classes.md`. A family records its class as
   written; the gate checks that it is one of the four classes and does not
   infer it from the handler. The `bridged` families are classed:
   - `<app>.call` and `<app>.execute` are `intent`.
   - `<app>.commands`, `<app>.inspect` and `<app>.status` are `query`.
   - `<app>.screenshot` is `intent`. A capture alone would be a query (a query
     may write the artefact it returns), but its `focus` option can bring the
     window forward, which another caller sees. A family has one class, so the
     verb is intent whether or not `focus` is set.

7. **Family ownership.** A family with `"instances"` belongs to its owner
   component only. The source check accepts an `on @.<suffix>` declaration
   only from the component whose `"instances"` family has that suffix. Every
   such family must be declared at least once in its owner's runtime code.
   Another component declaring the same suffix is refused as a wrong owner.

8. **Declarations come from the native parser.** The source check reads the
   handler metadata of the native `mix lint --json` (lint schema 2). There is
   no fallback reader.

## Checks and their limits

- **Source check (`--source ROOT`).** The gate reads the `.mix` files under
  `apps/`, `services/`, `cli/` and `libs/`. It skips `tests/`, `test/`,
  `benches/`, `fuzz/`, and `*.conf.mix`/`*.spec.mix` files. The declarations
  come from the native parser: each file is passed to `mix lint --json FILE`,
  and every handler whose command starts with `@.` is a declaration. The
  handler metadata is the parsed AST, so nested and duplicate declarations
  count, and text inside quotes, heredocs and comments does not. The source
  is never executed.
  - **Feature detection.** The lint document is schema 2, with a `handlers`
    list. A Mix that answers without it fails with `native Mix lacks parsed
    handler metadata; rebuild current Mix`. There is no fallback parser.
  - **Fail-closed.** A file that does not lint (nonzero exit), output that is
    truncated, a document that is not valid JSON, a wrong `tool`, `files` or
    `summary`, or a handler with a bad `command`, `file` or `line` each make the
    check fail. An unreadable source root or nested source directory fails the
    check too. Diagnostics and source text are never printed.
  - **Walk.** The gate lists each directory with the native `ls` and reads each
    entry with `stat`, recursing by hand. The native `glob` and `walk` skip an
    unreadable directory without an error, so they are not used to prove an
    absence. Test, bench and fuzz directories are skipped before the recursion.
    The source root and each kind directory must be a plain directory (a
    symlink is refused). Trailing slashes are dropped before that check, so a
    symlink named with one is refused too. Only the last component of each such
    path is checked without following links; a symlink in a directory above it
    is followed, as any path is. A symlink anywhere else in a kind's tree is
    refused, and so is a directory that cannot be listed.
  - The check is **structural**. It proves that a handler for the suffix sits
    in the owner's code, not that it registers at runtime. The owner's Bus
    suite tests the runtime registration.
- **Instance check (`--instances DIR`).** Each `*.conf.mix` file stem in DIR
  is checked against the Bus name rule, the reserved names (the `mixos-`
  prefix, `noded` and `noded-*`, the broker's session-name shape), the first
  segments of concrete verbs and the listed family members. A name that is a
  concrete prefix or a listed member is refused whatever its owner. DIR must be
  a plain directory (a trailing slash does not get a symlink past this), and an
  unreadable DIR fails the check. Any path may hold glob characters, since `ls`
  takes a name, not a pattern. Configs are flat by design: the gate lists the
  top level only, dot files included. A `*.conf.mix` entry in a subdirectory,
  and any other file, is ignored; none of them is a runtime config, and the gate
  neither recurses into them nor reads their contents. Symlinks, directories and other non-regular files named
  `*.conf.mix` are refused, and so are configs when the registry has no
  instances family. Config contents and tokens are never read.
- **Offline.** Neither check touches the network or a running broker. A name
  that passes here is a name the Bus will accept, as far as the Bus's own
  rules go. A runtime refusal (for example, a broker that has already
  registered the name) is still possible and is not this gate's concern.
- **Mesh exposure is unchanged.** Nothing in the gate or the registry changes
  what any mesh peer can reach. The families describe names; a service's own
  configuration decides what it serves.

## Consequences

- Schema 2 files stay valid as long as they carry no families.
- The source check needs a Mix whose `mix lint --json` reports handler
  metadata (lint schema 2). An older Mix fails the source check until it is
  rebuilt; the gate does not fall back to another reader.
- A new instance family needs a family entry, and an `on @.<suffix>` handler
  in its owner's code, in the same change.
- Public examples use only the neutral fixture names `fakeapp`, `demod` and
  `otherd`. No runtime instance name belongs in this repository's docs.
