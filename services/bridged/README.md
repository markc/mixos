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

Run the tests from the checkout root with Mix >= 0.112.1 (the Bus test needs
a `noded`, default `/opt/mixos/bin/noded`, or `--noded <path>`; to test
another mix binary, run the tests with it and set `BRIDGED_MIX` to it):

```sh
mix services/bridged/tests/bridge_core_test.mix
mix services/bridged/tests/bridged_test.mix
```

**Security:** the Bus gives every mesh member every verb, so any mesh member
can drive a bridged application through `<app>.call`, within whatever its
control channel allows. Read the manual's Security section before bridging
anything.

**Verb registration is pending.** The verbs are an instance family
(`<app>.<verb>`, owner `bridged`, runtime members). They are registered in
`docs/spec/bus/verbs.conf.mix` together with the verb registry gate's family
mapping, not before. Nothing installs the registry to
`/opt/mixos/share/bus/verbs.conf.mix` yet: until it does, a unit needs
`BRIDGED_VERBS` in `/etc/mixos/bridge/<app>.env`, or the start check refuses
(fail closed).
