# Relocatable layout

Status: accepted, 2026-10-25; amended the same day to preserve main's uid and
directory rules.

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
4. `libs/paths` owns the implementation without workspace dependencies;
   `config::dir` re-exports the original API. Rust and `mixos_path` share it.
   Process helpers resolve once; pure explicit inputs support tests.
5. External files use `@{prefix}`, `@{share}`, `@{etc}`, `@{var}`, `@{run}`,
   `@{cache}`. `mix layout apply` renders idempotently and prints changes.
   Unit templates remain with their components and ship in `share/layout`.
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
   Missing privileges are reported as skips.
9. The stable host entry is `/usr/local/bin/mix`; `/usr/bin/mix` is managed
   only in a MixOS-owned image. Login shells and `/etc/shells` use that link.
   `mix-static` stays unlinked. Remote lookup is option, environment,
   per-host table when one exists, then `mix` on PATH.

N-1 tooling remains usable before a new binary is installed. Installers get
target defaults by invoking the verified staged binary's `mix layout defaults`
command. They do not select a target from their own interpreter. Rollback can
read the receipt or target layout without running the installed target.

This decision does not deploy or migrate a live installation, move app data,
introduce hot relocation, generations or a versioned store.
