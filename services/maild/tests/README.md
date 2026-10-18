# Maild tests

Run Rust tests on a configured build worker, against the exact pushed commit.
The full family suite is `cargo test -p maild -p maild-auth -p maild-rules
-p maild-bayesian -p mds`. Shared Bus changes also require `cargo test -p bus`.

All source tests have been ported, including the binary's tests in the
library's `command` module. No test files were dropped. Counts below include
unit and integration test declarations, excluding examples in comments.

| Crate | Source | Port | Difference |
|---|---:|---:|---|
| maild | 1,481 | 1,483 | Two repository registry checks added |
| maild-auth | 53 | 53 | None |
| maild-rules | 73 | 73 | None |
| maild-bayesian | 41 | 41 | None |

One maild sequence-map test is release-only: the debug and release constructors
deliberately handle duplicate UIDs differently. The debug suite therefore
collects 1,482 tests: 1,476 pass and six are ignored. A release build additionally
collects `from_entries_release_build_coalesces_duplicate_uids`.

The six ignores are inherited:

- `native_bus_submission_replays_across_actors_without_duplicate_mail` needs
  an exact built native noded binary and a persistent fixture directory. Set
  `MIXOS_NATIVE_MAIL_NODED` and `MIXOS_NATIVE_MAIL_FIXTURE_DIR`, then run
  `cargo test -p maild --test native_bus_submission -- --ignored` on the worker.
  This starts an isolated broker and never contacts an external MX.
- Five `jmap::email::tests::email_set_contract_*` functions are deliberately
  panicking placeholders. Their ignore reasons identify the missing executable
  assertions: multi-mailbox creation, empty mailboxIds rejection, per-id partial
  updates, update-before-destroy ordering and blob import idempotence. They
  remain present; the port does not turn those placeholders into coverage.

The `parser_nesting` suite directly exercises the vendored mail-parser 0.11.5
without maild's MIME preflight. It must pass all three cases on a 2 MiB stack,
including parse and drop of 100,000 nested messages. Do not remove the patch or
these tests when updating dependencies.
