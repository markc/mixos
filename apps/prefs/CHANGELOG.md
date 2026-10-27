<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
# Changelog

## Unreleased — 2026-10-27

- Delegate Email launching and registration waits to citizen's shared async
  helper. Keep passwords outside effect bodies until sending, preserve
  refusals, and return a failed password to its field. Account status reads
  and best-effort password removal do not launch Email.
