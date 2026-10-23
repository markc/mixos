---
title: Weekly CBC releases, starting with Linux x86_64
description: MixOS release builds run on designated CBC workers at an exact source commit; GitHub hosts the verified result.
---

# Weekly CBC releases, starting with Linux x86_64

Status: accepted, 2026-10-22. The first public release still requires its
own green gate and acceptance record.

MixOS starts with Linux x86_64 binaries built against Ubuntu 24.04's glibc
2.39. Each later architecture or operating system needs its own build,
package and install proof before it appears in a release.

All Rust build, test and Clippy work runs on the designated CBC workers at
one full source commit. The release build uses a signed Ubuntu 24.04 build
root on a CBC worker. The packager runs every shipped binary there, records
the source commit and hashes in `release.manifest.mx`, and refuses a higher
glibc floor. Those exact bytes move to a draft GitHub Release; GitHub does
not rebuild them. A published release is immutable.

The full gate is a distinct commit status,
`mixos/weekly-x86_64-full`. It runs for a frozen weekly candidate after the
first release. Normal pushes do not launch a full GitHub-hosted Rust build.
A failed gate cannot create a publishable candidate, and the last published
version remains current. Any cheaper source checks must use their own status
name and cannot stand in for the full gate.

The weekly candidate is cut on **Friday** (amended 2026-10-23). The weekend
is for review, and Sunday night is the window for an emergency rebuild, so
the working week starts on a known-good release. Changes merged between
releases are not release candidates and do not run the full gate.

Every push runs the **lint tier** instead (`tools/lint.mix`, workflow
`lint`, amended 2026-10-23). It runs `rustfmt --check`, a syntax or data
check of every Mix file, and the repository's own gates that need no
compile: closure, layering, verb registry, webd, units, style, component
index and site. The site check needs the pinned font assets and subsetting
tools, so a bare runner skips it; the pre-commit hook runs it on every
commit. The tier compiles no Rust
and runs no tests. Its runner uses the `mix` binary from the latest
published release, verified against that release's `SHA256SUMS.txt`.

`mixos-desktop` and `mixos-server` have separate package membership and
acceptance. A desktop candidate may pass while a server service still needs
admission. The release notes state exactly which archives are accepted.

The canonical system installation is `/opt/mixos`. A privileged Mix gold
installer verifies the published manifest and archives, records a receipt,
and supports rollback. It does not activate running sessions. Prefs and
releasesd currently offer verified **per-user** application installs; their
MixOS entries do not update `/opt/mixos`. A system update control in Prefs
must use a reviewed native privilege and activation path before it can claim
to update the system.

Implementation and operator steps: [Releasing MixOS](../dev/releasing.md).
