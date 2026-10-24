---
title: webd
description: Virtual hosts, CMS routes, embedded Mix handlers and ACME for the MixOS web service.
---

# webd

The MixOS web service serves per-host static files, CMS SQLite APIs,
embedded trusted Mix HTTP handlers, mail proxy routes and file shares.
It registers as `webd` on the native Bus. The server profile uses
`/opt/mixos/bin/webd serve --db-path /var/lib/mixos/webd/web.db`.

Node configuration is read through config's native directory rules.
`webd.vhost` supplies hostnames, document roots, optional CMS databases,
manual TLS identities or Let's Encrypt HTTP-01 ACME settings. Explicit
listeners assign hosts to interfaces; the legacy default uses the node IP.
Production issuance requires the accepted Subscriber Agreement URL.

The shipped `vhosts/` Mix handlers and `examples/mix-cms/` remain alongside
the service. Configure document roots and credentials explicitly; example
values are documentation, not deployment configuration. Session cookies,
environment variables and data paths use MixOS names.

Build and test with `cargo test --locked -p webd`. `webd --version` emits
MixOS build provenance. Units, sysusers and tmpfiles fragments are in
`services/webd/units/`; they are not installed by building the service.
The deployment identity is user `webd`, UID 502, unit `webd.service`.

State includes CMS and props SQLite/WAL data, session revocations and
sealing keys, ACME accounts/certificates, and uploads. A deployment needs
a single writer, counts/hashes and native HTTP/TLS/CMS acceptance checks,
plus a rehearsed rollback preserving writes. This source transplant
performs no deployment or data migration.

Source: [services/webd](https://github.com/markc/mixos/tree/main/services/webd).

## Source gate limitation

The Rust suite passes, including the service's registry comparison. The
repository-wide command source gate currently treats embedded HTTP template
fragments as standalone programs. It reports undefined prelude functions
and request globals. With those symbols declared to the native linter,
`vhosts/apps/sw.mix:182,193,220,294` and
`vhosts/widgets/treeview.mix:200` still report arity errors: these templates
intentionally rely on nil-padded optional trailing arguments. The source
comments document that contract. The templates and gate are retained;
the source entry gate has not passed. A context-aware lint integration is
required before declaring the complete transplant entry bar satisfied.

No deployment, live ACME issuance or production-data rollback rehearsal
has been performed by this source port.
