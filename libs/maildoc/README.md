<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
# maildoc

Inert mail documents in ABP framing. See [the manual](../../docs/maildoc/README.md).

`write(&MailDocument) -> Result<String, Refusal>` refuses non-canonical input.
`validate(&MailDocument) -> Result<(), Refusal>` exposes the same checks for
Email and ced. Successful writes parse back to an equal document; invalid
headers, non-UTF-8 paths and oversized output return typed refusals.

Run `cargo test -p maildoc` through the build cluster.
