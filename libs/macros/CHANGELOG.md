<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
# Changelog

## Unreleased — 2026-10-23

- Return parent-directory creation errors promptly instead of retrying them
  as macro filename clashes; retain exclusive creation and symlink refusal.
- Make runner teardown tests independent of an installed Mix executable.

- Create user app macros from titles, using ASCII slugs and numbered clash
  suffixes through the existing exclusive file-creation path.
