# bridged

A Bus bridge to an external application's line-delimited JSON control
channel, one instance per application: `bridged@<app>` answers
`<app>.call`, `.commands`, `.execute`, `.inspect`, `.screenshot` and
`.status`. The manual is [docs/bridge.md](../../docs/bridge.md); the
contract (control channel, config schema, verbs, rc bands, start checks) is
[docs/spec/bridge/README.md](../../docs/spec/bridge/README.md).

A Mix component ([decision](../../docs/decisions/2026-10-10-mix-components.md)):
no Cargo package, identity in `component.mx`.

| Path | What |
|---|---|
| `scripts/lib/bridge_core.mix` | the stateless core: config, token, serve-name guard, line-JSON client |
| `scripts/bridged.mix` | the Bus service (`mix --serve … --name <app>`); `--start <app>` (the unit: checks, then the service as a child); `--check <app>` (the checks alone) |
| `units/bridged@.service` | one unit per application; config `/etc/mixos/bridge/<app>.conf.mix` |
| `tests/lib/fake_app.mix` | a stand-in application on `tcp_listen` (token or no auth, slow and dropping methods) |
| `tests/bridge_core_test.mix` | offline: config, token, guard, spec fixture, the client against stand-in apps |
| `tests/bridged_test.mix` | every verb over a private broker, the start refusals, `--start` (no child on a refusal, the child's exit code passed through), the app dying and coming back |

Run the tests from the checkout root with Mix >= 0.112.1. The mix binary the
tests run under is `BRIDGED_MIX`, else `MIXOS_TEST_MIX`, else
`/opt/mixos/bin/mix`. The Bus test needs a `noded`: `--noded <path>`, else
`MIXOS_TEST_NODED`, else `/opt/mixos/bin/noded`. An empty variable counts as
unset. `tools/mix_tests.mix` sets `MIXOS_TEST_MIX`, `MIXOS_TEST_NODED` and
`BRIDGED_MIX` to the fresh build:

```sh
mix services/bridged/tests/bridge_core_test.mix
mix services/bridged/tests/bridged_test.mix
```

**Security:** the Bus gives every mesh member every verb, so any mesh member
can drive a bridged application through `<app>.call`, within whatever its
control channel allows. Read the manual's Security section before bridging
anything.

**Verb registration.** The six verbs are declared in
`docs/spec/bus/verbs.conf.mix` (registry schema 3) as the `bridged` instance
family: `<app>.call`, `.commands`, `.execute`, `.inspect`, `.screenshot` and
`.status`, with `members: "instances"`, so the running instances are not
listed. The static gate `tools/verb_registry.mix` checks each family's owner
and suffix. `tools/verb_registry.mix --instances DIR` checks only the private
file-stem naming and collisions; it reads no config token. Mesh access is
unchanged: the Bus still gives every member every verb.

**Verb classes.** `<app>.call` and `<app>.execute` are `intent`;
`<app>.commands`, `<app>.inspect` and `<app>.status` are `query`.
`<app>.screenshot` is `intent`: its `focus` option can bring the window
forward, a change another caller sees, so it is not a query even though it
only captures. See the [decision](../../docs/decisions/2026-10-13-instance-verb-families.md).

Registry installation is a separate, deployed prerequisite. The scripts and
the registry must come from the same revision, and the registry must be at
`/opt/mixos/share/bus/verbs.conf.mix` (the default) or named by a `BRIDGED_VERBS`
override in `/etc/mixos/bridge/<app>.env`. Without one, the start check refuses
(fail closed). This checkout is not a live deployment, and nothing here
installs the registry for you.

The runtime guard reads the verbs and families lists. It does not enforce the
full canonical format (field order, spacing); the static gate does that.
