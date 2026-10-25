# Relocatable layout

Status: accepted, 2026-10-25; amended the same day to preserve main's uid,
directory and installation rules.

1. MixOS asks for installation roots rather than embedding locations.
   Existing environment names and per-caller directory policies are retained.
2. Canonical roots are `prefix`, `share`, `etc`, `var`, `run`, `cache`.
   `etc`, `var`, `run` and `share` preserve the rules of
   `libs/config/src/dir.rs` at `32650ae5`: `MIXOS_ETC`, `MIXOS_VAR`,
   `MIXOS_RUN`, `MIXOS_SHARE` and whole-tree `MIXOS`; real uid 0 means system,
   other uids mean user/XDG. Relative XDG values are ignored. Var uses XDG
   data, and run keeps the `/tmp/mixos-run` fallback. Only the installed prefix
   and default share location change to self-location. Cache is additive.
   Caller-owned app state and per-file overrides keep their own rules.
3. Per-root precedence is its override, then `$MIXOS` where applicable,
   then `<prefix>/layout.mx`, then uid defaults. Share ignores `$MIXOS` and
   accepts only an absolute override. Prefix comes from `MIXOS_PREFIX`, else
   canonical `/proc/self/exe` and the parent of `bin/`; it never selects tier.
   The bounded strict-data layout has at most `etc`, `var`, `run`, `share`,
   `cache`, and refuses a prefix key. The [paths manual](../../paths/README.md)
   gives the complete table and explicit system/user APIs.
   A root stored in layout.mx applies to every uid, like `$MIXOS`. Installers
   persist only differences from the selected tier's defaults for the target
   prefix (system by default); the default system installation has no layout.mx
   and preserves each user's XDG roots.
4. `libs/paths` owns the implementation without workspace dependencies;
   `config::dir` re-exports the original API. Rust and `mixos_path` share it.
   Process helpers resolve once; pure explicit inputs support tests.
5. External files use `@{prefix}`, `@{share}`, `@{etc}`, `@{var}`, `@{run}`,
   `@{cache}`. `mix layout apply` renders idempotently and prints changes.
   Templates render to the existing packaged places inside the prefix:
   `share/<component>/units/`, `share/applications/` and other shipped paths.
   Template sources remain with their components and ship in `share/layout`.
   Gold renders while staging, before hashing and recording receipt bytes.
   Enabling services remains the operator's step. Layout apply never installs
   systemd, sysusers, tmpfiles or polkit files outside the prefix and never
   touches unmanaged flat `share/units/` copies. Mix component units remain
   owned and rendered by component_install, outside the global manifest.
   Their existing packaged share/ unit copies render through the package's own
   manifest; this does not claim or overwrite component_install's systemd units.
   Only the ruling 9 link and, on a host, its line in `/etc/shells` are managed
   outside the prefix. There is no mixrc management. With `--root`, `/etc/shells`
   is touched only inside that root and only with `--image`, which requires
   `--root` resolving to a directory other than `/`. A non-root host apply
   renders its prefix and skips host integration with a notice when it cannot
   write both host targets. The root is canonicalised; symlinks below it and symlink render
   targets refuse. All targets are checked for writability before writing;
   existing modes and owners are preserved, and new rendered files are 0644.
   Paths containing whitespace refuse in systemd keys that do not unquote
   (including EnvironmentFile, ConditionPathExists and LoadCredential).
   Desktop `TryExec` takes the rest of its line as a literal path, so a
   spaced prefix needs no quoting there.
6. Saved MixOS references use `mixos:<root>/<relative>`. Root escapes refuse;
   user-owned paths retain their meaning. Offline migration keeps originals;
   live migration is a separate operator action.
7. The path gate joins lint. Literal defaults are limited to the resolver,
   external templates, default assertions and documentation. Shipped scripts
   use `#!/usr/bin/env mix` or are invoked with Mix.
   The behaviour-preservation review explicitly permits deferring a literal
   where a replacement cannot be proved equivalent. Three fixed-FHS Mix
   fallbacks and two storage-path lines in indexd's example are recorded by
   exact file, line and reason in
   `tools/path_gate_exceptions.mx`; stale entries or additional occurrences
   fail the gate. They are not a general path fallback or an environment alias.
8. Main's directory tests move with unchanged assertions. A table compares
   each root under public-safe desktop, system-unit and bare-login environments
   for uid 0 and a user at both default and moved prefixes. No default-layout
   user data moves are accepted. The relocation proof uses a disposable prefix
   with a space and hides the old roots in a private mount namespace; it runs
   installer, scene, nested desktop and SSH loopback checks where available.
   Units, desktop entries and runtime relocation all run under that spaced
   prefix. Missing privileges are reported as skips.
9. The stable host entry is `/usr/local/bin/mix`; `/usr/bin/mix` is managed
   only in a MixOS-owned image. Login shells and `/etc/shells` use that link.
   `mix-static` stays unlinked. Remote lookup is option, environment,
   per-host table when one exists, then `mix` on PATH.

N-1 tooling remains usable before a new binary is installed. Installers get
target defaults by invoking the verified staged binary's `mix layout defaults`
command. They do not select a target from their own interpreter. Rollback can
read the receipt or target layout without running the installed target.
Main receipts with an absolute target and backup and no layout are read in
their original shape; the absent layout means the default layout. Their
recorded prefix and var backup path suffice for offline rollback. They also
participate in release ordering alongside logical receipts.
The default layout has no layout.mx: offline rollback/recovery names `--var`
to locate receipts or supplies `--receipt-file`. A layout file that names var
can locate its receipts as well. Older archives without `layout defaults` can
be installed with all six roots explicit, persisting all five non-prefix roots;
otherwise they refuse with an archive-specific message. Gold journals the host
entrance as well as prefix files, backs up prior shells bytes and metadata and
the old link, and restores them on failed install, rollback and recovery.

This decision does not deploy or migrate a live installation, move app data,
introduce hot relocation, generations or a versioned store.
