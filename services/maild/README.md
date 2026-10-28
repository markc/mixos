# maild

JMAP, SMTP, IMAP, CalDAV and CardDAV mail service, with private authentication,
rule-engine and Bayesian crates, using the shared metadata store. Maild ships
as its own binary; `src/main.rs` calls the library entry point.

The server package carries the `maild` binary, service/socket units and sysusers
declaration under `share/maild/units`, and the patched parser's licences under
`share/licenses/maild/mail-parser`. `maild@INSTANCE` requires the matching
`noded@INSTANCE` and `<etc>/session/INSTANCE.env` broker configuration.
Its service config is `<etc>/maild/config.conf.mix`. State is owned by
`maild` under `/var/lib/maild`, with `MIXOS_VAR` set to that root by the unit.
Configured TLS certificates and DKIM keys must be readable by that account.

See the [manual](../../docs/maild/README.md) and
[storage API](../../libs/mds/README.md). The [test accounting](tests/README.md)
records every ported test and the inherited gates. Run Rust tests on a build
worker; parser nesting tests require the workspace's patched mail-parser.
