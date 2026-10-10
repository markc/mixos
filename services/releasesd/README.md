# releasesd

Upstream application releases for MixOS: follows the GitHub repositories in
the user's catalogue, shows the installed version against the latest
release, and installs each release's Linux tarball per user, verified and
with one-step rollback. The manual is [docs/releases.md](../../docs/releases.md).

A Mix component ([decision](../../docs/decisions/2026-10-10-mix-components.md)):
no Cargo package, identity in `component.mx`.

| Path | What |
|---|---|
| `scripts/lib/releases.mix` | all of the logic, as a module |
| `scripts/releasesd.mix` | the Bus service (`mix --serve … --name releases`) |
| `scripts/releases.mix` | the command line, same logic in-process |
| `units/releasesd@.service` | the session unit |
| `tests/releases_test.mix` | offline: validation, verification, install, rollback, links |
| `tests/releasesd_test.mix` | every verb over a private broker and a stand-in API |

Run the tests from the checkout root (they need `bsdtar`; the Bus test needs
a `noded`, default `/opt/mixos/bin/noded`, or `--noded <path>`):

```sh
mix services/releasesd/tests/releases_test.mix
mix services/releasesd/tests/releasesd_test.mix
```

The verbs are registered in `docs/spec/bus/verbs.conf.mix`; the catalogue
schema and layout are in `docs/spec/releases/README.md`.
