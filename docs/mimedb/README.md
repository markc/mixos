---
title: mimedb
description: A library that reads the freedesktop shared-mime-info database and decides a file's type from its name and content the way GLib does, so MixOS names a file's type as the desktop does.
---

# mimedb

`mimedb` reads the freedesktop shared-mime-info database from the same
files the desktop reads, and decides a file's MIME type by GLib's rules. The
answer agrees with `gio info` and the file manager. It is a plain Rust
library with no Bus commands, and its only dependency is `libc`.

Two components use it:

- [Mix](../mix/README.md), through the `mime_type()` builtin (behind the
  `mix` crate's `mime` feature; see [Mix I/O](../mix/io.md)).
- [thumbs](../thumbs/README.md), which types a file before choosing a
  thumbnailer for it.

The source is
[`libs/mimedb`](https://github.com/markc/mixos/tree/main/libs/mimedb).

## The database

Each `mime` directory under `$XDG_DATA_HOME` (default `~/.local/share`) and
`$XDG_DATA_DIRS` (default `/usr/local/share:/usr/share`) can hold `globs2`
(or the older `globs`), `magic`, `aliases` and `subclasses`. Directories are
read highest precedence first, and relative entries are ignored, as the XDG
spec requires. A higher directory's `__NOGLOBS__` and `__NOMAGIC__` entries
clear a lower one's rules for that type. Missing directories and files are
skipped. A file over 16 MiB (`MAX_FILE`) is skipped, and so is anything that
is not a regular file once opened: a FIFO in a database directory is never
waited on.

## How a type is decided

1. A non-regular file is its `inode/*` type (`inode/directory`,
   `inode/fifo` and so on), and an empty file is `application/x-zerosize`.
2. If exactly one glob matches the name, that is the answer.
3. Otherwise the leading bytes are sniffed: the highest-priority magic
   match, or `text/plain` when the content looks like text.
4. With no glob match, the sniffed type is the answer, or
   `application/octet-stream` (uncertain) if nothing was sniffed. With
   several glob matches, a sniffed type of priority 80 or more wins; failing
   that, the first glob type that is a subclass of the sniffed type wins;
   failing that, the first glob type wins, marked uncertain.

Globs are matched in GLib's order: a literal name, then the longest
`*.suffix`, then full patterns. At most `SNIFF_MAX` (16384) bytes are read,
GLib's limit, and fewer when the magic rules reach less far.

One known limit: full patterns use the crate's own `fnmatch`, not glibc's.
POSIX named classes use Unicode's predicates instead of a locale's tables,
and collating symbols cannot end a range. The stock database uses neither.

## API

- `Database::load()` loads the desktop's database (`xdg_mime_dirs()`);
  `Database::load_from(&dirs)` loads given `mime` directories, highest
  precedence first.
- `for_path(path, content) -> PathGuess` types a file, following symlinks.
  With `content` false the file is never opened. A missing path is typed by
  its name (`exists: false`); an unreadable one by its name, uncertain.
- `guess(name, data) -> Guess` decides from an optional name and optional
  leading bytes, for a caller that already has the file open.
- `Guess { mime, by, uncertain }`, where `by` is a `By`: `Stat`, `Glob`,
  `Magic`, `Content` (an empty file or the text check) or `Fallback`.
  `By::as_str()` gives the lower-case name.
- `globs(name)`, `sniff(data)`, `unalias(mime)`, `is_subclass(mime, base)`
  and `parents(mime)`. Parents come nearest first and include the spec's
  implicit ones: `text/plain` for `text/*` types and
  `application/octet-stream` for everything but `inode/*`. Cycles in
  `subclasses` are visited once.
- `is_stale()` reports whether any directory or database file has changed
  (timestamp or size) since loading, so a long-lived caller can reload.
  `sniff_len()` is how many bytes to read before calling `guess`.
- `text_fallback(data)` and `looks_like_text(data)` are the two text checks.

## Example

thumbs reads the start of a file it already has open and asks for its type:

```rust
let db = mimedb::Database::load();
let mut data = Vec::new();
source.take(db.sniff_len() as u64).read_to_end(&mut data)?;
let mime = db.guess(Some("photo.jpg"), Some(&data)).mime;
```

From Mix, the same answer comes from `mime_type()`, which keeps one
database per thread and reloads it when `is_stale()` says so or the XDG
directories change:

```mix
$r = mime_type("/etc/hosts")
print($r.mime)
```

## Source and tests

- `src/lib.rs`: the database, loading and layering, the decision rules,
  and the subclass graph.
- `src/glob.rs`: parsing `globs2`/`globs` and the desktop's match order,
  including `fnmatch`.
- `src/magic.rs`: parsing and matching the binary `magic` file.

Unit tests in each module build private `mime` directories in a temporary
directory, so they never depend on the host's `/usr/share/mime`. They cover
layer clearing, staleness after a same-size rewrite, oversized files,
priority-zero magic, cycles and very wide parent graphs. The `mix` crate's
[`tests/mime_type.rs`](https://github.com/markc/mixos/blob/main/libs/mix/tests/mime_type.rs)
tests the builtin against a private database the same way.
