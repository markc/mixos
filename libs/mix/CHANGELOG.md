# Changelog

## Unreleased

- Add the `archive` feature with the `tar_list`, `tar_unpack` and `tar_pack`
  builtins. Archives use zstd (default), gzip or no codec. `tar_unpack`
  validates every member, extracts into a staging directory and renames it
  into place only after the stream verifies. See `mix man tar`.

## 0.109.1

Initial MixOS transplant of the current language library and regression corpus.
Language syntax, builtin contracts and evaluator behaviour retain the source
version. Environment and project roots use MixOS names. SSH helpers retain the
installed Mix path used by existing Cosmix nodes during the migration.
