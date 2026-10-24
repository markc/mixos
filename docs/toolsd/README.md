---
title: Tool runs
description: Runs the tools webd's admin panel queues (whois, dig, traceroute, shwho) as an unprivileged drain, outside webd's sandbox.
---

# Tool runs

webd's admin panel has a tools modal: look up a domain's whois record, query
DNS, trace a route, or run `shwho` (a whois and DNS summary). webd itself
cannot run programs. Its embedded Mix sandbox denies process execution
outright, so the panel only *queues* a run, and `toolsd` runs it.

## How a run flows

1. The panel posts a run. webd checks the caller's role against the tool
   registry, validates the parameters, and stores the run in its own queue
   (the `tools.` schema beside the site's CMS database).
2. webd calls the authority-free Bus command `toolsd.wake`. It takes no
   arguments and names no tool; it only asks toolsd to go and look.
3. The wake citizen starts the drain unit, `toolsd-drain.service`.
4. The drain claims the next run from webd over HTTPS with its own secret,
   checks the role and parameters **again** against the same registry, runs
   the tool with a fixed argv, a minimal environment and a timeout, and
   streams or reports the output back.
5. The panel polls webd and shows the output as it arrives.

A timer runs the drain every five minutes as a backstop, for a wake that
arrived while a drain was finishing, or for a burst larger than one drain
takes.

## Two identities

| Unit | User | Holds | May do |
|---|---|---|---|
| `toolsd.service` | `toolsd-wake` | nothing | start `toolsd-drain.service`, through one polkit rule |
| `toolsd-drain.service` | `toolsd` | the tools claim secret | claim and report tool runs; run the registered tools |

The split exists because running a tool blocks. A 30 second whois inside
the Bus citizen would stall every other caller, so the work happens in a
separate short-lived process. The wake identity holds no credential at all,
which is what lets webd call it.

## What the drain may reach

The drain unit denies every private and management address range and
re-allows only the local resolver stub (`127.0.0.53`) and the webd it claims
from. Public addresses stay reachable, because whois talks to registry
servers directly. So code running as `toolsd` cannot probe the mesh or the
hypervisors.

## Installing

| Source | Installed as |
|---|---|
| `services/toolsd/scripts/` | `/opt/mixos/lib/toolsd/` (root-owned, not writable) |
| `services/webd/vhosts/apps/tools_registry.mix` | `/opt/mixos/lib/toolsd/lib/tools_registry.mix` |
| `services/toolsd/units/*.service`, `*.timer` | `/etc/systemd/system/` |
| `services/toolsd/units/toolsd.tmpfiles.conf` | `/etc/tmpfiles.d/toolsd.conf` |
| `services/toolsd/units/49-toolsd-wake.rules` | `/etc/polkit-1/rules.d/` |

The registry is the same file webd uses, so both ends check against one
list. `tools/shwho.mix` must be `root:root 0444` under root-owned parents:
the drain checks its owner and mode before every run.

Each host also needs:

- the system users `toolsd` and `toolsd-wake`;
- `/etc/mixos/toolsd/env` with `TOOLS_BASE_URL` (the webd vhost to claim
  from, for example `https://admin.example.com`) and `TOOLS_CA_FILE` (the
  certificate chain to verify it with);
- `/etc/mixos/toolsd/secret`, `0400 root`, matching the vhost's tools
  secret;
- a drop-in, `/etc/systemd/system/toolsd-drain.service.d/webd.conf`, with
  `IPAddressAllow=<webd address>/32`.

Then enable `toolsd.service` and `toolsd-drain.timer`.

## Adding a tool

A tool needs an entry in `tools_registry.mix` (its label, minimum role,
timeout, output cap and parameter types) and an argv template in
`scripts/lib/tools_exec.mix`. The argv template exists only on the toolsd
side; webd knows tool names and parameters, never command lines.
