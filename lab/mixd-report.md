# mixd job 1 — discovery lab

Branch: `lab/mixd`; baseline: `b7c22b99727b`. No merge, install, unit change,
desktop interaction or restart. This is a disposable prototype for survey.

## Result

`ws = ["net-client", "net-server"]` preserves existing consumers.
`server = ["net-server"]`; server depends on client because accepted streams
use the existing client socket representation. `mix-shell/server` forwards it.
`system` defaults to no personalities and uses client-only `mix-shell`.
`mixd` defaults to the five existing Rust personalities and enables server.

The four names remain in metadata. Client-only calls raise `NEEDS_MIXD`, naming
the builtin and `mixd run <script>`. Static calls receive `MIX-E1513`.
`mixd run` re-execs the same executable with a private argv[0], removing `run`
before invoking the unchanged shell runner. Personality subcommands use the
same technique so service mains receive their original argument layout.
There is no reactor in this job.

## Feature-site audit

All 67 plain `cfg(feature = "ws")` sites in baseline `builtins.rs` were
inspected individually. Classification below uses baseline line numbers:

| Sites | Allocation |
| --- | --- |
| 914, 976, 978 | server builtin dispatch: udp_listen, tcp_listen, tcp_accept |
| 919–974 excluding those above, 980 | client dispatch, including UDP handle operations |
| 22676–24434 | client implementations, errors, send/receive helpers and shared ownership |
| 24507–24873 except 24852 | server listeners, accept errors, adoption, accept loops and counters |
| 24852 | shared timeout slice also used by client send/receive waits |
| 24927 | shared local-address query; listener lookup additionally server-gated |
| 25022 | shared socket subscriptions; listener kind, constructor, cleanup and accept readers additionally server-gated |
| 29329 | client close; listener branches additionally server-gated |
| 37476, 37546, 37837, 37904 | client tests against Rust fixture peers |
| 41144 | compatibility `ws` feature-reporting assertion stays `ws` |

Compound cfgs (Unix connect, platform waits, test modules), negative cfgs and
the cfgs in `native_events.rs`, `udp.rs` and `evaluator.rs` were also split.
Unix client connect needs socket2, so that dependency belongs to net-client.
HTTP serving was previously unconditional; its dispatch and implementation
now require net-server. Shared HTTP receive helpers remain compiled.

## Scripts and harnesses affected

Source scan: `tcp_listen|tcp_accept|udp_listen|http_serve` calls in Rust process
tests and in `tools/`, `share/`, and service `*.mix` files.

| Location | Action |
| --- | --- |
| cli/system/tests/http_serve_signal.rs | moved to cli/mixd/tests; invokes mixd run |
| cli/system/tests/tcp_wait_sigterm.rs | moved to cli/mixd/tests; invokes mixd run |
| cli/system/tests/personalities.rs | legacy optional-feature suite gated on its five required features |
| services/releasesd/tests/releasesd_test.mix | HTTP fixture launched with mixd run |
| services/bridged/tests/bridge_core_test.mix | run outer test with mixd run; fake peers use MIXOS_TEST_MIXD |
| services/bridged/tests/bridged_test.mix | fake peers use MIXOS_TEST_MIXD; production client citizen stays mix |
| services/bridged/tests/lib/fake_app.mix | launched under mixd run by both bridge harnesses |
| services/dlnad/scripts/dlnad.mix | listener requires mixd run; lab service, no unit changed |
| services/dlnad/scripts/lib/ssdp.mix | udp_listen requires mixd server context |
| tools/mix_tests.mix | selects mixd run for bridged and releasesd listener harnesses; passes MIXOS_TEST_MIXD |
| tools/, share/ | no direct builtin calls found |

Private hub test scripts named by the plan are outside this lab worktree and
were not modified. `MIXOS_TEST_MIXD` overrides the default prefix/bin/mixd in
bridge fixtures; no binary was installed. Full service integration harnesses
need a test host with the new mixd available.

## Measurements and validation

Pending CBC receipts. Release measurements are reproducible with
`lab/mixd_measure.mix`, run through cbc-build. It copies the client-only mix
before building mixd, preventing feature-unified artefacts from contaminating
the measurement.

## Surprises and limits for the survey

* `cargo build --workspace` unifies mixd/server and consumers of ws into the
  shared mix and mix-shell crates. Its mix therefore regains server builtins;
  system's empty personality defaults still apply. Separate package builds
  are required to obtain and verify the intended shell artefact.
* Component policy only permits system to depend on the CLI mix-shell crate.
  The requested mixd shape is refused by component_index, and hence the site
  generation pre-commit hook. Lab commits bypass landing hooks; policy and
  generated site remain unchanged. Derivation needs an explicit rule change.
* foreign_gate has a fixed list of strict packages. `--elf mixd` tests ELF
  dependencies, but does not add mixd to that list or prove a mixd-specific
  foreign allowlist closure. No foreign policy was edited.
* The brief gates exactly four builtins. `http_recv` and legacy `udp_recv`
  still bind inbound sockets. Thus job 1 does not yet enforce the plan's
  broader "never accepts inbound" promise. This needs a follow-up decision.
* Existing optional system personality features remain as compatibility
  opt-ins; they are absent from default system builds.
* Module splitting, reactor, performance comparisons, fault contract and
  installation changes are later jobs, not claimed by this report.
