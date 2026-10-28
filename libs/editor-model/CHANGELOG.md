<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
# Changelog

## Unreleased — 2026-10-28

- Match editd's initial scratch dirty state in FakeEditd: pathless opens,
  including empty scratches, remain dirty until saved. Disk-backed opens
  start clean.
