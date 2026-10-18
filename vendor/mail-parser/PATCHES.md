# mail-parser 0.11.5: nesting safeguard

This is upstream mail-parser 0.11.5 with the nesting-cap patch described in
README.patch.md. The upstream licence files and copyright notices are unchanged.
The workspace selects it through `[patch.crates-io]`.

The patch caps embedded message/rfc822 parsing at 64 levels, sharing the budget
between encoded and plain messages, so parsing and recursive drop stay bounded.
Its upstream base and SHA-256 are recorded in README.patch.md.

Guard: `cargo test -p maild --test parser_nesting` on the build worker. All three
tests must pass, including 100,000 embedded messages on a 2 MiB stack.
