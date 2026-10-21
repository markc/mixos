# noded

The native ABP broker: service discovery, routing, subscriptions and authenticated local session grants.

Documentation: <https://mixos.dev/noded/> — source [`docs/noded/README.md`](../../docs/noded/README.md).

## Offline broker principal preparation

`scripts/broker_profile.mix` is an opt-in module for a fresh, explicitly marked
offline rootfs. It never calls a service manager or changes the running system.
`plan(spec)` describes a separate desktop client and broker UID/GID, protected
profile, native Unix endpoint, public client config, dedicated broker environment
and a per-instance `noded@N.service` override. `prepare(root, spec)` requires
namespace root, a canonical root-owned rootfs and an inode-bound
`.broker-rootfs.json` marker. It refuses `/`, existing broker profiles, account
collisions, mismatched desktop accounts and symlink paths. It creates the broker
only in that rootfs's passwd/group/shadow databases. Desktop state is never
recursively chowned. Existing system units and single-account desks are unchanged.

The override retains the existing unit/dependency name, resets inherited desktop
environment files and supplementary groups, and selects the private broker
account, runtime and environment. Client config/environment is separate and
does not change application VAR or the desktop runtime. Preparation creates no
authority baseline, seed or trust anchor. Its receipt starts incomplete and
becomes prepared only after files are synced. No production image consumer or
mesh distributor is enabled by this module; activation still requires effective
unit verification, signed authority and connection identity gates.

`tests/broker_namespace_test.mix` uses real subordinate UIDs in private user,
mount, PID and network namespaces, with installed native Mix and a candidate
noded copied into a disposable chroot. It preserves the enclosing container's
masked proc mounts. Disposable signing material stays in its temporary root and
is removed. It checks a signed authority query as the desktop UID, repeats the
query after isolated TCP becomes unreachable, verifies private witness/lock
ownership and mode0600, requires actual desktop write denials, and checks parent
mount/loopback state and temporary-root cleanup. It does not boot systemd or
claim the separately owned server-PID pin was tested.

Run through native Mix; `MIXOS_TEST_NODED` may select a CBC-built candidate file
for this fixture only. It is copied and hash-checked, never installed to gold.
The candidate must include the explicit private witness/lock mode invariant.
The fixture needs Linux subordinate UID/GID mappings and native `unshare`,
`chroot`, `mount`, `ip`, `readelf`, `ldconfig` and `pidwait` facilities; unsupported
namespace policy is a fixture environment failure, not a noded regression.
