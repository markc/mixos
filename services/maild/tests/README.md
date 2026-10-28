# Maild tests

Run Rust tests on a configured build worker, against the exact pushed commit.
The full family suite is `cargo test -p maild -p maild-auth -p maild-rules
-p maild-bayesian -p mds`. Shared Bus changes also require `cargo test -p bus`.

All source tests have been ported, including the binary's tests in the
library's `command` module. No test files were dropped. These are historical
port counts, before subsequent protocol work added coverage:

| Crate | Source | Port | Difference |
|---|---:|---:|---|
| maild | 1,481 | 1,483 | Two repository registry checks added |
| maild-auth | 53 | 53 | None |
| maild-rules | 73 | 73 | None |
| maild-bayesian | 41 | 41 | None |

One maild sequence-map test is release-only: the debug and release constructors
deliberately handle duplicate UIDs differently. A release build additionally
collects `from_entries_release_build_coalesces_duplicate_uids`.

The inherited native-broker test remains opt-in:

- `native_bus_submission_replays_across_actors_without_duplicate_mail` needs
  an exact built native noded binary and a persistent fixture directory. Set
  `MIXOS_NATIVE_MAIL_NODED` and `MIXOS_NATIVE_MAIL_FIXTURE_DIR`, then run
  `cargo test -p maild --test native_bus_submission -- --ignored` on the worker.
  This starts an isolated broker and never contacts an external MX.

The five former `jmap::email::tests::email_set_contract_*` placeholders now
run as authenticated HTTP tests in `jmap_import.rs`: multi-mailbox creation,
empty mailboxIds rejection, per-id partial updates, update-before-destroy
ordering and duplicate import without a second creation/change. The ordering
test checks RFC 8620 §5.3; the old placeholder's whole-request transaction
requirement was stronger than the RFC. The import test checks RFC 8621 §4.8's
`alreadyExists`/`existingId` response, replacing the placeholder's incorrect
successful-create response for a duplicate. The same suite tests import state
preconditions, Received date defaults, partial failure, account-owned CAS ids,
concurrent duplicate rejection, thread properties and upload/import/submission.
It also covers transactional quota enforcement, EAI byte preservation, foreign
blob refusal and mailbox/import/submission creation references in one request.

`config_paths.rs` checks every configured path through both loaders and
rejects logical root escapes. `socket_activation.rs` passes actual listening
fds with LISTEN_PID/LISTEN_FDS/LISTEN_FDNAMES to a subprocess, checking name
routing, multiple sockets, fallback binds, close-on-exec and invalid metadata.
It also checks that activation metadata is cleared before runtime startup.
`service_units.rs` prevents capability removal before host activation is
configured and checks that socket templates choose the matching service
instance without enabling any address.

The JMAP HTTP suite also covers null import dates, invalid-MIME SetErrors,
optional batched thread reads, creation references throughout implemented
/set methods, dependent mailbox creates and submission success actions.
The `round2_*` HTTP regressions reject malformed stage/action arguments before
writes, ignore submission actions without a successful operation in that call,
and fence every implemented `/set` with its initial `ifInState`. A matching
guard allows later update/destroy stages after creates have advanced state.
SQLite statement tracing on a current-thread HTTP executor detects blocking
storage calls in import, blob-based creation and Email/get. Duplicate imports
into another mailbox set must leave memberships, keywords, dates and state
unchanged. The current update path supports single-mailbox moves, rather than
membership union; that requires an atomic operation preserving per-membership
keywords and IMAP-only flags.

The `parser_nesting` suite directly exercises the vendored mail-parser 0.11.5
without maild's MIME preflight. It must pass all three cases on a 2 MiB stack,
including parse and drop of 100,000 nested messages. Do not remove the patch or
these tests when updating dependencies.

After building maild on the worker, run
`<prefix>/bin/mix services/maild/tests/package_test.mix target/debug/maild`.
This builds throwaway package archives, extracts the server archive, executes
the packaged `maild` binary and checks its identity, service/socket units and parser licences.
Other binary entries are fixtures; the gate makes no desktop-package claim.
