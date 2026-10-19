---
title: daemon
description: Shared HTTP host parsing, listener guards, TLS and ACME provisioning.
---

# daemon

Protocol-independent daemon infrastructure. `http_host` parses and normalises
request hosts. `listen` owns explicit interface binds, connection tagging,
ACL/rate/concurrency guards and live enable/disable controls.

Enable `tls` for SNI certificate snapshots, LE production/staging chain
validation, HTTP-01 ACME account/order handling and internal self-signed
certificate generation. SNI selection and the PEM loader are re-exported
from config, preserving one implementation.

The four staging trust anchors retain their original DER bytes and source
references in `tls/le_staging_roots.rs`. They are used only for staging.

Test with `cargo test --locked -p daemon --features tls` and
`cargo test --locked -p daemon --no-default-features`. No issuance request
or deployed listener is needed for the test suite.

Source: [libs/daemon](https://github.com/markc/mixos/tree/main/libs/daemon).
