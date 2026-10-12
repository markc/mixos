# Usage statistics

`mix stats` records how Mix is used and reports either dynamic usage or static
authorship coverage. Recording is best-effort: a missing, read-only, corrupt,
or busy state store never changes program output or exit status.

## Collection modes

| Invocation | Mode | Script label |
|---|---|---|
| interactive REPL | `interactive` | none |
| `mix file.mix` | `script` | basename only |
| `mix -c CODE` | `c` | none |
| `mix -` | `stdin` | none |
| `mix --serve file.mix` | `serve` | basename only |

Calls in functions, callbacks, handlers, `source`, and `include` retain the
top-level mode and script label. Paths are never stored. Basenames are truncated
to 96 Unicode characters; each weekly document keeps at most 128 distinct
labels and merges overflow into `(other)`.

The standard prelude and `~/.mixrc` load before collection is attached, so
bootstrap code does not pollute the invoking program's figures. Remote ssh
execution needs no special mode: the far-end process records its normal
`script`, `c`, or `stdin` invocation.

Enabled collection is not free. Exact statement/function counts make recursive
functions use the canonical evaluator path instead of the native sync-recursion
fast path, and tracked calls perform builtin membership lookup. This can be
measurable in tight recursive or call-heavy workloads; use `MIX_STATS=off` when
that cost matters.

## Disable collection

Set `MIX_STATS=off`; `0` and `false` are synonyms, ignoring ASCII case and
surrounding whitespace. This is the documented setting for remote and mesh
servers:

```text
MIX_STATS=off mix -c 'print("unrecorded")'
```

The setting is read once per process. When disabled, Mix allocates no collector
and opens no state file, database, or lock. Every runtime tracking hook first
checks the same cached boolean and returns before borrowing collector state,
formatting errors, or scanning builtin names. `mix stats`, including `coverage`,
prints a disabled message without reading its target.

## Storage and concurrency

State lives under `$XDG_STATE_HOME/mix`, falling back to
`$HOME/.local/state/mix`. One-shot runs accumulate counters in memory and flush
once after evaluation, including after `exit(code)` and ordinary uncaught
errors. Long-lived processes retain daily UTC buckets so a run crossing
midnight or an ISO-week boundary is split truthfully.

The store is JSONL. A finished run appends one record, its counters by day
plus its run record, to `runs/YYYY-MM-DD.jsonl`, named for the UTC day it was
written. That is all a run does with the store: one `write(2)` on an
`O_APPEND` file, with no lock, no read, no listing and no retry. Each record
starts and ends with a newline, so a record a crash or a short write cut short
cannot swallow the next one; readers skip blank and unparseable lines, and a
complete record that lost only its final newline still counts once. A short or
failed write drops that run's counters. Raw files are history: nothing ever
rewrites, truncates or deletes them. A file's name says only where a record
was written: its buckets and runs carry their own dates, and reports filter
by those.

Every record carries a random `batch` id, and readers keep one copy of each
batch per day file (the dedupe does not reach across files). A REPL
`mix stats` pipeline flushes its live counters mid-session and may retry: the
retry repeats the same record into the same file. It hands the counters back
to the session only when every attempt is known to have written nothing; if an
attempt may have landed (an I/O error after the write), they are dropped
rather than risk being counted twice. A short write is never retried. A
`--serve` generation retiring at RELOAD is finished, so it is recorded like a
run's exit, once.

`cache/YYYY-MM-DD.json` holds an advisory aggregate per day, with the dates
its records touch, written only by `mix stats` commands, only for days before
today, and only when the command gets `cache.lock` (a busy lock just skips
it; runs never touch it). Each entry names the raw file it summarises by
device, inode, length, mtime and ctime, all taken from the descriptor that
was read, and is fully validated when read. If the raw file no longer matches,
because a late run, a clock rolled back or anything else changed it, the
entry is ignored and the day is read from the raw file. Deleting `cache/`
loses nothing.

`clear` and `reset` append an operation to `ops.jsonl` instead of rewriting
anything, and apply only to the JSONL store. An operation is anchored to
storage positions, not to a clock: it records the length of every raw file
when it was issued and applies to exactly the records whose JSON ends within
those lengths; anything stored later, in any file, is after it. Operations
take `ops.lock` (runs never do), so two at once take turns, and one refuses if
another holds it for two seconds. Repeated operations on the same target
(clears of one name, resets of one week) collapse into one when read, which
bounds how many a report applies; every report still reads all of
`ops.jsonl`.

Weekly documents from the previous store (`current.json`, `YYYY-WNN.json`) are
read in place, with that store's own rules, including the fingerprints that
stop an interrupted rotation from counting a week twice. They are never moved,
rewritten or quarantined, and their writers' lock, `stats.lock`, is never
taken: a binary from before the JSONL store may still be running and writing
them, and it drops a write if that lock stays busy. Since it writes only those
files while new binaries write only JSONL, the totals add up exactly during a
mixed rollout. The documents are read as a validated snapshot: if one changes
while it is read (an older binary rotating or rewriting it), the read is
repeated, and if it never settles the report says the legacy store was busy.
`clear` and `reset` refuse, recording nothing, while any of these documents
exists: an older binary may still be writing them, so they cannot be cleared
exactly. They age out of week windows. The check runs when the operation
starts and again once it holds `ops.lock`; it is a point-in-time check, not
proof that no older binary will write one later. `mix.db` held only what the documents
carried and is ignored.

Reports stream: they read each day line by line, use the cache where it still
matches (and skip a day whose cached dates miss the window), and keep run
records only for reports that list them (`sessions`, `raw`, `query`). Run IDs
include 128 bits of random entropy. `mix stats` commands that get the lock
also remove atomic-write temporary files more than an hour old. Details of
skipped records are printed only when `MIX_DEBUG` is set; nothing in
collection ever changes a run's output or exit status.

The store is required to be on a responsive local filesystem: appends rely on
local `O_APPEND` semantics (a single write lands whole or is short, never
interleaved), and a stalled NFS/FUSE mount can stall a flush.

In script, `-c`, stdin, and serve modes, SIGINT and SIGTERM take the normal
flush path. The interactive REPL stays alive on Ctrl-C and later flushes on EOF,
`exit`, or ordinary shutdown, but an OS SIGTERM uses the terminal process's
default termination and may lose its in-memory batch. `SIGKILL`, a panic, or
power loss can also lose the in-memory batch; stats are intentionally
best-effort.

If legacy history remains under `$MIXOS_SRC/_stats`, report commands print a
migration or acknowledgement command. History is never moved automatically.

## Reports and windows

Every usage report prints its window in the first line. The default window is
the current ISO week, and `overview` and `never` use the exact same snapshot.

```text
mix stats                 current-week overview
mix stats builtins       current-week builtin calls
mix stats functions      current-week user-function calls
mix stats aliases        current-week alias expansions
mix stats commands       current-week external commands
mix stats keywords       current-week canonical keyword executions
mix stats meta           current-week meta commands
mix stats errors         current-week errors
mix stats sessions       current-week completed runs
mix stats never          current-week builtins and keywords with zero use
mix stats modes          current-week per-mode breakdown
mix stats scripts [N]    top scripts; N defaults to 10 and is bounded to 1–100
mix stats raw            current-week JSON snapshot
mix stats week YYYY-WNN  one ISO week
mix stats since DATE     buckets on or after YYYY-MM-DD
mix stats all            everything recorded
mix stats trend NAME     last 30 recorded days
mix stats query SQL      all-time SQL over an in-memory database
```

At the interactive REPL, a bare `mix stats …` line runs in-process and reports
this session's live counters. Since 0.61.1 a piped or redirected line
(`mix stats never | wc -l`, `mix stats never >> log`) runs as a real external
pipeline instead — live stats are flushed to disk first, so the child reads
current data including this session. Before 0.61.1 the plumbing was silently
dropped. See `mix man shell-mode` § "`mix` meta-commands under plumbing".

`clear NAME` removes a name from every counter category of everything stored
before it. `reset` removes the current week's buckets and runs stored before
it; older days stay, and a live REPL retains older pending buckets. Both are
appended operations, anchored to storage positions and applied at read time;
neither rewrites stored data, and runs recorded later keep their counts. Both
refuse while the old weekly documents exist (see Storage and concurrency). Week
and date arguments must use canonical `YYYY-WNN` and `YYYY-MM-DD` forms. `query`
loads everything recorded into an in-memory SQLite database with the tables the
old on-disk `mix.db` had (`usage`, `usage_context`, `runs`, `sessions`,
`stats_meta`), so existing queries keep working.

The canonical tracked keyword vocabulary is:

```text
if for while loop function return select print eprint parse die
try catch finally export alias break continue send address emit on
source include sh
```

Structural delimiters such as `then`, `end`, `done`, and `each` are not usage
events.

## Static authorship coverage

```text
mix stats coverage DIR
```

Coverage follows the root path explicitly named by the user, then recursively
scans sorted `.mix` files without following symlinks encountered below that
root. It tokenises and parses with Mix's real lexer and parser, then walks the
AST. Builtin call sites inside strings, heredocs, or comments cannot be mistaken
for code. Nested calls, higher-order builtins, and builtin method syntax are
counted. Calls through a function-valued expression are reported as “dynamic
calls not classifiable” and are never guessed.

The command reports used and never-authored builtins and canonical keywords.
If any selected file is unreadable or fails to lex or parse, diagnostics are
printed and the command exits non-zero without emitting a misleading coverage
report. Coverage performs no persistence writes.
