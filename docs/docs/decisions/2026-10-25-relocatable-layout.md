# Relocatable layout

Status: accepted, 2026-10-25.

1. MixOS code asks for a root rather than naming an installation location.
   This is a clean break, without old-path fallbacks or location aliases.
2. The only roots are `prefix`, `config`, `state`, `cache` and `runtime`.
   `bin`, `lib` and `share` are children of prefix. The default table and
   system/user tier rule are in the [paths manual](../../paths/README.md).
3. Precedence is per root: `MIXOS_<ROOT>`, then `<prefix>/layout.mx`, then
   defaults. Prefix self-location follows `/proc/self/exe` through symlinks.
   A layout document has at most five unique known keys and is strict data.
4. `libs/paths` owns resolution and depends on no workspace crate. Rust uses
   `paths::root`, `bin`, `share` and `resolve`; Mix uses `mixos_path`.
5. Files read by external programs are rendered from `@{root}` templates.
   `mix layout apply` renders them idempotently. Component unit sources stay
   with their owner; installed templates ship under `share/layout/`.
   Moving a tree requires re-rendering and restarting affected processes.
6. Saved MixOS references use `mixos:<root>/<relative>`. Readers resolve the
   reference at use time. Root escapes refuse. User-owned paths stay unchanged.
   Offline migration retains originals; live migration is a separate operation.
7. `tools/path_gate.mix` joins lint. Literal defaults belong only in the
   resolver, external templates, default assertions and documentation.
   Shipped scripts use `#!/usr/bin/env mix` or are invoked with `mix`.
8. Release proof installs to an owned temporary prefix with a space and hides
   the old prefix and config roots in a private mount namespace. It exercises
   installer tests, scenes, a nested desktop and SSH loopback where available.
   Unsupported privileged fixtures report a skip rather than passing silently.
9. One managed link is the fixed entry point: `/usr/local/bin/mix` on hosts,
   `/usr/bin/mix` only inside a MixOS-owned image. Login shells and `/etc/shells`
   use that link. `mix-static` remains unlinked. Host Elixir installations do
   not change the chosen name. Remote precedence is option, environment,
   per-host table when one exists, then bare `mix` on PATH.

This changes no live installation and does not introduce hot relocation,
generations or a versioned store. The default layout stays in place.
