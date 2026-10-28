---
title: Decision — non-root install, per-service users, mesh-first ports
description: MixOS installs and updates without root after a one-time bootstrap. Server daemons run as their own unprivileged users, listen on the WireGuard mesh by default, and get privileged ports from systemd rather than from file capabilities or root.
---

# Decision — non-root install, per-service users, mesh-first ports

Status: proposed 2026-10-12. Builds on
[one repository, install profiles](2026-10-12-one-repository-install-profiles.md)
and the one-system-binary design (every headless daemon a personality of
`mix`, chosen by `argv[0]`), which needs its own decision record.

## The question

The server programs (maild, webd, dnsd and the rest) are moving into MixOS,
headed for one `mix` binary. Updates already reach desktop users through
releasesd, which by rule never changes anything as root. Can a whole node,
servers included, be installed and kept current without root, and how do
unprivileged daemons serve ports 25, 53, 443, 465, 587 and 993?

## Where these servers run

The usual MixOS server is a node on its owner's own WireGuard mesh, a /24
of their machines. Mail, web and DNS are served to that mesh, not to the
internet. A server that must take connections from outside the mesh (an MX
on public port 25, a public website on 443) is the exception. This record
covers the mesh case and states what it does not settle for the public one.

## The decision

### 1. Root installs once; updates never need it

- **Bootstrap (root, once per node, and again only when units change):**
  create the system users, install the unit, socket, sysusers and tmpfiles
  files, and hand the server prefix to the `mixos` user.
- **The server prefix `/opt/mixos` is owned by the system user `mixos`.**
  A releasesd instance running as `mixos` keeps it current, with the same
  checks as a desktop install (GitHub digest and `SHA256SUMS`, staged
  unpack, previous version kept for rollback). It uses releasesd's existing
  layout through its environment: `MIXOS_RELEASES_ROOT=/opt/mixos`, so
  releases land in `/opt/mixos/opt/<app>/<version>` and their programs are
  linked into `/opt/mixos/bin`. The unit paths (`/opt/mixos/bin/<daemon>`)
  do not change.
- **Desktop installs stay per user** in `~/.local`, as today.
- Binaries are readable and executable by everyone and writable only by
  `mixos`. No daemon runs as `mixos`.

### 2. Every daemon runs as its own user

- Each server daemon runs as its own system user (`maild`, `webd`, `dnsd`
  and so on), declared in its component's sysusers fragment. Never as root,
  never as a login user, never as `mixos`.
- Each daemon's state lives in its own `StateDirectory=`, which no other
  service user can read. A compromised webd cannot read mail, keys or the
  login user's home.
- Units use systemd's sandboxing (`NoNewPrivileges=yes`,
  `ProtectSystem=strict`, `ProtectHome=yes`, `PrivateTmp=yes` and a
  `CapabilityBoundingSet=` that names only what the daemon needs), and each
  component's units are reviewed against that list.

### 3. Privileged ports come from systemd

- **Socket activation is the target.** A system `.socket` unit binds the
  privileged ports as root and passes the open sockets to the daemon
  (`LISTEN_FDS`). The daemon never holds a capability. Socket activation
  also keeps the listening socket open across a restart, so an upgrade
  drops no connections that are still queued.
- **Interim:** a daemon that cannot take inherited sockets yet gets
  `AmbientCapabilities=CAP_NET_BIND_SERVICE` with
  `CapabilityBoundingSet=CAP_NET_BIND_SERVICE` in its unit. Taking
  inherited sockets is part of each port's definition of done (maild,
  webd, dnsd).
- **maild status, 2026-10-28:** named descriptor adoption is implemented,
  but host-specific activation installation remains incomplete. Both service
  forms retain the interim capability so existing configured privileged binds
  survive upgrades. The socket templates have no active addresses and must
  be rendered from each host/instance's config and explicitly enabled. Only
  then may the documented service drop-in clear both capability settings.
  Descriptor adoption alone does not complete the port migration.
- **Not file capabilities.** The kernel clears them whenever the file is
  replaced, so every update would silently lose the ports. In the one
  binary they would also give the `mix` shell, and every script, the right
  to bind 443.
- **Not lowering `net.ipv4.ip_unprivileged_port_start` by default.** It
  applies to every user on the node. A node owner may still choose it for a
  single-user machine, but MixOS does not do it for them.

### 4. Mesh first

- Server daemons listen on the node's mesh address by default, never on
  every address. The `.socket` units name the mesh address
  (`ListenStream=<mesh-ip>:25`, with `FreeBind=yes` so the socket can be
  bound before the WireGuard interface is up).
- Listening on a public address is an explicit per-node configuration,
  never a default and never part of an install profile.

### 5. Applying an update without root

- releasesd (as `mixos`) moves `current` and records the transition in
  `meta.json`. A root-owned systemd `.path` unit watches that file and
  starts a root service that restarts only the units whose component
  version or hash changed in the release manifest. The `mixos` user
  triggers the restart but never controls which units restart or how.
- A release whose units, sysusers or tmpfiles fragments changed cannot be
  applied by `mixos`. releasesd installs the binaries, reports that the
  bootstrap step is needed, and leaves the running services on the
  previous version until a root user runs the installer.

### 6. What still needs root at run time

- **nspawnd:** `systemd-nspawn` needs root. It stays a root service in its
  own profile, outside this model.
- **noded:** reading network state needs nothing; changing it needs
  `CAP_NET_ADMIN`, granted in its unit the same way as the interim port
  capability, only if that node uses the feature.
- **compd:** none. Seat, input and display access come through logind and
  libseat, as today.

## Not settled: servers open to the internet

A daemon that accepts connections from outside the mesh (an MX on public
port 25, a public website on 443) faces hostile traffic. Running it as its
own sandboxed user, with the port granted by systemd, is still the baseline.
It is not shown to be enough: rate limits, DoS behaviour, TLS and ACME
exposure, whether the public listener should be its own process or node, and
firewall policy all belong to a separate decision before any public
listener ships as a supported configuration.

## Why

- **Updates without root** are what make the releasesd model work for a
  whole operating system: no package manager, no distribution repository,
  and no root shell for routine updates.
- **Per-service users** limit what one compromised daemon can reach. Running
  every daemon as the login user would have been simpler, but would have
  handed a mail-server bug the owner's SSH keys and secrets.
- **Ports from systemd** grant the right per service and per port, survive
  updates, and keep capabilities away from the shell that shares the
  binary.
- **Mesh first** matches how MixOS servers are used, and keeps the
  internet-facing case visible as the exception it is.

## Alternatives

- **Everything as the login user** (user units with lingering, plus the
  sysctl). Rejected: no isolation between daemons or from the owner's home.
- **File capabilities on `mix`.** Rejected: lost on every update, and
  granted to every script.
- **Root installs through a package manager.** Rejected: it is the
  dependency that releasesd removes.
- **Root-run releasesd for the server prefix.** Rejected: releasesd's trust
  rule (never act as root) is what makes a bad release unable to take over
  the node. A fixed `.path` unit for restarts keeps root's part small.

## Consequences

- Each server component ships a sysusers fragment, its units with the
  sandboxing list, and `.socket` units for its ports. The install-profile
  resolver lists them, as that decision already requires.
- maild, webd and dnsd learn to take inherited sockets as part of their
  ports.
- releasesd gains a server instance (`mixos` user, `/opt/mixos` root) and a
  "units changed" result. The restart service and its `.path` unit are new
  work, alongside the installer the install-profiles decision calls for.
- The bootstrap installer is the only routine root step, and it runs again
  only when a release changes units.
