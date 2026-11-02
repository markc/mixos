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

Release measurements at `19c36ad9e1baf141c6a82cef2f775092365b1c57`, CBC3:

| Separate release build | Bytes | MiB | Cargo elapsed |
| --- | ---: | ---: | --- |
| cargo build -p system --release → mix | 23,485,784 | 22.40 | 3m43s |
| cargo build -p mixd --release → mixd | 48,618,136 | 46.37 | 7m44s |

`mix` is 51.69% smaller than `mixd`; these are the files as produced by the
repository release profile, without a separate strip step. Both `ldd` lists
contain only `linux-vdso.so.1`, `libgcc_s.so.1`, `libm.so.6`, `libc.so.6` and
`ld-linux-x86-64.so.2`. No display, GPU, input or audio libraries.

`mix builtins --names` and `mixd run builtins --names` are byte-identical:
386 names, empty difference. This is deliberate: availability is enforced at
lint/runtime rather than by deleting metadata.

Both `tools/foreign_gate.mix --elf <binary>` runs returned 0 and printed:
`foreign_gate: 20 foreign crate(s), all listed; strict mix, mix-shell, system clean`.
The mixd-specific closure limitation is recorded below.

Measurement job: `BUILD_RC=0`, `20261102-174113-cecd7795-a1`, worker `cbc3`.
Log (operator-local):
`~/.cache/mixos/cbc-build/20261102-174113-cbc3-19c36ad9e1ba-20261102-174113-cecd7795-a1-b70b524b.log`.
The generated Cargo.lock diff in this job was retrieved and committed in
`14339b4a1`; the measurement binaries report dirty provenance from that lock
change. Subsequent final implementation removes the client-only unused
listener-address registry and owner accessor; sizes above are the measured
prototype, not a claim of byte-identical final artefacts.

Measurements are reproducible with `lab/mixd_measure.mix`, run through
cbc-build. It copies the client-only mix before building mixd, preventing
feature-unified artefacts from contaminating the measurement.

Release runtime probes independently caught `NEEDS_MIXD` for all four calls
and checked their messages. `mixd` process suite at `19c36ad9e1ba` passed all
10 tests: `mixd run` listener + script arguments, five personalities by both
subcommand and argv[0], HTTP SIGINT, and seven TCP/WS SIGTERM cases.
`BUILD_RC=0`, worker `cbc3`, log:
`~/.cache/mixos/cbc-build/20261102-174259-cbc3-19c36ad9e1ba-20261102-174119-daab74cd-a1-c481daf2.log`.

Final implementation: `d479b9075ca819d6e0359f8462417bb47619cbf6`.
The final report commit changes only this report.

| Command | BUILD_RC | Worker | Log under ~/.cache/mixos/cbc-build/ |
| --- | ---: | --- | --- |
| cargo test -p mix --locked --no-fail-fast | 0 | cbc3, secondary checkout | 20261102-175154-cbc3-d479b9075ca8-20261102-175154-9683af32-a1-3d88725d.log |
| cargo test -p system --locked --no-fail-fast | 0 | CBC-scheduled temporary light worker | 20261102-175151-*-d479b9075ca8-20261102-175151-d3f6841c-a1-26ef5281.log |
| cargo check --workspace --all-targets --locked | 0 | cbc3, secondary checkout | 20261102-175618-cbc3-d479b9075ca8-20261102-175618-9aa6929b-d0fdf686.log |

The system log path uses a wildcard for the private worker label. A host-neutral copy is also saved locally at `/tmp/mixd-system-final.log`.

The system run includes all four structured NEEDS_MIXD refusals and all four
static MIX-E1513 diagnostics. Full saved logs were read, not just their
compiler-error tails. The library run exercises existing ws consumers.
The build queue automatically assigned the one-crate system test to its
configured temporary-worker shadow checkout; no cargo command was invoked directly
in the agent worktree or desktop project checkout.

Workspace check: `BUILD_RC=0`, job `20261102-175618-9aa6929b`, 104.8s.
Primary CBC3 checkout was dirty with another session's source changes, causing rc 75 (not-run)
and automatic requeues. Those attempts were cancelled and replaced by
`--direct --worker cbc3 --kind check --checkout mixos-light`; the queue
otherwise retained incompatible slot requirements from the cancelled job.
The secondary checkout is a configured CBC3 slot, not a local build.

Earlier failures were fixed rather than excluded: feature-reporting allowlist
and lint documentation omissions; missing client socket2 and shared wait
slice; listener key lookups still present in client-only close; generated
Cargo.lock tripping clean-source provenance; and the new runtime probe using
shell-classified one-line try syntax instead of a real Mix source file.
The final library/system runs above have no failing targets. Workspace check
is green with pre-existing warnings. `mix lint` on the aggregate test runner
and measurement recipe reports no errors, warnings or notes.

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

## Job 2 — server extension crate and rebuild scope

Work starts at `9691ec118`. The question is whether a crate boundary, rather
than a Cargo feature, can keep inbound capability out of a workspace-built
shell. This section records job 2; the measurements above remain job 1 data.

### Registry and resource API

The provisional API is `mix::extensions::register(&'static dyn BuiltinFamily)`.
A family supplies its name, `BuiltinInfo` entries (including contracts and
capability classes), and dispatch. Registration is explicit, process-local
and idempotent for the same instance. Duplicate names and replacement of
existing core builtins are refused; the six reserved server placeholders
are deliberately implementable by an extension. Core keeps their metadata
and NEEDS_MIXD refusal. Lint checks actual registration, not Cargo features.

```rust
mix_server::register()?; // embedding host, before evaluation
// BuiltinFamily: name(), entries(), call(), optional sockets()
// SocketExtension: accept(), subscribe(), own(), retire(),
//                  is_listener(), local_addr(), close(), unmark(), subscribed()
```

`mixd` installs the family before dispatching either scripts or personalities.
`system` never installs it. `net-server` and `server` were removed from core;
`ws` remains an alias of `net-client`. Cargo feature unification can compile
the extension crate but cannot execute its registration in another binary.

The six implementations now live under `libs/mix-server/src/`: TCP listener
tables, bind/accept loops and subscription readers; UDP listener creation;
the static HTTP server, HTTP receive and legacy UDP receive. Shared connected
socket representation stays in core. `server_api` exposes opaque permits and
ownership, monotonic socket IDs, connected-stream/UDP adoption, handle-presence
and removal operations, bounded queue records, the cancellation/join wrapper,
and shared argument validators/readiness waits. Client tables and stream
variants remain private. Listener tables are entirely in the extension.

Generation cleanup follows the existing NativeEvents lifecycle: stop/join
subscription readers, retire listener resources through the provider, then
close owned connected handles. Accepted streams retain their permit and
generation ownership while pulled or subscribed. A test-support feature
exposes generation/UDP observations solely for migrated low-level tests.

### Findings during implementation

The first attempt kept server unit tests in core and imported the extension
through a dev dependency. Rust builds the unit-test crate and its normal
library dependency as different type identities, so Value, errors and socket
leases could not cross that cycle. Server-only unit tests were moved into
the extension's integration tests. Core client ownership tests remain in
core; existing core integration tests explicitly register the server family
when they need it. No production constructor registers implicitly.

The API is broader than a name-to-function map: accept readiness, bounded
queues, cancellation, ownership and client handle interoperation are all
part of the seam. Its socket-specific trait and test-support surface are
prototype debt for the survey. The registry currently takes a read lock and
walks family metadata during lookup; this job measures rebuild scope, not
runtime throughput. Native Rust service implementations remain unchanged.

Workspace proof, rebuild timings and final test receipts follow below.
