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

Your catalogue is `~/.config/mixos/releases.mx`. MixOS ships it empty:

```text
{
  schema: 1,
  apps: [
    {name: "example", repo: "owner/example"}
  ]
}
```

`name` is how you refer to the app and the directory it installs into;
`repo` is the GitHub `owner/name`. If a release has several Linux tarballs,
add `asset:` with a regular expression that picks the right one. The full
schema is in the [releases contract](spec/releases/README.md).

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

(`releases.mix` lives with the service in `/opt/mixos/lib/releasesd/`.)

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

## On the Bus

The session runs the service as `releases`:

```sh
send releases releases.check
send releases releases.install body='{"app": "example"}' timeout=600
```

The verbs, bodies and replies are in the [releases contract](spec/releases/README.md).
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
