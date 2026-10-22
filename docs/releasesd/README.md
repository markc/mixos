---
title: Application releases
description: How releasesd follows upstream application releases on GitHub and installs their Linux builds per user, verified, with rollback, from the command line or the Bus.
---

# Application releases

Many good Linux applications ship as a GitHub release: a tarball per
platform, a checksum file, release notes. `releasesd` makes those first-class
on MixOS. You list the repositories you follow; it tells you when a new
release is out, installs it for you (never system-wide), checks it against
the checksums the release publishes, and keeps the previous version so you
can step back.

It works the same from the command line, from the Bus (so agents and the
settings panel can drive it), and on any Linux, not only MixOS.

## The catalogue

Your catalogue is `~/.config/mixos/releases.mx`. Until you write one,
releasesd follows MixOS itself: `mixos-desktop` (the compositor and the
apps) and `mixos-server` (`mix` and the headless services). This shows the
latest published version and permits a **per-user** install. It does not
replace the system's `/opt/mixos` tree or activate a running desk. Your own
catalogue replaces that list, so keep those entries if you want to follow
MixOS releases:

```text
{
  schema: 1,
  apps: [
    {name: "mixos-desktop", repo: "markc/mixos"},
    {name: "mixos-server", repo: "markc/mixos"},
    {name: "example", repo: "owner/example"}
  ]
}
```

`name` is how you refer to the app and the directory it installs into;
`repo` is the GitHub `owner/name`. If a release has several Linux tarballs,
add `asset:` with a regular expression that picks the right one. To start an
app through a launcher of your own, give `exec_wrapper:` its command as a
list, for example `["/usr/local/bin/launch", "example"]`. The app's menu
entry then runs `launch example -- <the app's own command>`. The full schema
is in the [releases contract](../docs/spec/releases/README.md).

## Everyday use

```sh
mix releases.mix check            # ask GitHub what's new
mix releases.mix                  # installed vs latest, no network
mix releases.mix install example  # latest release; or: install example v1.2.0
mix releases.mix notes example    # what changed
mix releases.mix update           # every installed app that has a newer release
mix releases.mix rollback example # back to the previous version
mix releases.mix remove example
```

(`releases.mix` lives with the service in `/opt/mixos/lib/releasesd/`;
`mix tools/component_install.mix releasesd` puts it there from a checkout.)

`check` uses conditional requests, so asking again costs almost nothing. It
uses `GITHUB_TOKEN`, or your `gh` login if you have one; without either,
GitHub allows 60 requests an hour.

## What an install does

1. Picks the release's `linux-<arch>.tar.gz` (not the AppImage, Flatpak,
   deb or rpm: the tarball needs nothing from the system and unpacks as a
   ready prefix).
2. Verifies it. Every checksum the release publishes must match: GitHub's
   own digest of the file, and the release's `SHA256SUMS` file if there is
   one. If the release publishes neither, or anything differs, nothing is
   installed.
3. Unpacks it to `~/.local/opt/<name>/<version>` and points
   `~/.local/opt/<name>/current` at it.
4. Links its programs into `~/.local/bin`, and its desktop entry, icons and
   file types into `~/.local/share`, so it appears in the launcher and opens
   its files. The entry's command is rewritten to run the installed binary
   through `current`, so an update or rollback needs no relinking.
5. Keeps the previous version for `rollback`; older ones are removed.

It never overwrites a file it didn't create. If something of yours is
already at `~/.local/bin/example`, the install leaves it alone and says so.

## MixOS system updates

An operator updates the canonical `/opt/mixos` tree from the published
`release.manifest.mx` and matching archives with `tools/gold_release.mix`.
The installer checks the trusted manifest digest, archive and binary hashes,
architecture and glibc floor. `plan` shows the changes; `apply` records a
receipt and backups; `rollback --receipt ID` restores the previous files.
It needs one deliberate privileged invocation and does not restart a desk.
Shared desks see the host tree through their mount; isolated desks need a
verified copy followed by their own restart. Prefs does not yet drive this
system update path.

## On the Bus

The session runs the service as `releases`:

```sh
send releases releases.check
send releases releases.install body='{"app": "example"}' timeout=600
```

The verbs, bodies and replies are in the [releases contract](../docs/spec/releases/README.md).
After any change it asks the application registry to rescan, so the
launcher updates by itself.

## Sharing one install with a container

An install can be linked into a second place, such as a MixOS session in a
container that sees your `~/.local/opt` through a read-only bind mount at
the same path. `releases.mix link` writes only the desktop entries, icons and
file types into that session's data directory (`MIXOS_RELEASES_SHARE`), with
its own record of what it made. Nothing is downloaded twice, and an update on
the host reaches the container at once. When the container sees the files at
another path, `MIXOS_RELEASES_VIEW` says where.

## Source and tests

`releasesd` is a Mix component
([decision](../docs/decisions/2026-10-10-mix-components.md)): no Cargo
package, identity in `component.mx`. The source is
[`services/releasesd`](https://github.com/markc/mixos/tree/main/services/releasesd):

| Path | What |
|---|---|
| `scripts/lib/releases.mix` | all of the logic, as a module |
| `scripts/releasesd.mix` | the Bus service (`mix --serve … --name releases`) |
| `scripts/releases.mix` | the command line, same logic in-process |
| `units/releasesd@.service` | the session unit |
| `tests/releases_test.mix` | offline: validation, verification, install, rollback, links |
| `tests/releasesd_test.mix` | every verb over a private broker and a stand-in API |

Run the tests from the checkout root. The offline test needs `bsdtar` for
two hostile fixtures (the service itself unpacks with Mix's `tar_unpack`); the Bus test
needs a `noded`, by default `/opt/mixos/bin/noded`, or `--noded <path>`:

```sh
mix services/releasesd/tests/releases_test.mix
mix services/releasesd/tests/releasesd_test.mix
```

The verbs are registered in `docs/docs/spec/bus/verbs.conf.mix`.
