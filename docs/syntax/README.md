---
title: syntax
description: The syntax highlighting library for MixOS text editors, which runs Microsoft Edit's compiled lsh language definitions line by line over any chunked text store.
---

# syntax

Deterministic syntax highlighting for MixOS text editors. It wraps `lsh`
from Microsoft Edit (msedit): a small compiler from `*.lsh` language
definitions to bytecode, and a line-oriented runtime that turns each line
into highlight spans. The two upstream crates, `lsh` and `stdext`, are
vendored with documented patches under
[`vendor/msedit`](https://github.com/markc/mixos/tree/main/vendor/msedit)
(MIT, from microsoft/edit at `826b4c0`).

It is a core library (`kind = "lib"`, `layer = "core"`) with no Bus
contract. Its one direct user is [`editor-model`](../editor-model/README.md)
([`libs/editor-model/src/highlight.rs`](https://github.com/markc/mixos/blob/main/libs/editor-model/src/highlight.rs)),
which maps its highlight kinds to colour classes for the
[editor](../editor/README.md) widget, and so for [ced](../ced/README.md).
Mix source is highlighted by the Mix lexer, not by this crate.

The source is [`libs/syntax`](https://github.com/markc/mixos/tree/main/libs/syntax).

## Concepts

- **Bytes, not graphemes.** lsh works on bytes, so a text store may hand
  over chunks that split a grapheme cluster.
- **Line numbers are 1-based** everywhere in the API.
- **Lines of 32 KiB or more are left plain** (`MAX_LINE_LEN`, as upstream).
  Unlike upstream, the highlighter still advances to the real end of such a
  line, so later line numbers stay correct.
- **A checkpoint every 1024 lines** (`cache::INTERVAL`, pinned in every
  build profile; upstream uses 16 in debug builds). A random seek costs at
  most one interval of parsing.
- **No build script and no mutable statics.** The compiled definitions in
  `src/defs.rs` are generated once and committed.

## API

- `language_for_path(&Path)`, `language_by_id("rust")`,
  `language_by_name("Markdown")` (ASCII-case-insensitive): look up a
  `&'static Language`.
- `LineSource`: the trait a text store implements. `read_forward(offset)`
  returns the contiguous bytes from `offset` to some chunk boundary (empty at
  the end); the highlighter assembles lines that span chunks. The optional
  `indexed_line_end(line)` lets a store with a line index skip over-long
  lines without reading them. `&[u8]` and `str` implement it.
- `highlighter::Highlighter`: parses one line at a time from the top.
  `parse_next_line(&mut Vec<Span>)` fills the spans of the next line;
  `snapshot()` and `restore()` save and resume a `HighlighterState`.
- `highlighter::Span { start, kind }`: `kind` applies from `start` (an
  absolute byte offset in the source) to the next span or the end of the
  line.
- `cache::Cache`: checkpointed random access. `parse_line(h, line, out)`
  resumes from the nearest checkpoint; `invalidate_from(line)` drops the
  states an edit at `line` makes stale; `advance(h, target, max_lines)`
  time-slices a cold seek with a per-call line budget and returns true once
  `target` is reachable; `reach()` is the furthest line reachable without
  parsing. The cache also keeps a frontier, so a budget below one interval
  still makes progress from frame to frame.
- `defs::HighlightKind`: `Comment`, `String`, `KeywordControl`,
  `StorageType`, the `Markup*` kinds and the rest. `defs::LANGUAGES` and
  `defs::FILE_ASSOCIATIONS` are the generated tables. `syntax::runtime`
  re-exports the vendored `Language` and `Highlight` types.

## Languages

C, C++, C#, Diff, F#, Git commit and rebase messages, Go, ignore files,
Java, JavaScript, JSON, LSH, Lua, Markdown, Odin, PowerShell, properties,
Python, Ruby, Rust, shell script, TOML, XML and YAML: one per definition
file in
[`vendor/msedit/lsh/definitions`](https://github.com/markc/mixos/tree/main/vendor/msedit/lsh/definitions).

## Example

From the tests:

```rust
use syntax::highlighter::Highlighter;

let text = "fn main() {\n    // hello\n}\n";
let src = text.as_bytes();
let rust = syntax::language_by_id("rust").expect("lsh defines rust");
let mut h = Highlighter::new(&src, rust);
let mut spans = Vec::new();
h.parse_next_line(&mut spans); // line 1
h.parse_next_line(&mut spans); // line 2: a Comment span starts at the "//"
```

An editor keeps one `Cache` per buffer, builds a fresh `Highlighter` over
its text each frame (a highlighter borrows the text), calls `cache.advance`
with a line budget for lines not yet reached, and calls
`cache.invalidate_from(line)` after each edit.

## Regenerating the definitions

`src/defs.rs` is written by the `gen` example, the same generator msedit
runs from its build script, with two changes: the compiler's IR dump is left
out (it prints node addresses, so it differs on every run) and
`HighlightKind` also derives `Hash`.

```sh
cargo run --release -p syntax --example gen            # writes src/defs.rs
cargo run --release -p syntax --example gen -- --stdout
```

The lsh compiler panics on patterns it does not support, so it runs only
here and in the tests (`syntax::compiler`, hidden from the docs), never
inside an editor. The crate has no features.

## Source layout

| Path | What |
|---|---|
| `src/lib.rs` | Language lookup, `LineSource`, the hidden `compiler` module |
| `src/highlighter.rs` | `Highlighter`, `HighlighterState`, `Span`, `MAX_LINE_LEN` |
| `src/cache.rs` | `Cache` and `INTERVAL` |
| `src/defs.rs` | Generated bytecode, strings, charsets and language tables |
| `examples/gen.rs` | The generator for `src/defs.rs` |

## Tests

- `tests/defs_fresh.rs`: the committed `src/defs.rs` matches a fresh
  generation byte for byte; two generations in one process agree; every
  file association points into `LANGUAGES`, and the lookups work.
- `tests/highlighter.rs`: spans are absolute and ordered; any chunk size
  gives the same spans as one slice; `INTERVAL` stays 1024; over-long lines
  are plain and keep later line numbers, with and without a line index;
  reading past the end is empty; `advance` makes progress below one interval
  per call; and a proptest checks that random seeks, also after an edit and
  `invalidate_from`, equal a linear parse. An ignored benchmark times
  over-long lines with and without a line index.
