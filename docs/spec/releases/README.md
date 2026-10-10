# Releases contract

Status: accepted, version 0.2.0 (0.2.0 adds `exec_wrapper`). Verified by
`services/releasesd/tests/releases_test.mix` (offline logic) and
`services/releasesd/tests/releasesd_test.mix` (every verb over a private
broker).

`releasesd` follows GitHub repositories named in a per-user catalogue and
installs their Linux release tarballs for that user. This page fixes the
catalogue schema, the Bus verbs and the on-disk layout that other tools may
rely on.

## Bus identity

| Item | Value |
|---|---|
| Service | `releases` (`mix --serve releasesd.mix --name releases`) |
| Verbs | `releases.status`, `.list`, `.check`, `.notes`, `.install`, `.update`, `.rollback`, `.remove`, `.link` |
| Bodies | JSON objects; `{}` or an empty body where no field is required |
| Success | rc 0, JSON reply |
| Refusals | rc 10 bad request, rc 14 unknown app, rc 15 the operation failed; body `{error_code, message}` |

Error codes: `USAGE`, `ROOT`, `UNKNOWN_APP`, `CATALOGUE`, `NO_RELEASE`, `NO_ASSET`,
`AMBIGUOUS_ASSET`, `DOWNLOAD`, `DIGEST`, `UNVERIFIABLE`, `LAYOUT`,
`NO_PREVIOUS`, `PENDING`, `BUSY`.

| Verb | Body | Reply |
|---|---|---|
| `releases.status` | — | `{catalogue, apps, installed, root, share, bin, state, authenticated, arch}` |
| `releases.list` | `{apps?}` | rows from the last check |
| `releases.check` | `{apps?}` | rows after asking GitHub |
| `releases.notes` | `{app}` | `{app, tag, published, url, notes}` |
| `releases.install` | `{app, tag?}` | result |
| `releases.update` | `{apps?}` | results; default every installed app |
| `releases.rollback` | `{app}` | result |
| `releases.remove` | `{app}` | result |
| `releases.link` | `{apps?}` | results |

A row is `{app, repo, installed, latest, published, checked, status}`, with
`status` one of `current`, `update`, `not installed`, `unchecked` or
`error: <text>`. A result is `{app, action, version?, previous?, verified?,
links?, skipped?, error?}`; `verified` is `{asset, sha256, proofs, tag}`.

## Catalogue, schema 1

A strict-data document, by default `$XDG_CONFIG_HOME/mixos/releases.mx`
(`~/.config/mixos/releases.mx`). A missing file is an empty catalogue. It is
re-read on every request.

```text
{
  schema: 1,
  apps: [
    {name: "example", repo: "owner/example"},
    {name: "other", repo: "owner/other", asset: "(?i)linux-x86_64-full\\.tar\\.gz$"}
  ]
}
```

| Field | Rule |
|---|---|
| `name` | `^[a-z0-9][a-z0-9-]{0,63}$`, unique; it becomes a directory name |
| `repo` | `owner/name` on GitHub |
| `asset` | optional regex choosing the tarball when the default is wrong or ambiguous |
| `exec_wrapper` | optional list of 1–32 non-empty strings without control characters, the first an absolute path without `=`; see below |

**`exec_wrapper`.** When `link` writes the app's desktop entries, every
`Exec=` becomes `Exec=<wrapper…> -- <the entry's own Exec>`. The entry's own
Exec keeps its field codes (`%F`, `%U` …) and is made absolute as usual.
Each wrapper argument is written per the Desktop Entry Specification: `%` is
doubled; an argument with a reserved character is double-quoted, with
`"` `` ` `` `$` `\` escaped inside the quotes; then every backslash is
escaped once more as a string value. The literal `--` separates the wrapper
from the app's command, so a wrapper can always fall back to running that
command as it stands. Every `Exec` is wrapped, Desktop Actions included,
with whitespace before the key and around `=` allowed. The main group and
every Desktop Action must have a non-empty `Exec`, or the entry is refused
(`LAYOUT`). An install, update or rollback checks this before the version
becomes current, so a refusal changes nothing. If a wrapper is added while
an install or rollback is interrupted and the target cannot be wrapped, the
retry abandons that change: `current` returns to the other recorded version
(if that one passes), `previous` is cleared, and `meta.json` records
`abandoned: {op, target, reason}`. A rollback that does this reports it as
its result. A wrapped entry gets `DBusActivatable=false`, since D-Bus
activation would start the app without its `Exec`. `TryExec` stays the
app's real program. The entry is
rebuilt from the app's own copy on every link, so it is never wrapped twice.
The link manifest records the final content. Removing the field unwraps the
entry on the next link.

The default asset is the one release file matching `linux[-_.]<arch>.tar.gz`
(`x86_64`, `amd64` or `x64`; `aarch64` or `arm64`), excluding web builds.

## Verification

An install proceeds only if every checksum source the release publishes
matches, and at least one exists: the asset's API `digest`, and a
`SHA256SUMS`/`checksums` file (itself matched to its own API digest) that
lists the tarball exactly once. Anything else refuses with `DIGEST` or
`UNVERIFIABLE`, and nothing changes on disk.

## Trust rules

- **Never as root.** Every operation that changes files refuses with `ROOT`
  when run as uid 0, before taking any lock or creating any file. Installs
  are per user, by that user.
- **The release is checked before use.** It is unpacked into a stage without
  owners, special permissions, ACLs, extended attributes or file flags, then
  refused (`LAYOUT`) if any symlink does not lead, step by step and through
  no other symlink, to an existing file or directory inside the release; if
  it holds a special file; or if any file is setuid or setgid. Desktop entries are read
  only when they are regular files inside the release.
- **Downloads are bounded.** The tarball may not exceed its published size
  (2 GiB when none is given), a checksum file 1 MiB, and each file must
  arrive within 30 minutes.
- **A manifest is evidence, not permission.** Each entry is `{path,
  states}`, every state a link target or a sha256. A path is replaced or
  removed only if it is inside this target and still in one of its states;
  anything else is left and reported (`skipped`, `kept`). Before changing
  anything, a relink publishes an intent manifest listing each path's old and
  new state, so an interrupted relink is recognised and finished.
- **Only managed versions are touched.** A version directory is managed when
  it carries `.verified-<version>.json`. Pruning and removal touch nothing
  else; an app directory holding anything unmanaged is kept. `current` and
  names ending in `.json` are never versions.
- **A retry finishes the job.** Before `current` moves, `meta.json` records
  the new version and previous one plus `pending: {op, target}`, cleared once
  the links are done. A later install, rollback or link first resumes a
  pending transition exactly: a resumed rollback is the rollback asked for; a
  resumed install is finished before a new rollback starts from it. Several
  link targets may share one opt directory (a session seeing it read-only):
  `pending` records its originating target by a random id kept in the
  target's own `<share>/mixos/releases/.target-id` (never by path: two mount
  namespaces can show different directories at one path). Only that target
  resumes and clears it, another target's `link` only reconciles its own links, and its
  install, rollback or remove refuses with `PENDING` until the origin has
  finished.
  Installing the version that is already current reconciles its links.
- **Serialised, in one order.** Locks are always taken installation first
  (`<root>/opt/.releases.lock`, shared by every target using that opt
  directory, whatever its state directory), then target
  (`<share>/mixos/releases/.lock`). Install, update, rollback and remove hold
  both. A link holds its target and, when it can open it, the installation;
  through a read-only opt it cannot, and it then only reconciles its own links
  and never resumes a pending transition. The service also handles requests in order:
  while an install downloads, other requests wait, so callers allow minutes
  for `install` and `update`.

## Layout

Root defaults to `~/.local`.

| Path | What |
|---|---|
| `<root>/opt/<app>/<version>/` | the release prefix: `bin/`, `share/` |
| `<root>/opt/<app>/current` | symlink to the active version |
| `<root>/opt/<app>/meta.json` | `{name, repo, version, previous, verified, installed_at, pending?, abandoned?}` |
| `<root>/opt/<app>/.verified-<version>.json` | the proof of each kept version |
| `<share>/mixos/releases/<app>.links.json` | what was made in this target: `[{path, states: [{link} or {sha256}]}]` |
| `$XDG_STATE_HOME/mixos/releases/<app>.json` | the last check: ETag, latest release |

The current and previous versions are kept; older ones are pruned. Links:
`bin/*` into `<root>/bin`; desktop entries into `<share>/applications` with
`Exec`/`TryExec` made absolute through `current`; hicolor icons and MIME
packages as symlinks. A path that is not in the target's manifest is never
replaced or removed.

## Environment

`MIXOS_RELEASES_ROOT`, `MIXOS_RELEASES_VIEW` (the root as the apps see it,
for installs into a container), `MIXOS_RELEASES_SHARE`, `MIXOS_RELEASES_BIN`,
`MIXOS_RELEASES_CATALOGUE`, `MIXOS_RELEASES_STATE`, `MIXOS_RELEASES_API`
(default `https://api.github.com`) and `GITHUB_TOKEN` (else `gh auth token`).
The token is sent only to `https://api.github.com`.
