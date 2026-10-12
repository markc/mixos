---
title: The host service
description: How hostd answers questions about the host and carries out privileged host changes through closed, typed verbs behind standing grants, receipts and rollback, so agents never need sudo.
---

# The host service

`hostd` is the one MixOS service that runs as root on a host. It does two
jobs:

- **It describes the host:** hardware, OS, the MixOS mode, and who manages
  the network.
- **It makes privileged changes** through a short list of closed, typed
  verbs. Each change is checked, recorded, and reversed if it doesn't work.

Agents and session code ask hostd instead of running `sudo`. The rules
behind it are in the
[hostd decision](decisions/2026-10-12-hostd.md).

## Calling it

Locally, hostd listens on `/run/hostd/hostd.sock` (mode 0660, group
`hostd`). Frames are JSON, each ended by a NUL byte, many per connection:

```text
-> {"verb":"net.state","body":{}}\0
<- {"rc":0,"body":{...}}\0
```

`rc` is 0 on success and 10 on refusal, with `body.status` saying why. From
a shell:

```text
hostd call host.info
hostd call net.link.apply '{"link":"eth1","config":{"mode":"dhcp"}}'
```

Read verbs are also on the Bus, owned by `hostd`. Changes are never taken
from the Bus.

## Verbs

| Verb | Tier | What |
|---|---|---|
| `host.info` | read | hostname, kernel, OS, MixOS mode, container, uptime, CPU, memory |
| `net.state` | read | which network managers run, networkd links and files, resolved DNS, iwd stations, hostd's own files |
| `hostd.status` | read | frozen or not, the policy's state, the caller's identity, and the grants it holds: `held` (names) and `held_grants` (each in full: verbs, limits, expiry, rate) |
| `hostd.receipt` | read | one change's receipt: what it changed, under which grant, how it ended (never file contents) |
| `hostd.freeze` | control | refuse every change until thawed: root on the host, or a grant naming it |
| `hostd.thaw` | control | accept changes again and lift grant suspensions: root on the host only |
| `net.link.apply` | write | configure one link through networkd, rolled back unless it comes up |
| `hostd.undo` | write | put back what an applied change replaced; needs no grant of its own (see below) |

`host.info` is also published as the retained topic `hostd.info`.

## Who may change what

Reads are open to anyone who can reach hostd. Changes need a **standing
grant** in `/etc/hostd/policy.mx`, a strict-data document that only root
can change:

```text
{
  schema: 1,
  grants: [
    {name: "desk-network",
     caller: {cgroup: "/machine.slice/systemd-nspawn@desk.service/*"},
     verbs: ["net.link.apply"],
     limits: {links: ["eth0", "wlan0"]},
     expires: "2027-01-01T00:00:00Z",
     rate: {count: 3, per_s: 600}}
  ]
}
```

- **The caller is matched by what the kernel says,** never by what it
  sends: its uid and its cgroup. A cgroup only counts when hostd holds a
  pidfd for the caller and the caller was alive after the cgroup was read.
  Both are read again for every request.
- **`X/*` matches the cgroup `X` and everything below it.** Name a subtree
  the caller can't move itself into, such as one under `machine.slice`.
- **Verbs** are exact names or a family such as `net.*`. No grant can name
  `hostd.thaw`.
- **Limits** are allow-lists the verb checks its arguments against
  (`links` for `net.link.apply`). A key the grant doesn't list is
  unrestricted.
- **Rate** applies per caller, with a grant-wide cap of four callers' worth.
- **Default deny.** No policy file means no grants.

The policy is read for every change, so an edit takes effect at once. A
file that anyone but root could change, or that sits in a directory anyone
but root could change, is refused.

## What a change does

Every write follows the same pattern:

1. **Check:** the request is typed and validated (no free text reaches a
   file), then the grant, limits, rate and freeze state.
2. **Record:** a receipt keeps the previous content of every file the change
   replaces.
3. **Apply:** hostd writes only files it names itself, and never edits a file
   it didn't write.
4. **Prove:** a health check runs within the change's window (5 to 60
   seconds, 20 by default).
5. **Keep or revert:** a change that doesn't pass is rolled back
   automatically.

One change runs at a time, host-wide. A change waits at most 15 seconds
behind others (`busy` after that), and the whole gate runs again once it
holds the lock, so a freeze, a suspension, an expiry or a policy edit that
lands while it waits is obeyed. A change runs to the end, rollback
included, even if the caller hangs up.

**A stop waits for a running change** (up to 80 seconds), and a change that
a crash or a forced kill cuts short is rolled back when hostd next starts,
before it accepts any change.

Recovery looks at what each file holds before it acts. It puts back the
backup only over content hostd itself installed. A file someone else has
changed since is left alone, and the receipt is marked `abandoned`. A
rollback, undo or recovery that can't finish, or a receipt that can't be
recorded, blocks all changes (`recovery_pending`). Every later change
retries it first, and changes resume once it finishes.

**Undo puts back exactly what a receipt replaced,** and only while the file
still holds what that change installed. If a later change, or anyone's
edit, has replaced it since, undo is refused (`superseded`): undo the later
change first.

**Two rollbacks in a row** under one grant suspend that grant until root on
the host thaws hostd.

## Network links

`net.link.apply` gives one link a networkd configuration:

```text
{"link": "eth1", "config": {"mode": "dhcp"}}
{"link": "eth1", "config": {"mode": "static",
  "addresses": ["192.0.2.10/24"], "gateway": "192.0.2.1", "dns": ["192.0.2.53"]},
 "window_s": 30}
```

- hostd writes `/etc/systemd/network/50-hostd-<link>.network`, reloads
  networkd and reconfigures the link.
- **It passes** when networkd reports the link configured from hostd's file,
  carrying every requested address with its prefix length. With DHCP or a
  gateway, the default route must also be installed (through the requested
  gateway, if static) and **the gateway must answer**: hostd sends it one UDP
  datagram and reads the kernel's neighbour table until the gateway's
  address resolves. A gateway that doesn't exist fails the change, even one
  inside the link's own subnet.
- **A link another file already configures is refused** (`foreign_config`),
  and so is a link that carries addresses nobody's networkd file set, by
  hand or by another manager such as NetworkManager
  (`unmanaged_configured`). hostd's rollback restores files, so it can't
  give such a link its old runtime configuration back, and it doesn't take
  it over.

The reply to a change that passed (`rc` 0):

```text
{"receipt": "<id>", "outcome": "applied", "reason": null, "grant": "<grant>",
 "grant_suspended": false, "link": {"admin": "configured", "operational": "routable"}}
```

A change that failed and was reversed comes back with `rc` 10, as
`{"status": "rolled_back", "detail": {...}}`, where `detail` has the same
keys. Keep the `receipt` id to undo the change later, or to look it up with
`hostd.receipt`.

**Undo needs no grant of its own.** `hostd.undo {receipt}` is allowed to
whoever holds a grant that covers the original change: its verb and its
limits (for `net.link.apply`, that link). A grant that may configure `hd0`
may undo its own changes to `hd0`. Naming `hostd.undo` in a grant adds
nothing.

## Stopping it

`hostd.freeze` stops every change at once and survives restarts and
reboots. An unreadable freeze record counts as frozen. Only root on the
host itself thaws it, never root inside a container.

## Refusals

| Status | Meaning |
|---|---|
| `not_served` | no such verb |
| `bad_request` | the body doesn't fit the verb's schema |
| `local_only` | a control or write verb arrived over the Bus |
| `caller_unverified` | hostd couldn't prove the caller's cgroup |
| `no_grant` | no unexpired grant covers this caller and verb |
| `outside_grant` | a grant covers the verb but not these arguments |
| `grant_suspended` | the circuit breaker suspended the grant |
| `rate_limited` | the grant's rate is used up |
| `frozen` | hostd is frozen |
| `policy_invalid` | the policy can't be loaded; changes are refused |
| `root_only` | only root on the host may do this |
| `foreign_config`, `unmanaged_configured`, `no_such_link` | `net.link.apply` won't touch this link |
| `rolled_back` | the change failed its health check and was reversed |
| `superseded` | undo refused: the file no longer holds what that change installed |
| `not_applied` | undo refused: the change was rolled back or already undone |
| `rollback_failed`, `undo_incomplete` | a rollback or undo couldn't finish; it is retried, and writes wait |
| `recovery_pending` | an unfinished change is being retried and hasn't finished yet; writes wait |
| `timeout`, `busy` | too slow, or too many requests at once |
