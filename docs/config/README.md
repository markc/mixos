---
title: config
description: Where MixOS keeps its files and how a conf.mix file is read, with one directory rule for every component and descriptor-safe atomic file helpers.
---

# config

Where MixOS keeps its files, and how a `*.conf.mix` file is read.

- `config::path(Dir::Etc | Var | Run | Share)`: one rule for every
  directory. A `MIXOS_ETC` / `MIXOS_VAR` / `MIXOS_RUN` / `MIXOS_SHARE`
  override wins; else `$MIXOS/etc`, `/var`, `/run` when a root is set; else
  the XDG directory for a user (`~/.config/mixos`, `~/.local/share/mixos`,
  `$XDG_RUNTIME_DIR/mixos`) or the system directory for root (`/etc/mixos`,
  `/var/lib/mixos`, `/run/mixos`). `Share` is installed read-only data at
  `/opt/mixos/share` whatever the root or user. Resolved once per process
  and cached; `Dirs::resolve(&Environment)` applies the rule to explicit
  inputs for tests.
- `config::parse` / `config::parse_file` and `Value`, `Map`, `Error`,
  `ErrorKind`: the [strict](../strict/README.md) crate's strict-data
  loader, re-exported so a config reader needs one dependency. Typed
  loading is `strict::from_file`.

No dependency on the Mix language. Test with `cargo test -p config`.

## Node configuration and TLS

`node::NodeConfig` reads `node.conf.mix`, selected by `MIXOS_NODE_CONFIG`
or the native `Dir::Etc` search paths. The optional `maild` section contains
typed ports, hostname, storage paths, spam settings, the legacy `tls_cert` /
`tls_key` pair and `tls: { identity: [...], strict_sni: false }`. Mail state
defaults to `path(Dir::Var)/maild/{mail.db,blobs,mds}`. Unknown service and
mail settings survive typed round trips.

Optional `jmap_listen`, `jmap_upstream`, `smtp_inbound_listen` and
`smtps_listen` fields override the corresponding address helpers. Otherwise
the helpers use the node IP and mail ports (8443, 25 and 465 by default).
`jmap_upstream()` uses HTTPS. Missing mail settings remain valid.

`node::{TlsConfig,TlsIdentityConfig}` are always available. Each identity
requires `server_name`, `cert` and `key`; its independent `default` and
`no_sni_fallback` flags default to false. Enable the optional `tls` feature
for `load_tls_config(cert_path, key_path)` and
`tls::sni::{SniCertResolver,TlsIdentity,pick_identity}`. The resolver loads
PEM pairs and matches names case-insensitively. Unknown SNI uses the default;
absent SNI prefers the no-SNI identity then the default. If neither flag is
set anywhere, the first identity fills both roles. Strict SNI rejects unknown
names and clients which omit SNI. `default_identity()` remains available for
pre-TLS greetings, including in strict mode.

`from_conf_mix_str` and `load_conf_mix_path` return `config::Error` (the
strict loader error); node search/load functions retain `anyhow::Result`.
Verify TLS with `cargo test -p config --features tls`.

The source is [`libs/config`](https://github.com/markc/mixos/tree/main/libs/config).

## Application directories

`AppDirs` shares the existing Ced/Dopus per-application root, config, state
and cache rules. First absolute root wins: `MIXOS_APP_HOME`, then
`MIXOS_APPS_HOME/<component>`, `MIXOS_VAR/apps/<component>` (or
`MIXOS/var/apps/<component>`), `XDG_STATE_HOME/mixos/apps/<component>`,
then `HOME/.local/state/mixos/apps/<component>`. An invalid component or
absent absolute root returns `None`. Resolution performs no I/O. App-owned
filenames stay in their owners; this promotion preserves existing paths and
lets other native settings consumers use the same isolated cache policy.
`resolve_with` injects inputs for tests without mutating process
environment.

## Atomic directories

`atomic::open_directory` refuses symlinks in every component.
`atomic::create_directory` uses the same held-descriptor walk and provisions
missing components with mode 0700. It validates the entire absolute path
before creation, refuses parent traversal and leaves existing permissions
unchanged.

## Web configuration and ACME policy

`node::WebdConfig` adds the web listener, vhost, share roots, auxiliary
DB and ACME settings to `NodeConfig`. `web_listen()` derives the legacy
listen address; `synthesize_listeners()` validates explicit binds and
host ownership, retaining the fail-soft disabled-host path. Unknown web
settings survive typed node round trips.

`resolve_webd_acme()` checks per-vhost issuance choices, HTTP-01 readiness
and the node-level Let's Encrypt production Subscriber Agreement. Only
the policy resolver can mint an `AcmeTosAcceptance` proof.

Node loading, directory resolution and TLS/SNI use the existing config
implementations. Client discovery and connect helpers already live in
`bus::client_helpers`; config remains independent of the Bus client.
