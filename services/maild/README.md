# maild

JMAP, SMTP, IMAP, CalDAV and CardDAV mail service, with private authentication,
rule-engine, Bayesian and metadata-store crates. The system binary dispatches
the installed `maild` symlink to this library's `main`; `src/main.rs` is the
development binary.

The server package carries `maild -> mix`, the template unit and sysusers
declaration under `share/maild/units`, and the patched parser's licences under
`share/licenses/maild/mail-parser`. `maild@INSTANCE` requires the matching
`noded@INSTANCE` and `<config>/session/INSTANCE.env` broker configuration.
Its service config is `<config>/maild/config.conf.mix`. State is owned by
`maild` under `/var/lib/maild`, with `MIXOS_STATE` set to that root by the unit.
Configured TLS certificates and DKIM keys must be readable by that account.

See the [manual](../../docs/maild/README.md) and
[storage API](../../libs/mds/README.md). The [test accounting](tests/README.md)
records every ported test and the inherited gates. Run Rust tests on a build
worker; parser nesting tests require the workspace's patched mail-parser.
