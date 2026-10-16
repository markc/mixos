# tar — archives in-language: tar_list, tar_unpack, tar_pack

Archives are read and written by the language itself, so a verified artifact
can be unpacked without an external `tar` or `zstd`. Inside a MixOS machine
there is no such tool at all: `/bin/sh` is mix. On other hosts the same
builtins avoid depending on a distribution's import tooling. A signed
manifest pins a digest, `hash_file` compares against it, and `tar_unpack`
consumes the SAME verified file. No C codec ships in the binary: zstd is the
pure-Rust RFC 8878 implementation (structured-zstd), and gzip framing is
flate2 over its Rust backend.

One-line help for each builtin is `mix builtins tar_list` (or `tar_unpack`,
`tar_pack`).

## Codecs and the artifact profile

`codec` is explicit on every entry point: `"zstd"` (default), `"gzip"`,
`"none"`. The builtin decoder is the universal reader, so the artifact
profile below is a contract that any encoder must satisfy:

- **single frame** — `tar_unpack`/`tar_list` drain the stream to true EOF:
  the zstd frame checksum / gzip CRC are verified there, any non-zero decoded
  bytes after the tar end are refused, and raw input left unconsumed (a
  concatenated second frame or gzip member, including an empty member) is
  refused. Trailing compressed bytes, including zeros, are refused too;
- **no dictionaries**;
- zstd levels are `1..=22` (default 7). Level 22 is the largest the builtin
  packs, because its 2^27 (128 MiB) window is exactly the decoder's ceiling.
  An artifact produced by an external zstd must not use `--long` beyond that
  window, and must decode with `tar_list` before it is trusted.

For artifacts that people unpack with stock tools, use `codec:"gzip"`
(`tar -xzf` works everywhere). Gzip levels clamp to 9.

## The builtins

| Call | Result |
|---|---|
| `tar_list(path[, opts])` | list of `{name,size,mode,uid,gid,kind,mtime}` — streaming, nothing extracted, stream verified; `kind` is `file`, `dir`, `symlink`, `hardlink`, `metadata` (a PAX global header) or `other` |
| `tar_unpack(path, dest[, opts])` | receipt `{files,dirs,symlinks,hardlinks,metadata,bytes,xattrs_restored,entries,trailing_padding,codec,warnings}` |
| `tar_pack(source, path[, opts])` | receipt `{files,dirs,symlinks,bytes,capabilities,codec,level}` |

`opts` keys are validated strictly — unknown keys raise.

## tar_unpack is safe by default, and staged

- `dest` **must not exist** (a dangling symlink there counts). A bare name
  such as `"out"` means the current directory. Extraction happens in a
  private `0700` staging sibling with an unpredictable name, held by
  descriptor from creation to commit: every member is created relative to
  it, one `O_NOFOLLOW` component at a time, so a staging name swapped in a
  writable parent cannot redirect a write. It is renamed into place only
  after the whole stream verifies, with `RENAME_NOREPLACE`, so anything that
  appears at `dest` in the meantime is never replaced. (On a filesystem
  without `RENAME_NOREPLACE` the commit checks `dest`, then renames; a
  directory created at `dest` between the two is replaced. Unpack into a
  parent other users cannot write.) Failures restore traversable/writable
  staging-directory modes before removing the tree through the same
  descriptor. A cleanup failure is reported alongside the original error,
  with the staging name, rather than silently discarded.
- Member names are validated on the *resolved* path (GNU longname and PAX
  smuggling land there): no absolute paths, no `.`/`..` components, UTF-8
  only (invalid bytes raise — the extractor never guesses what got
  extracted).
- Device, fifo and unknown entry types are **refused**. GNU sparse entries
  are refused by both the lister and extractor. A PAX global header (the
  first record `git archive` writes) is archive metadata: it is counted in
  the receipt's `metadata` and nothing is extracted for it.
- A corrupt or truncated stream (zstd frame checksum, gzip CRC or a cut-off
  stream, including an empty input or truncated zstd header) raises the
  structured code `ARCHIVE_STREAM_CORRUPT`. Match the code
  in the optional second `catch` binding, not the message text.
- Symlink targets must be contained (no absolute, no `..`). Nothing is
  ever extracted *through* a symlink the archive created. Hardlinks may
  only target an earlier **regular file** (hardlink-to-hardlink is
  refused).
- `numeric_owner` (default true) restores uid/gid through the entry's
  descriptor, never following a link. Non-root callers get the capability
  limit silently (the owner is skipped) rather than an error. Any other
  owner failure (an unmapped uid in a user namespace, a root-squashing
  filesystem) is listed in the receipt's `warnings`. When an entry's owner
  was not applied, its setuid/setgid bits and `security.capability` are
  dropped (also listed in `warnings`): a special bit is only ever kept on
  the owner the archive named. Mode and mtime failures raise.
- Modes are restored as the archive gives them, as GNU tar does for root:
  a `0777` directory or `0666` file stays world-writable. Unpack untrusted
  archives into a tree whose modes you then check, or repack them.
- `xattrs` (default true) restores `SCHILY.xattr.*` records by hand, and by
  default ONLY the `user.*` namespace. `security.capability` also needs
  `keep_special_bits:true` (a capability is a privilege grant exactly like
  a suid bit, and suid/sgid are stripped from modes by the same rule).
  Every other namespace, `trusted.*`, POSIX ACLs (`system.posix_acl_*`,
  which can grant any user write access) and the rest of `security.*`
  (SELinux labels, IMA), needs `privileged_xattrs:true`. Restored xattrs are
  applied last, after owner and mode, because chown and data writes clear
  `security.capability`.
- `max_xattr_bytes` (default 16 MiB, at most 1 GiB) bounds the admitted
  xattr names and values held for the whole archive until the post-pass
  applies them; one archive of many small records cannot exhaust memory.
- Directory and symlink metadata (mode, mtime, owner) is applied in a
  children-first post-pass, so directory mtimes stick.
- Limits: `max_entries` (200k), `max_bytes` (64 GiB of file bytes),
  `max_name` (4096), and `max_stream_bytes` (16 GiB). The last bounds the
  **decoded** stream, including headers, metadata, file data and all trailing
  zero padding, across parsing and final verification. An entry's declared
  size must fit the remaining `max_bytes` before its file is created, and
  copying also enforces that remaining budget.
- Each GNU longname, GNU longlink or PAX extension payload is limited to
  **64 KiB**, independently of `max_stream_bytes`, before buffering. GNU
  name/link payloads additionally obey `max_name` plus one terminating NUL.
  Once `max_entries` is reached, the next extension header is refused before
  reading its body. `tar_list` accepts `max_entries`, `max_name` and
  `max_stream_bytes` too.

## tar_pack

Deterministic (sorted, parents-first) walk of a source **directory**:
numeric owner, mtime and mode are preserved, and suid/sgid are stripped
unless `keep_special_bits:true`. Only that same opt-in captures
`security.capability` into a `SCHILY.xattr` PAX record, so capability
round-trips require opting in at both ends (root-only to apply on unpack).
The source directory must not be a symlink. Directory traversal retains
opened directory descriptors; children are opened with `O_NOFOLLOW`, with
metadata and capabilities read from the opened file. Queued directories
remain pinned if their pathnames are replaced. A detected inode replacement
between classification and opening raises. Device and fifo members are
refused. The output file is created new (`0600`) and is never overwritten.
An output inside the source directory is refused before it is created, and
the walk also refuses the output's own inode if it reaches it another way.
A file whose size changes while it is packed raises rather than writing a
header that disagrees with its data. Two packs of an unchanged tree are
byte-identical.

## Round-trip

```
tar_pack("/var/tmp/stage", "/var/tmp/app.tar.zst", {codec:"zstd", level:11})
$receipt = tar_unpack("/var/tmp/app.tar.zst", "/var/tmp/app")
-- receipt.xattrs_restored counts applied records; capabilities only when
-- keep_special_bits was set on BOTH ends.
```
