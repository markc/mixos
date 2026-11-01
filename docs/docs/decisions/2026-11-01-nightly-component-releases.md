---
title: Nightly component releases
description: Publish verified CBC gold bytes in independent component release lines.
---

# Nightly component releases

Status: accepted, 2026-11-01. Amends
[weekly CBC releases](2026-10-22-weekly-cbc-releases.md).

The nightly 04:00 gold replaces the weekly whole-OS cut. After installation
and acceptance succeed, publish changed components independently in the one
monorepo. GitHub hosts verified CBC bytes and never builds them. Release
failure must not roll back accepted gold.

Every binary in the release contract and every shipping Mix component gets
tags `<name>-v<declared-version>-<YYYYMMDD>`. Mix uses its language version;
server and desktop bundles use the workspace version. Existing published
tags are immutable. Server is the only line marked GitHub Latest, preserving
full-system downloads for clients using the repository's default release.

Each component archive has one top directory containing its binary or Mix
scripts and its runtime share files. Mix additionally has a bare binary.
SHA256SUMS.txt and release.manifest.mx bind assets to full source commit,
gold job, date and binary version lines. Compare remote digests after upload;
remove the newly created release and fail loudly on mismatch.

Publish only when the component's directory, a transitive Cargo path
dependency, lockfile or version changed. Parse Cargo statically, including
workspace-inherited dependencies. Conservatively include shared packaging,
layout and toolchain inputs. Bundles take the union of their members' inputs.
Notes show the source range and path-limited commit log. First run publishes
all lines. No periodic rebuild is needed to package an unchanged binary.

Use current Arch Linux glibc for now. Record the highest required GLIBC_
symbol version as information, with no floor gate. Cross-builds and older
distribution compatibility are deferred until this release split works.

The release job uses a throwaway checkout at the gold commit and the already
built snapshot, never a mutable development checkout. A gold predating the
packager cannot be published through newer packaging code. Dry-run packaging
and read-only planning precede the operator's first real publication.
