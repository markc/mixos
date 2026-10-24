---
title: monitord — changelog
description: Release notes of the monitord component, mirrored from services/monitord/CHANGELOG.md.
generated: by docs/_build/gen-site.mix from services/monitord/CHANGELOG.md; do not edit
---

# monitord contract changes

> Mirrored from [`services/monitord/CHANGELOG.md`](https://github.com/markc/mixos/blob/main/services/monitord/CHANGELOG.md), where releases update it. The component's page is [monitord](README.md). This page describes the current `main` branch of MixOS.

## 0.1.0 (draft)

- Read commands `monitord.status`, `monitord.snapshot`, `monitord.processes`,
  `monitord.process`, `monitord.series` and `monitord.history`, all
  queries.
- Retained topic `monitord.summary`, every 5 s.
- History: every sample for 10 minutes in memory; minute rollups for 7 days
  and hour rollups for a year in SQLite (`/var/lib/monitord/history.db`,
  schema 1).
- Command line: `serve`, `snapshot`, `ps`, `history`, and the local actions
  `signal` and `renice`, which run as the caller.
