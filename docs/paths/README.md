# Relocatable paths

`paths` implements the directory rules from `config` at `32650ae5`. The
process's real uid selects the default tier: uid 0 is system; every other uid
is user. Where the binary is installed does not select the tier.

| Root | Override | Whole-tree `$MIXOS` | User default | System default |
|---|---|---|---|---|
| `prefix` | `MIXOS_PREFIX` | Ignored | Parent of the running binary's `bin/` | Same |
| `share` | `MIXOS_SHARE` (absolute only) | Ignored | `<prefix>/share` | Same |
| `etc` | `MIXOS_ETC` | `$MIXOS/etc` | `$XDG_CONFIG_HOME/mixos`, else `~/.config/mixos` | `/etc/mixos` |
| `var` | `MIXOS_VAR` | `$MIXOS/var` | `$XDG_DATA_HOME/mixos`, else `~/.local/share/mixos` | `/var/lib/mixos` |
| `run` | `MIXOS_RUN` | `$MIXOS/run` | `$XDG_RUNTIME_DIR/mixos`, else `/tmp/mixos-run` | `/run/mixos` |
| `cache` | `MIXOS_CACHE` | `$MIXOS/cache` | `$XDG_CACHE_HOME/mixos`, else `~/.cache/mixos` | `/var/cache/mixos` |

An empty variable is unset. Relative XDG variables are ignored. The original
`etc`, `var` and `run` overrides and `$MIXOS` retain main's acceptance of relative
values; callers that previously required absolute application directories still
enforce that requirement. An absent HOME falls back to `/root`, as in main.

Per-root precedence is its override variable, then `$MIXOS` where applicable,
then `<prefix>/layout.mx`, then the uid default. Prefix itself comes only from
`MIXOS_PREFIX` or `/proc/self/exe`, following symlinks to the real executable.
`layout.mx` is bounded strict data with at most five keys: `etc`, `var`, `run`,
`share`, `cache`. A `prefix` key is refused. Root values in that file must be
absolute paths without `..` or control bytes. Unknown and duplicate keys refuse.
A root in layout.mx applies to every uid, like `$MIXOS`; it is not a system-tier
override. Gold writes only roots differing from the selected tier's defaults
for the target prefix (system unless `--tier user`) and removes layout.mx when
there are no differences. A default system install
therefore retains user XDG directories even with the system prefix.

```text
etc: "/srv/example/etc"
var: "/srv/example/var"
```

Process helpers cache one environment and layout snapshot for the process.
`Environment` plus `Dirs::from_source` provide pure env/uid/executable inputs;
`Dirs::from_environment` additionally reads the layout file. `config::Dir`,
`config::Environment`, `config::Dirs` and `config::path` are thin re-exports.
There is one implementation. [Compatibility fixtures](compatibility.md) spell
out the expected values for both uids at two installation prefixes.

Rust uses `paths::root(paths::Root::Etc)`, `paths::bin("mix")`,
`paths::share("scenes/editor")` and `paths::resolve("mixos:share/scenes")`.
Fallible callers use `try_root`, `try_system` and `resolve`; the infallible
helpers document their startup `expect` contract. `paths::system(Root::Etc)`
selects system defaults regardless of uid, retaining override precedence.
`system_default` explicitly ignores process overrides for existing APIs whose
contract was a fixed system fallback, such as the native bus endpoint.
It also keeps webd's WWW default, blobd's fallback and indexd's database/model
defaults at their existing system paths. In particular indexd's model cache
stays under `var/indexd/.cache`; it does not move to the additive cache root.

```mix
mixos_path("etc")
mixos_path("etc", "node.conf.mix", {tier: "system"})
mixos_path("var", "example", {tier: "user"})
mixos_path("mixos:share/scenes/editor/scene.mix")
```

Logical references use `mixos:<root>/<relative>` with the six names above.
Relative tails cannot escape their root. Ordinary user file paths are unchanged.
Existing per-file overrides such as `MIXOS_NODE_CONFIG` stay in place.

Application state keeps its existing policy. `AppDirs`, Ced, Dopus and screenshot
retain `MIXOS_APP_HOME`, `MIXOS_APPS_HOME`, `MIXOS_VAR/apps`, XDG state and home
state precedence; config/state/cache remain children of that application root.
The macro catalogue keeps its own XDG config directory and `MIXOS_MIX` override.
These policies do not switch to the directory resolver's XDG data default.

External files use `@{prefix}`, `@{share}`, `@{etc}`, `@{var}`, `@{run}` and
`@{cache}` templates. Template sources ship under `share/layout/`; output stays
in its packaged place inside the prefix, including `share/<component>/units/`
and `share/applications/`. The unmanaged flat `share/units/` tree is never changed.
`mix layout apply [--root SYSROOT] [--dry-run] [--image]` renders them
idempotently, prints changed files and manages a single link to
`<prefix>/bin/mix`: `/usr/local/bin/mix` on hosts, `/usr/bin/mix` only in a
MixOS-owned image. On a host it adds the link to `/etc/shells`. It does not
manage mixrc or enable services. `--image` requires `--root`; with `--root`,
no host or HOME files are touched, and only `--image` adds staged `/etc/shells`.
An image root must resolve to a directory other than `/`; aliases to `/` refuse.
An unprivileged apply without `--root` still renders its own prefix. When it
cannot write both host targets it skips the link and shells line with a notice.
A non-symlink at the link refuses. The staging root is canonicalised before
checking its children. Symlink parents below it and symlink render targets
refuse. Every target is checked before the first write. Existing file modes
and owners survive; new files are 0644. Exec and Environment paths are quoted
when needed; whitespace in a path used by a systemd key that does not unquote
(such as EnvironmentFile, ConditionPathExists or LoadCredential) refuses.
Desktop `TryExec` takes the rest of its line as a literal path, so a spaced
prefix renders there unquoted.
`mix-static` is never linked. Host Elixir installs do not rename this entry.

Installers select a target independently of their interpreter. The staged binary
prints defaults from this same Rust implementation without self-location,
layout files or MIXOS overrides:

```text
/path/to/staged/mix layout defaults --tier system --json
/path/to/staged/mix layout defaults --tier user --prefix /home/user/mixos --json
```

System installation defaults use `/opt/mixos`; user installation requires
`--prefix`. Both commands return six roots, including the user run fallback.
`tools/gold_release.mix` and `tools/component_install.mix` accept `--prefix`
and optional `--share`, `--etc`, `--var`, `--run`, `--cache`, `--tier` roots.
Explicit target flags precede its layout file, then the staged binary's
defaults. They do not inherit the installer's ambient layout overrides.
Component `--root` is a staging sysroot; `--mix STAGED_MIX` supplies defaults
before a target binary exists. Gold verifies the release before querying its
binary, renders the owned staged files before hashing, journals any non-default
layout document and saves all six target roots in receipts. Per-package template
manifests coexist, so desktop and server install together. The packaged notifyd
and releasesd unit copies render in `share/<component>/units/`; component_install
continues to own and record the separate lib and systemd copies. They remain
excluded from the global source manifest.
Release records are beneath `var/releases`. Rollback/recovery
can use `--receipt-file FILE` without invoking a target binary.
At the default layout there is no layout.mx. Give `--var` to locate receipts
(or use `--receipt-file`); a non-default layout file can also supply var:

```text
mix tools/gold_release.mix rollback --prefix /opt/mixos --var /var/lib/mixos --receipt ID
mix tools/gold_release.mix recover --prefix /opt/mixos --var /var/lib/mixos
```

Emergency installs of older archives whose binary lacks `layout defaults`
require all six roots explicitly. Gold reports the older archive and persists
all five non-prefix roots rather than guessing defaults. Without all six flags
it refuses. For a current binary, the baseline uses the selected `--tier`.
Gold also journals the host entrance: prior link target or absence and prior
shells hash, with original bytes and metadata in the release backup. Failed
installs, rollback and recovery restore those states, refusing subsequent drift.
Receipt entrance targets are restricted to the link and shells, with a fixed
sysroot marker for isolated tests; receipts cannot introduce arbitrary paths.
Main's earlier absolute root/backup receipts without a layout are also readable,
including release ordering; their shapes are retained during rollback.

Repository tools work under the previous release and `mix-static`. Docs resolve
logical paths against the checkout: `mixos:share/x` names `<repo>/share/x`.
Populate pinned font inputs with `mix share/assets/install.mix --root
/absolute/checkout/share/assets`; these ignored build inputs are excluded from
packages. Lint skips installed-asset checks when `mixos_path` is unavailable.

To relocate, stop affected processes, move the prefix, update non-default roots
in `layout.mx`, render again and restart. Existing processes retain their cached
snapshot. This implementation does not migrate live data.
`tools/layout_migrate_saved.mix --source COPY --from-prefix OLD_PREFIX
--from-layout OLD_LAYOUT` previews offline changes by default; `--apply` keeps
originals. Token boundaries prevent replacing roots embedded inside other paths.
Colon is not a left boundary, so `/usr/bin:/opt/mixos/bin:/bin` stays unchanged;
assignment boundaries using `=` are still accepted.
An interrupted apply can resume when the retained original still equals the
unmigrated file; a different original refuses. Dry-run never removes temporaries.
The old layout supplies the five non-prefix roots. SQLite stores
must first be exported by their owner so revisions and audit chains survive.

The gate rejects literal roots outside resolver defaults, templates, default
assertions and documentation. The release proof uses a prefix with a space and
a private mount namespace hiding the old roots. Installer, scene, nested desk
and SSH loopback tests use disposable roots; unavailable privileges report a
clear skip. Units, desktop entries and runtime relocation all run under the
spaced prefix.
The template proof compares 37 global and 8 component-owned templates
byte-for-byte with main; `--installed` additionally reports drift in existing packaged targets.
It does not inspect or change unmanaged flat unit copies. Templates not included
in a selected release, such as dnsd's EnvironmentFile with a prefix path, can
require a prefix without whitespace. Unit/runtime relocation still uses a prefix
with a space; desktop rendering requires a prefix without whitespace.

Three fixed-FHS Mix defaults remain as exact path-gate deferrals: bridged's
system config fallback, scene-sync's session environment file, and topbar's
identity file. Their original per-file/CLI overrides remain authoritative.
Using a tier lookup there would newly honour MIXOS or layout overrides and
would not preserve main's selection. `tools/path_gate_exceptions.mx` records
each line and its reason. An isolated relocation run must supply their original
overrides if it exercises these paths; these defaults are not relocation proof.
The same registry retains the two literal storage paths in indexd's shipped
example: its loader does not resolve logical strings, and changing their tier
would change the database/model locations. The registry has five exact lines
in four files; it may shrink, and cannot grow silently.
