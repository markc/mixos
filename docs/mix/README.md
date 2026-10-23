# Mix — manual

The reference manual for the **Mix** language, one page per topic. The canonical
home of these pages is
[`docs/mix/`](https://github.com/markc/mixos/tree/main/docs/mix) in the public
[markc/mixos](https://github.com/markc/mixos) monorepo; add a page by dropping
`TOPIC.md` there. The same files render everywhere:

- **Terminal** — `mix man TOPIC` (`mix man` alone prints this index). Every
  page is built into the `mix` binary, so it needs no checkout and no network;
  the lookup order and its overrides are in [the mix command](cli.md).
- **Web** — [mixos.dev/mix](https://mixos.dev/mix/overview) serves this
  directory.
- **GitHub** — browse [`docs/mix/`](https://github.com/markc/mixos/tree/main/docs/mix)
  directly; this file doubles as the directory README.
- **Local clone** — plain markdown with relative links; any editor or viewer
  works.

**New to Mix, or coming from bash/Python/JS? Read [gotchas](gotchas.md) first** —
it is the errata against every other language's reflexes, and every row of its
table is executed by `cargo test -p system --test man_gotchas`, so it cannot
rot. `mix man syntax` is the mental model; gotchas is the corrections.

## Start here

- **[overview](overview.md)** — what Mix is, why it exists, where it came from.
- **[invocation & CLI](invocation.md)** — `mix file`, `-c`, `-`, `-i`, login shell, `--serve`, flags.
- **[the mix command](cli.md)** — `mix help`/`man`/`builtins`/`what`/`status`/`trace`/… meta-commands.
- **[syntax & the classifier](syntax.md)** — tokens, the newline rule, shell-vs-Mix dispatch.
- **[gotchas](gotchas.md)** — the guesses from bash/Python/JS that are silently wrong here. Read it first.

## The language

- **[variables, sigils & scope](variables.md)** — `$` sigils, `${...}`, function-local binding.
- **[strings](strings.md)** — `'raw'` vs `"interp"`, `..` concat, codepoint/byte/grapheme ops.
- **[numbers](numbers.md)** — f64, radix literals, ordering, coercion.
- **[operators](operators.md)** — arithmetic, comparison, `and`/`or`/`not`, `..`, `?:`, `??`.
- **[control flow](control-flow.md)** — `if`/`while`/`for`/`loop`, if-as-expression, `break`/`continue`.
- **[functions, lambdas & modules](functions.md)** — `fn`, closures, the pass-in/return/reassign triad.
- **[modules — require/include/source](modules.md)** — `require()` isolated module loading vs the splice loaders.
- **[lists & maps](collections.md)** — literals, indexing, `push`/`pop`, the base footguns.
- **[higher-order functions](hof.md)** — `map`/`filter`/`reduce`/`sort_by`/`group_by`/…
- **[errors & exit handling](errors.md)** — `try`/`catch`/`die`/`panic`, `run_rc().rc`, timeouts.

## Builtins & I/O

- **[math](math.md)** — rounding, powers, logs, trig, `min`/`max`/`clamp`, constants.
- **[files & I/O](io.md)** — `read_file`/`write_file`/`glob`/`stat`/`chmod`/`walk`/…
- **[processes & system](system.md)** — `run`/`run_rc`/`run_stream`/`spawn`/`env`/`exit`/…
- **[job control](job-control.md)** — foreground/background jobs, suspension, resume (interactive).
- **[data & serialization](data.md)** — JSON, TOML, `jq`, `data_encode`, strict-data `.mix`.
- **[byte buffers](buffer.md)** — `buffer`/`buffer_push`/`freeze`, the one reference-semantic type.
- **[regular expressions](regex.md)** — `re_match`/`re_find`/`re_replace`/`re_split`/`grep_lines` (subject first).
- **[dates & time](datetime.md)** — `time`/`date_format`/`now_iso`/`duration_format`.
- **[http](http.md)** — `http_get`/`http_post`/`http_request`, idle and whole-call timeouts, `http_stream` line callbacks.
- **[datastar](datastar.md)** — `ds_*` SSE event framing.
- **[archives](tar.md)** — `tar_list`/`tar_unpack`/`tar_pack`, zstd/gzip/none, safe staged extraction.
- **[SQLite](sqlopen.md)** — `sqlopen`/`sqlexec`/`sqlclose`, read-only by default, `"rwc"` creates.

## Mesh & runtime

- **[Bus messaging](bus.md)** — `send`/`emit`/`address`/`on`, `$result`/`$rc` bands, the mesh.
- **[serving as a citizen](serve.md)** — `mix --serve service.mix`, the supervised runtime.
- **[remote execution (ssh)](remote.md)** — `ssh_run`/`ssh_must`/`ssh_mix`, env transports.
- **[capabilities & embedding](capabilities.md)** — the capability classes + sandbox model.
- **[expression evaluation mode](expr-eval.md)** — `eval_expr_string()`, the single-expression rule, the static deny walk.

## Reference

- **[builtin index](builtins.md)** — every builtin by category.
- **[lint diagnostics](lint.md)** — every MIX-XXXX code and its story.
- **[reserved words](keywords.md)** — the keyword set, keywords-as-names.
- **[shell-dispatch mode](shell-mode.md)** — pipes, `&&`, brace expansion, `$(...)`, redirects.
- **[owned interactive editor](owned-editor.md)** — `MIX_EDITOR=owned` (preview).
- **[usage statistics](stats.md)** — runtime modes, report windows, persistence, kill switch, static coverage.

## Where Mix lives

Mix is the MixOS shell and scripting language. The interpreter lives in the
`mix` library at `libs/mix`; `cli/mix-shell` owns the interactive shell and
native ABP citizen runtime, and `cli/system` builds the `mix` binary.
The source versions are declared in
[`libs/mix/Cargo.toml`](https://github.com/markc/mixos/blob/main/libs/mix/Cargo.toml)
and [`cli/mix-shell/Cargo.toml`](https://github.com/markc/mixos/blob/main/cli/mix-shell/Cargo.toml).
Use `mix --version` to identify the installed binary, which can differ from
the current source.

Use `mix builtins --names` to discover valid builtin names, and `mix builtins
<name>` for their contracts. `mix man overview` and `mix man syntax` explain the
language and command classifier. Discovery should inspect process status rather
than stop a sequence when a lookup is invalid.

Mix scripts use `run_argv` for local processes and native Bus verbs for application
control. `run_argv_must` is appropriate when a non-zero exit must stop the script.
The public language regression corpus accompanies the interpreter transplant.

## Programs and documents

The accepted target for the next major Mix refactor and deployment is two file
families:

| Extension | Meaning |
| --- | --- |
| `.mix` | Executable Mix program: macro, behaviour, service, module, installer or test |
| `.mx` | Declarative Mix document: configuration, scene, theme, catalogue, manifest or data specification |

All program roles use the same language. Documents use strict parsing and an
owning schema where applicable. Only declared binding fields may evaluate the
existing restricted, side-effect-free expression subset. Data strings otherwise
remain literal. A schema is a contract/validator, not necessarily another file.

This is an **accepted target, not a completed runtime migration**. Current
consumers still use `*.conf.mix`, `*.spec.mix`, `*.data.mix`, `scene.mix` and other
legacy data names. Keep their working paths until upgraded readers are deployed.
Do not rename executable helper modules such as a scene library's `data.mix`.

The refactor must refuse `.mx` through executable file loaders and retain strict
document errors without code/shell fallback. Scenes keep their current envelope
and pure bindings initially. Readers upgrade before files and writers; rollback
includes compatible binaries, scripts and documents.

New purposes use existing programs or versioned document schemas. Another file
family or evaluation mode requires an architectural decision. See the
[decision](https://mixos.dev/docs/decisions/2026-10-08-mix-programs-and-documents/) and
[file-family contract](https://mixos.dev/docs/spec/mix/2026-10-08-programs-and-documents/).

## Exit status

Uncaught script errors print a diagnostic and exit 1, including messages with
`interrupted:false` or the word `interrupted`. Signal outcomes come from actual
SIGINT/SIGTERM delivery: non-interactive scripts and `-c` exit 130/143. SIGINT
on an interactive terminal retains exit 0 unless `--result-fd` is used.
A caught interrupt still takes precedence over the evaluation's result;
`--result-fd` emits an error frame rather than a successful value.
Script-defined error codes do not impersonate OS signals; `--serve` retains
its graceful shutdown behaviour.

## Design docs & specs

Public contracts and accepted decisions live under
[`docs/docs/spec`](https://github.com/markc/mixos/tree/main/docs/docs/spec) and
[`docs/docs/decisions`](https://github.com/markc/mixos/tree/main/docs/docs/decisions).
Private deployment records are not part of the public manual. Accepted
decisions can describe staged work; check the current implementation and
installed binary when a feature's availability matters.

## The library

The `mix` component is the language library
([`libs/mix`](https://github.com/markc/mixos/tree/main/libs/mix)): lexer,
parser, evaluator, builtin contracts and embedded handler interfaces.
Features select the optional data, networking and cryptographic
facilities; the interpreter's core stays independent of the Bus and
desktop. It was transplanted from `mix` at mixos
`e0297242305f3a3c3de09f1ca01e8faa771768da`, retaining its regression corpus
and public language version. The shell is [mix-shell](https://mixos.dev/mix-shell/),
and the `mix` binary is built by [system](https://mixos.dev/system/).

## Changes

- **[changelog](changelog.md)**: the Mix library's change history, generated from
  `libs/mix/CHANGELOG.md`.

## See also

```
mix help              the full categorized builtin reference
mix builtins [CAT]    list builtins, optionally by category
mix what NAME         one-line description of a builtin or keyword
mix man TOPIC         read any of the pages above in the terminal
```

For AI agents: [`AGENTS.md`](https://github.com/markc/mixos/blob/main/AGENTS.md) at
the repo root is a short orientation sheet whose canonical reference is this
manual.
