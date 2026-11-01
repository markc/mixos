---
title: Releasing MixOS
description: Nightly component releases from verified CBC gold bytes.
---

# Releasing MixOS

The accepted [nightly release decision](../decisions/2026-11-01-nightly-component-releases.md)
replaces the weekly whole-system cut. The 04:00 gold job builds and gates one
exact source commit on CBC, installs it and accepts the desks. Only then does
the private hub release job package that snapshot. A publishing failure leaves
accepted gold installed. Manual nightly runs accept `--no-release`.

Each binary in `tools/release_packages.mx` and each Mix component with scripts
or units has its own release line. The same contract lists server and desktop
membership: `packages` for binaries, `scripts` for Mix services. Mix services
are also discovered from the component index identity files. Bundles release
when any member's inputs change. First run publishes all lines.

Tags are `<name>-v<version>-<YYYYMMDD>`, titles `<name> <version> (YYYY-MM-DD)`.
The version comes from the package (resolving workspace inheritance), the Mix
component identity, or the language version printed by the gold Mix binary.
Bundles use the workspace version. Existing tags and releases are never replaced.
Only `mixos-server` requests GitHub Latest; all other lines use `--latest=false`.

## Package already-built bytes

Run the packager from a clean throwaway checkout at the gold source commit:

```text
mix tools/package.mix --component term --commit FULL_SHA --bin-dir /path/to/gold/bin --out /tmp/term-assets --gold-job JOB --date YYYY-MM-DD
mix tools/package.mix --components all --commit FULL_SHA --bin-dir /path/to/gold/bin --out /tmp/component-assets
mix tools/package.mix --package server --release VERSION --commit FULL_SHA --bin-dir /path/to/gold/bin --out /tmp/server-assets
```

`--components all` makes one subdirectory per component. Bundle modes remain
`server`, `desktop`, and `all`. Each archive has one top directory with `bin/`
or `lib/<component>/`, plus component runtime data and relocatable unit templates
under `share/`. Mix also ships a bare executable for bootstrap. SHA256SUMS.txt
covers the assets and release.manifest.mx; the manifest records source, job,
date, version, binary version lines and asset hashes. Highest required GLIBC_
symbol versions are informational. Packaging uses the current Arch Linux glibc;
there is no glibc floor refusal. Cross-build compatibility is deferred. The old
pinned libinput bundle is optional when explicitly requested with its source;
normal gold packaging uses the host's libraries.

The hub `nightly_release.mix plan [--gold-job ID] [--commit SHA]` reads release
history without remote writes. `run --dry-run` packages accepted current gold
but stops before creating releases. `run` publishes each changed line at the
exact source SHA, compares every uploaded asset's GitHub digest with local
SHA256SUMS-bound bytes, and deletes a newly created release if verification fails.
The first real publish is performed by the operator. GitHub never builds Rust.

`tools/release_closure.mix` statically parses Cargo manifests, including target
and build dependencies and workspace-inherited path dependencies. Changes in
that closure, Cargo.lock, workspace metadata, toolchain, packaging or layout
inputs cause a release. Notes contain the commit range and path-limited oneline
log. The previous source comes from that line's release manifest. A missing or
invalid previous manifest fails closed.

## Follow a component with releasesd

releasesd supports `tag_prefix` in schema-1 catalogue entries. It scans published,
non-prerelease releases with pagination and chooses the newest publication in
that line. An asset regex alone cannot choose a release line. For example:

```text
{
  schema: 1,
  apps: [
    {name: "term", repo: "markc/mixos", tag_prefix: "term-v", asset: "^term-.*linux-x86_64[.]tar[.]gz$"},
    {name: "mix", repo: "markc/mixos", tag_prefix: "mix-v", asset: "^mix-.*linux-x86_64[.]tar[.]gz$"}
  ]
}
```

Explicit tags also work. Per-user installation verifies checksums and archive
safety; it does not install system gold or restart desks. Default catalogue
entries follow the two bundle prefixes. CI fetches the newest published `mix-v*`
bare executable and verifies its checksum, falling back to the old full-system
release only while no Mix release line exists.