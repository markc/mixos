# Changelog

## Unreleased

- Harden archive builtins: count trailing padding against the shared decoded
  stream budget, reject oversized declared files before creation and bound
  copying, cap GNU/PAX metadata payloads before buffering, and refuse unread
  gzip input or additional members. Packing captures capabilities only with
  `keep_special_bits:true` and walks pinned directory/file descriptors without
  following replacement symlinks. Rollback restores directory access and
  reports cleanup failures. Empty/truncated zstd headers now raise
  `ARCHIVE_STREAM_CORRUPT`. GNU sparse entries are refused; `tar_list` also
  accepts `max_entries`. Each fix has targeted archive regression coverage.
- Add the `archive` feature with the `tar_list`, `tar_unpack` and `tar_pack`
  builtins. Archives use zstd (default), gzip or no codec. `tar_unpack`
  validates every member, extracts into a staging directory and renames it
  into place only after the stream verifies. See `mix man tar`.
- Fix `password_hash` sha512-crypt output, which glibc and Dovecot could not
  verify. The salt is now 16 chars, and default rounds use the implicit
  `$6$salt$hash` form. `password_verify` raises on a `$6$` salt longer than 16
  chars instead of answering true.

## 0.109.1

Initial MixOS transplant of the current language library and regression corpus.
Language syntax, builtin contracts and evaluator behaviour retain the source
version. Environment and project roots use MixOS names. SSH helpers retain the
installed Mix path used by existing Cosmix nodes during the migration.
