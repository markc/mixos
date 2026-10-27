<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
# Changelog

## Unreleased — 2026-10-27

- Add Mail selection to File, the row context menu and Ctrl+M, available for
  selections of regular files (symlinks followed). Listing workers supply
  regular-file metadata, keeping availability checks off the filesystem.
- Pass attachment paths to Email over the existing Bus task, launching it
  through citizen when absent. BUSY falls back to adding to the open compose.
  Status and Bus replies report the outcome; headless calls share this path.
