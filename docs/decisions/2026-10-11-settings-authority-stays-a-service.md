# The settings authority stays a separate service on noded

Status: accepted. Builds on
[Desktop settings have one native ABP authority](2026-10-07-settings-authority.md).

## Context

The question came up whether settingsd should be merged into noded, since
every settings read and publication already passes through the broker.

- noded is the Bus broker: every service and application on a node
  depends on it, and the ABP wire is frozen.
- settingsd is a domain authority. It owns a durable, sealed settings
  store, compiles design sources into projections, and answers
  `settings.apply` and `settings.get`.
- noded retains settingsd's last publication, so an application starts
  with the right look while settingsd is stopped or restarting.

## Decision

1. **settingsd stays its own process and unit**, a client of noded like
   editd, portald and releasesd. It is not merged into noded, and noded
   gains no settings logic beyond what any service gets: topic
   reservation for the registered owner, retention and owner stamping.
2. **The reasons are failure isolation and least privilege, not
   convenience:**
   - a settingsd crash, restart or upgrade must not drop the Bus;
     retained topics carry consumers through it;
   - design compilation is CPU work. One apply once took about 370 ms of
     compile time; inside the broker that would stall every message;
   - noded listens on the network; the settings store is written only by
     settingsd's service account;
   - noded is generic plumbing and runs on headless nodes with no desktop
     settings at all.
3. **The costs are accepted and handled where they arise:** start order
   (settingsd starts after noded is reachable, and the session target
   upholds it), one Bus hop per read, and changes that touch both, such as
   a new reserved topic, ship together.
4. **The exception:** a future single-binary image may host the settings
   authority in-process, as the same library behind the same verbs and
   topics. That needs its own decision; it is not a merge into noded's
   code.

## Consequences

- A proposal to fold another service (editd, portald, releasesd) into
  noded meets the same test and is refused for the same reasons unless a
  decision says otherwise.
- Bus-level improvements that help settings, such as publisher stamps for
  plain clients or conditional reads, belong in noded as generic features
  available to every service.
