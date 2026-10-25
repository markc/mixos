# Relocatable paths

`paths` resolves the five MixOS roots. It has no workspace dependencies.
The running binary finds its prefix through `/proc/self/exe`, following the
managed entry link to the real binary, then taking the parent of `bin/`.
Moving that tree requires no rebuild.

| Root | System default | Prefix under `$HOME` |
|---|---|---|
| `prefix` | Derived from the binary | Derived from the binary |
| `config` | `/etc/mixos` | `$XDG_CONFIG_HOME/mixos`, else `~/.config/mixos` |
| `state` | `/var/lib/mixos` | `$XDG_STATE_HOME/mixos`, else `~/.local/state/mixos` |
| `cache` | `/var/cache/mixos` | `$XDG_CACHE_HOME/mixos`, else `~/.cache/mixos` |
| `runtime` | `/run/mixos` | `$XDG_RUNTIME_DIR/mixos` (required) |

Each root takes `MIXOS_<ROOT>` first, then its key in `<prefix>/layout.mx`,
then the default. The environment prefix also selects where to read
`layout.mx`; a `prefix` key inside that document does not start another lookup.
The document is strict data, with at most five unique keys, each an absolute
path string. Unknown keys, relative paths and executable expressions refuse.
Empty root overrides refuse rather than falling back.

```text
config: "/srv/example/config"
state: "/srv/example/state"
```

Rust callers use `paths::root(paths::Root::Config)`, `paths::bin("mix")`,
`paths::share("scenes/editor")` and `paths::resolve("mixos:prefix/share/scenes")`.
Mix callers use `mixos_path("config")`, `mixos_path("prefix", "bin/mix")` or
`mixos_path("mixos:prefix/share/scenes")`. Relative paths cannot escape their
root with `..`. Ordinary user paths are unchanged by `resolve`.

Saved MixOS paths use `mixos:<root>/<relative>`. Programs resolve them when
opening the file. The old `MIXOS`, `MIXOS_ETC`, `MIXOS_VAR`, `MIXOS_RUN`,
`MIXOS_SHARE`, `MIXOS_BIN` and per-app location aliases have no layout role.
`MIXOS_SRC` separately names a development checkout.

Installers select a target independently of their interpreter. The staged
binary exposes the defaults from `libs/paths` without self-location or
`MIXOS_*` overrides:

```text
/path/to/staged/mix layout defaults --tier system --json
/path/to/staged/mix layout defaults --tier user --prefix /home/user/mixos --json
```

The system installation prefix defaults to `/opt/mixos`; a user installation
requires `--prefix`. User roots use the caller's XDG environment; an unset
runtime is omitted from the JSON and must be supplied explicitly by the
installer. Runtime path resolution still derives the prefix from the binary.

`tools/gold_release.mix` accepts `--prefix`, `--tier`, and overrides for the
other four roots. It verifies the release before querying its staged Mix.
Command-line roots take precedence over the target's existing `layout.mx`,
then the staged binary's defaults. Applying journals the selected `layout.mx`
alongside the installed files. `--state` selects the state root; release
records and backups live beneath its `releases/` directory. Rollback/recover
can use `--receipt-file FILE`, or an explicit prefix and target layout (with
`--state` and `--receipt ID` to locate a saved receipt). They need no running
target binary and work under the previous release or `mix-static`.

`tools/component_install.mix` uses the same precedence. `--root` is a staging
system root, while `--prefix` selects the target prefix within that root.
`--mix STAGED_MIX` supplies defaults before a target binary exists; otherwise
it queries the target's `bin/mix`. Complete explicit roots, target layouts or
rollback receipts need no defaults query. Component receipts retain the
target layout. Neither installer derives the destination from its interpreter.

Repository tools support the previous Mix release. Docs resolve logical paths
against the checkout: `mixos:prefix/share/x` (or `mixos:share/x`) names
`<repo>/share/x`. Populate pinned font inputs in the ignored
`share/assets/sets/` with `mix share/assets/install.mix --root
/absolute/checkout/share/assets` before building the site. Dev lint skips
installed-asset checks when its interpreter lacks `mixos_path`.

External files use templates with `@{prefix}`, `@{config}`, `@{state}`,
`@{cache}` and `@{runtime}` placeholders. Component owners keep their unit
templates; packages carry them beneath `share/layout/`. Apply the layout with:

```text
mix layout apply --dry-run
mix layout apply
mix layout apply --root /tmp/example-root
```

`--root` stages writes beneath a throwaway system root. The renderer prints
each changed file and is idempotent. It manages the PATH block in `.mixrc`,
the login-shell entry in `/etc/shells`, and one link to `<prefix>/bin/mix`:
`/usr/local/bin/mix` on hosts, or `/usr/bin/mix` with `--image`. A non-symlink
at that link path refuses before writes. It never links `mix-static`.
On hosts with Elixir, the managed link makes `mix` mean MixOS.

Remote execution selects the `mix_bin` option, then `MIX_REMOTE_BIN`, then
bare `mix` on the remote PATH. There is no interpreter-owned mesh host table.
Absolute remote binary paths may contain spaces; they are quoted before SSH.

To relocate: stop the affected services, move the prefix, adjust `layout.mx`
for other moved roots, run `mix layout apply`, then restart. Running processes
keep their current paths. The implementation does not migrate live data.
`tools/layout_migrate_saved.mix` previews changes in explicit offline copies,
keeps originals when applied, and refuses live-root overlaps. SQLite stores
must be exported through their owning component first so audit chains survive.

The path gate runs in `tools/lint.mix`. The worker release gate also checks
default template bytes and runs `tests/layout_relocation_test.mix`, using a
prefix with a space and a private mount namespace. Missing namespace, nested
desktop or loopback SSH support is reported explicitly as a skip.

`mix tests/layout_templates_test.mix --installed` additionally compares the
rendered defaults against an existing installed `share/units` tree and reports
any deployment drift, including comments. The normal release check compares
against the immutable source baseline.
