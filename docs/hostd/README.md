---
title: The host service
description: How hostd answers questions about the host and carries out privileged host changes through closed, typed commands with receipts, rate limits and rollback, so agents never need sudo.
---

# The host service

`hostd` is the one MixOS service that runs as root on a host. It does two
jobs:

- **It describes the host:** hardware, OS, the MixOS mode, and who manages
  the network.
- **It makes privileged changes** through a short list of closed, typed
  commands. Each change is checked, recorded, and reversed if it doesn't work.

Agents and session code ask hostd instead of running `sudo`. The rules
behind it are in the
[hostd decision](../docs/decisions/2026-10-12-hostd.md).

## Calling it

Locally, hostd listens on `/run/hostd/hostd.sock` (mode 0666). Frames are
JSON, each ended by a NUL byte, many per connection:

```text
-> {"command":"net.state","body":{}}\0
<- {"rc":0,"body":{...}}\0
```

`rc` is 0 on success and 10 on refusal, with `body.status` saying why. From
a shell:

```text
hostd call host.info
hostd call net.link.apply '{"link":"eth1","config":{"mode":"dhcp"}}'
```

Every command is also on the Bus, owned by `hostd`. Writes accept local
Bus requests stamped `broker_origin: local`, and Unix-socket callers.
Mesh-origin writes remain refused.

There is one hostd per host, run as root and socket-activated:

```text
hostd serve                      # local socket, plus the Bus when noded is up
hostd serve --no-bus             # local socket only
```

`hostd serve` refuses to take over a socket another hostd is serving.

## Commands

| Command | Tier | What |
|---|---|---|
| `host.info` | read | hostname, kernel, OS, MixOS mode, container, uptime, CPU, memory |
| `net.state` | read | which network managers run, networkd links and files, resolved DNS, iwd stations, hostd's own files |
| `hostd.status` | read | frozen or not, and the caller's origin |
| `hostd.receipt` | read | one change's receipt: what it changed, its origin, how it ended (never file contents) |
| `hostd.freeze` | control | refuse every change until thawed: any local caller |
| `hostd.thaw` | control | accept changes again: any local caller |
| `net.link.apply` | write | configure one link through networkd, rolled back unless it comes up |
| `net.wifi.scan` | write | scan for Wi-Fi networks on one device (iwd); drives the radio |
| `net.wifi.connect` | write | connect a Wi-Fi device to a network (iwd), put back unless connected in time |
| `hostd.undo` | write | put back what an applied change replaced; any local caller |
| `unit.control` | write | start, stop or restart a named `.service` unit with `{unit, action}`; hostd's own units are protected |
| `net.wg.apply` | write | configure a WireGuard link through networkd, rolled back unless it comes up and its peers complete a handshake |
| `desk.restart` | write | restart the named desk machine, session and console included (starts `desk-restart@DESK.service`); the host stays up; no undo |
| `desk.stop` | write | power off the named desk machine and give the console back (starts `desk-stop@DESK.service`); the host stays up; no undo |

The desk commands take `{"desk":"desk-2"}`. The name must start with a
lowercase letter, contain only lowercase letters, digits and hyphens, and
be at most 32 characters. Every local caller may target any desk.

A desk power action is a cycle that outlives the caller: a restart stops the
desk's session, reboots the machine, starts the session again and switches
the console back to the desk's VT if it was in front (returning it to the
previous VT if the compositor does not come up); a stop returns the console
to the VT that was in front, stops the session and powers the machine off.
hostd runs none of these steps. It starts the host's template unit for the
action with `systemctl start --no-block`, records a receipt that the cycle
started, and answers. The host installs `desk-restart@.service` and
`desk-stop@.service`; the unit's journal says how the cycle ended. If either
unit is missing, the command is refused `unit_failed`.

Only hostd on the host can do this. A hostd running inside a desk refuses
both commands `in_container`. The desk reaches the host's hostd through its
socket bound into the machine.
Every refusal is logged at warn under `hostd::audit`.

`host.info` is also published as the retained topic `hostd.info`.

## Who may change what

Reads are open to every caller. Writes and control commands accept every
caller on the same host, over the Unix socket or the local Bus. No policy
file, cgroup membership, uid or grant determines access. Broker provenance
is required; unstamped Bus requests are malformed. Writes from other nodes
are refused with `local_only`.

Writes are limited to 30 calls per minute per caller and command, plus a
host-wide cap of 120 writes per minute. Socket identity (uid and cgroup)
and broker-stamped sender names provide rate-accounting keys only.

## What a change does

Every write follows the same pattern:

1. **Check:** the request is typed and validated (no free text reaches a
   file), then local origin, rate and freeze state.
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
holds the lock, so a freeze that lands while it waits is obeyed. A change runs to the end, rollback
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
- **`route_metric`** (static with a gateway, or DHCP) sets the metric of
  the default route the link installs. A higher metric loses to a lower
  one, so a second uplink with a high metric never takes over the host's
  default route.
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
{"receipt": "<id>", "outcome": "applied", "reason": null, "link": {"admin": "configured", "operational": "routable"}}
```

A change that failed and was reversed comes back with `rc` 10, as
`{"status": "rolled_back", "detail": {...}}`, where `detail` has the same
keys. Keep the `receipt` id to undo the change later, or to look it up with
`hostd.receipt`.

Any local caller may undo an applied receipt, even one created by another
caller. Receipt and file-integrity checks still apply.

## WireGuard links

`net.wg.apply` gives one WireGuard link its networkd configuration:

```text
{"name": "wg-home", "addresses": ["10.99.0.1/24"], "listen_port": 51820,
 "peers": [{"public_key": "<base64>", "endpoint": "vpn.example.com:51820",
            "allowed_ips": ["10.99.0.0/24"], "keepalive_s": 25}],
 "window_s": 25}
```

- hostd writes `50-hostd-<name>.netdev`, `50-hostd-<name>.network` and the
  private key `50-hostd-<name>.key` in `/etc/systemd/network`.
- **The key never leaves the host.** hostd makes it the first time,
  readable only by root and group `systemd-network` (mode 0640), and keeps
  it across later applies. Replies carry only the `public_key`, for the
  peers' side.
- **It passes** when the link comes up configured from hostd's file with
  every address, and **every peer with an endpoint completes a handshake**
  within the window. A peer that never answers puts the change back.
- `require_handshake: false` skips the handshake wait. Use it for the first
  side of a new pair, whose peer doesn't exist yet.
- A link of that name hostd didn't create is refused (`foreign_config`).
- A changed `.netdev` takes effect because hostd deletes its own link and
  networkd creates it again. Peers reconnect within a keepalive.

hostd's unit has two capabilities, both for this command:
- `CAP_CHOWN`, to give the key file to group `systemd-network`;
- `CAP_NET_ADMIN`, to read peer handshakes over WireGuard netlink and to
  delete its own WireGuard link.

## Wi-Fi

Wi-Fi goes through iwd. hostd never starts iwd: if it isn't running, the
Wi-Fi commands answer `no_iwd`.

```text
{"device": "wlan0"}                                         net.wifi.scan
{"device": "wlan0", "ssid": "home", "passphrase": "…"}      net.wifi.connect
{"device": "wlan0", "ssid": "cafe-open", "window_s": 30}    net.wifi.connect
```

- **`net.wifi.scan`** replies with each network's SSID, security
  (`open`, `psk`), signal in dBm, and whether iwd already knows it.
- **`net.wifi.connect`** writes no files.
  - **The passphrase never leaves hostd except to iwd.** It reaches iwd
    through an agent that hostd registers for that one connect. The agent
    answers only the process that owns iwd's bus name, and only for the
    network being connected. iwd saves the network itself, after a
    successful connect. Receipts and the audit never hold the passphrase.
  - **It passes** when the device is connected to that SSID within the
    window.
  - **If it doesn't,** hostd forgets the network (if this change made it
    known) and reconnects the network the device was on before, or
    disconnects if it was on none.
  - **A known network takes no passphrase.** To change one, forget it
    first. An open network takes none, and 802.1X networks are
    `unsupported`.
- **Undo** puts the device back the same way. It is refused (`superseded`)
  once the device has moved to another network since. If someone else
  moved the device during a change, the put-back leaves it alone.

## Stopping it

`hostd.freeze` stops every change at once and survives restarts and
reboots. An unreadable freeze record counts as frozen. Any local caller
may freeze or thaw hostd.

## Refusals

An unknown command gets the refusal every component gives: rc 10 with
`error_code` `UNKNOWN_COMMAND`, plus `message` and `command`. Every other
refusal names its reason in `status`:

| Status | Meaning |
|---|---|
| `bad_request` | the body doesn't fit the command's schema |
| `local_only` | a control or write command arrived from another node |
| `unstamped` | a Bus request has no valid broker origin stamp |
| `rate_limited` | the caller or host-wide write rate is used up |
| `frozen` | hostd is frozen |
| `foreign_config`, `unmanaged_configured`, `no_such_link` | `net.link.apply` or `net.wg.apply` won't touch this link |
| `no_iwd`, `no_such_station`, `no_such_network`, `unsupported` | Wi-Fi: iwd isn't running, the device isn't an iwd station, the SSID isn't in range, or the security type isn't handled |
| `rolled_back` | the change failed its health check and was reversed |
| `superseded` | undo refused: the file no longer holds what that change installed |
| `not_applied` | undo refused: the change was rolled back or already undone |
| `rollback_failed`, `undo_incomplete` | a rollback or undo couldn't finish; it is retried, and writes wait |
| `recovery_pending` | an unfinished change is being retried and hasn't finished yet; writes wait |
| `timeout`, `busy` | too slow, or too many requests at once |

## Source layout

The source is
[`services/hostd`](https://github.com/markc/mixos/tree/main/services/hostd):

| File | What |
|---|---|
| `src/local.rs` | the local socket: NUL-framed JSON, per-uid and per-connection limits |
| `src/caller.rs` | caller identity from `SO_PEERCRED`, a held pidfd, cgroup and namespaces, read per request |
| `src/service.rs` | the command table, tiers, dispatch and the write gate |
| `src/rate.rs` | sliding-window write limits per caller and command, plus a host cap |
| `src/freeze.rs` | the kill switch, failing closed |
| `src/receipt.rs` | receipts, durable atomic writes, restore |
| `src/txn.rs` | the transaction every write runs through: receipt first, conclude (keep or put back, durable outcome), recovery, undo |
| `src/link.rs` | `net.link.apply`: validation, rendering, the change, its proof |
| `src/wg.rs` | `net.wg.apply`: WireGuard netdev, network and key files, the handshake proof |
| `src/wgnl.rs` | WireGuard generic netlink (peer handshakes) and deleting a link |
| `src/wifi.rs` | `net.wifi.scan` and `net.wifi.connect` through iwd: the one-connect passphrase agent, runtime put-back |
| `src/networkd.rs` | networkd over D-Bus and Varlink, the event-driven health wait |
| `src/net.rs`, `src/info.rs` | the read commands |
| `units/` | `hostd.socket`, `hostd.service` (sandboxed) |

## Adding a write command

1. A typed request with `deny_unknown_fields`, validated into a plan before
   anything else happens. Anything the plan reads from the host (a key, a
   file) is read inside the writer lock.
2. `service::write(host, origin, peer, command, ...)`: it gates the call, takes the writer
   lock, gates it again and runs the change in its own task.
3. The change through `txn`: `FileChange::backup` for every file (with the
   mode and group a new file needs), `txn::begin`, the change and its proof,
   then `txn::conclude`. Never order the receipt by hand.
4. A `reapply` for the command in `txn::put_back`, so rollback, recovery and
   undo can make the host use the restored files. Without it the command
   doesn't ship.
5. The paths it writes added to `ReadWritePaths=`, and any capability it
   needs to `CapabilityBoundingSet=`, in `units/hostd.service`, with the
   reason beside it.
6. Registered in `docs/docs/spec/bus/commands.conf.mix` (class `intent`), in
   this page, and reviewed on its own.

## Tests

`cargo test -p hostd`: no test needs root. The live gate for network writes
runs as root on a disposable networkd host, on dummy links only, and
removes everything it made.
