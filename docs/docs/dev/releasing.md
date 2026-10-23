---
title: Releasing MixOS
description: Exact-source CBC gates, x86_64 glibc packaging, GitHub drafts, and verified gold installation.
---

# Releasing MixOS

The first release target is Linux x86_64 with a glibc 2.39 floor. The
release contract is in `tools/release_packages.mx`. The initial desktop
candidate contains one archive, `release.manifest.mx`, and `SHA256SUMS.txt`.
The desktop archive includes the ten desktop products, indexd, the desktop
settings authority settingsd, and the Mix runtime used by its session scripts.
An admitted server archive can join a later candidate. Every archive in a
candidate comes from one exact public source commit. Packaging an unadmitted
service does not admit it for publication.

## Build and gate

Use a designated CBC worker and freeze a full 40-character source commit.
`tools/release_gate.mix check --commit SHA` runs format, workspace build,
Clippy, functional tests, the release-mode editd latency test, Term's
native-session end-to-end tests against the freshly built Mix, source gates
and Mix tests. Its `LOG` lines identify the saved stage logs. The
`mixos/weekly-x86_64-source` commit status goes green only after that
exact-source gate passes. The separate `mixos/weekly-x86_64-artifacts` status
goes green only after packaging, disposable gold preflight, draft upload
and GitHub asset digest comparison also pass at the same commit. There
is no Rust build on a GitHub push. A push runs only the lint tier: run
`mix tools/lint.mix` locally before pushing to see the same result.
Candidates are cut weekly on Friday.

For packaging, prepare a signed Ubuntu 24.04 root with glibc 2.39, the
pinned Rust toolchain from `rust-toolchain.toml`, Cargo's cache, and the
development libraries used by the workspace: `libxkbcommon-dev`,
`libwayland-dev`, `libx11-dev`, `libxrandr-dev`, `libxi-dev`,
`libxcursor-dev`, `libgl1-mesa-dev`, `libegl-dev`, `libgles-dev`,
`libgbm-dev`, `libdrm-dev`, `libinput-dev`, `libudev-dev`,
`libseat-dev`, `libpixman-1-dev`, `libpulse-dev`,
`libpipewire-0.3-dev` and `libspa-0.2-dev`. It also needs `git`,
`build-essential`, `pkg-config`, `clang`, `libarchive-tools`, `meson`,
`ninja-build` and `patchelf`.
Ubuntu 24.04 supplies libinput 1.25, while compd's tablet dial support
uses the 1.26 ABI. Build libinput 1.26.2 from upstream tag `1.26.2`
(peeled commit `3402f380019dc4f882e1d8627029607aca3f4be6`) inside the
same root with Meson, install it under `/usr`, and run `ldconfig` there.
Keep that pinned source checkout inside the target directory. The gate
refuses another libinput source or library version. The desktop archive
bundles the 1.26.2 shared library and its licence; compd's relative
runtime library path makes that copy take precedence over the host copy.
After installing the listed build dependencies in the root, the CBC worker
can provision the pinned library with:

```text
/opt/mixos/bin/mix tools/libinput_compat.mix \
  --rootfs /srv/ubuntu-24.04-root --target-dir /srv/mixos-target
```

The script verifies the upstream commit and installed ABI. It is safe to
run again after an Ubuntu root update.
On the CBC worker, the native Mix gate takes absolute
paths for the root, Cargo and Rustup homes, and a dedicated target
directory. It bind-mounts the exact checkout, toolchain, cache, target,
`/proc` and `/dev/null` into the root, checking each mount's identity.
It refuses a root that does not report glibc 2.39 or does not see the
requested commit.

```text
mix tools/release_gate.mix package --commit FULL_SHA --version V \
  --package desktop \
  --rootfs /srv/ubuntu-24.04-root --target-dir /srv/mixos-target \
  --cargo-home /home/user/.cargo --rustup-home /home/user/.rustup \
  --libinput-source /srv/mixos-target/libinput-1.26.2-src \
  --out /srv/mixos-target/release-V
```

The package stage builds each selected Cargo package once in that root.
The packager then runs each binary's `--version` inside the same root,
checks the source hash and bundled libinput resolution, and writes the
archives and manifest. The
`--fixture-binaries` option is for tests only; the gold installer refuses
fixture manifests. Check the complete saved log and the archive hashes
before upload. Never substitute a build made against a newer host glibc.

## Stage and publish

Transfer the three desktop output files from CBC with a hash comparison on both
sides. Upload them to a new **draft** GitHub Release at the frozen commit,
then compare GitHub's asset digests with the CBC files. Do not replace an
existing published tag or its assets. Publishing requires a full green
status, archive checks, clean install and rollback proof, and acceptance
for every included archive. The release notes identify any archive that
is still pending. Enable immutable releases before public publication.

After the initial release, cut one candidate each week. Failure holds the
previous release; it does not trigger repeated rebuilds on every push.

## Install the system gold tree

The operator supplies the manifest hash from the verified GitHub asset
digest. `plan` makes no gold changes. `apply` needs one explicit privileged
invocation, and prints a receipt ID for `rollback`. The installer refuses
a missing or changed archive, wrong source or architecture, a glibc floor
above the host, and unsafe archive paths.

```text
sudo /opt/mixos/bin/mix tools/gold_release.mix plan \
  --manifest /path/to/release.manifest.mx --manifest-sha256 TRUSTED_SHA256 \
  --archive-dir /path/to/assets --package desktop

sudo /opt/mixos/bin/mix tools/gold_release.mix apply \
  --manifest /path/to/release.manifest.mx --manifest-sha256 TRUSTED_SHA256 \
  --archive-dir /path/to/assets --package desktop
```

Install and activate are separate operations. A shared desk sees the host
gold tree through its mount. An isolated desk needs a hash-checked copy
of that tree and its own restart. Test the isolated desk first, then the
shared desk. The public installer does not assume particular console
numbers. Prefs currently tracks and installs MixOS releases per user; it
does not update or restart a system desk.
