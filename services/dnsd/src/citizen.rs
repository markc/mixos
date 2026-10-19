// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Daemon identity reporting and WireGuard-versus-LAN bind validation.
//! Sysusers and systemd enforce UID/GID 506; the runtime check is advisory.
//! Missing node identity retains the standalone explicit-flag bind policy.

use std::net::{IpAddr, SocketAddr};

/// SPEC-10 v1.4.8 registered daemon-identity facts for `dnsd`.
/// These MUST track SPEC-10 Appendix A / §2.2 / §9.3 exactly. Drift is
/// caught two ways: the registry-generic `spec10_postcheck.mix` lint
/// out of process, and the unit test below cross-checking these
/// constants against the checked-in `services/dnsd/units/dnsd.sysusers.conf`
/// projection (so a registry renumber/rename trips `cargo test`, not
/// only the Mix gate).
const SPEC10_VERSION: &str = "1.4.8";
const DAEMON_NAME: &str = "dnsd";
const DAEMON_UID: u32 = 506;
/// SPEC-10 §2.2: every daemon-identity row has GID == UID.
const DAEMON_GID: u32 = 506;
/// The service and its Bus registration use the same plain name.
pub(crate) const BUS_SERVICE: &str = "dnsd";

/// Emit the SPEC-10 daemon-identity legibility surface at startup:
/// one structured identity log line plus an advisory euid/egid
/// self-report. Never refuses — enforcement is sysusers + systemd
/// `User=` + the registry-generic `spec10_postcheck.mix` (the
/// maild/webd posture).
pub fn report_spec10_identity() {
    tracing::info!(
        mixos.spec = 10,
        mixos.spec.version = SPEC10_VERSION,
        mixos.daemon = DAEMON_NAME,
        mixos.uid = DAEMON_UID,
        mixos.gid = DAEMON_GID,
        mixos.bus_service = BUS_SERVICE,
        "SPEC-10 daemon identity (citizen build): enforced by sysusers + systemd User= + registry-generic spec10 postcheck; this line is the legibility surface"
    );

    match effective_ids() {
        Some((euid, egid)) => {
            if euid != DAEMON_UID || egid != DAEMON_GID {
                tracing::warn!(
                    running.euid = euid,
                    running.egid = egid,
                    expected.uid = DAEMON_UID,
                    expected.gid = DAEMON_GID,
                    "running euid/egid does NOT match the SPEC-10 dnsd slot (506/506) — ADVISORY only, not refusing (systemd User= is the enforcement point, matching the maild/webd posture)"
                );
            } else {
                tracing::info!(
                    running.euid = euid,
                    running.egid = egid,
                    "effective uid/gid match the SPEC-10 dnsd slot"
                );
            }
        }
        None => {
            tracing::warn!(
                "could not read /proc/self/status for the advisory euid/egid self-report (non-fatal — advisory surface only)"
            );
        }
    }
}

/// Read effective uid/gid from `/proc/self/status`. The `Uid:` /
/// `Gid:` lines are tab-separated `real effective saved fs`; field
/// index 1 (0-based, after the label) is the *effective* id — the one
/// that matters for "what privileges am I running with". Linux-only,
/// zero new dependency (the same `/proc/self/status` parse the SPEC-18
/// accept harness uses). Returns `None` on any read/parse miss — this
/// is an advisory surface, so a miss is a soft WARN, never a hard
/// failure.
fn effective_ids() -> Option<(u32, u32)> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let mut euid = None;
    let mut egid = None;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("Uid:") {
            euid = rest.split_whitespace().nth(1).and_then(|v| v.parse().ok());
        } else if let Some(rest) = line.strip_prefix("Gid:") {
            egid = rest.split_whitespace().nth(1).and_then(|v| v.parse().ok());
        }
    }
    Some((euid?, egid?))
}

/// Is `ip` a plausible WireGuard *mesh unicast* address — i.e. usable
/// as this node's own identity? Rejects the classes that can never be
/// a node's WG IP and would be dangerous to treat as one:
///
///   * **unspecified** (`0.0.0.0` / `::`) — the load-bearing guard: a
///     `node.conf.mix` typo here would otherwise flow through
///     [`citizen_check_bind`]'s `ip == wg` arm and *allow a wildcard
///     bind*, the exact thing P1's `check_bind` hard-forbids with no
///     override (plan §6 "never `0.0.0.0`");
///   * **loopback** — would silently turn the daemon into a
///     localhost-only server while still *claiming* WG-vs-LAN was
///     validated; honest behaviour is to degrade to the logged P1
///     posture instead;
///   * **multicast** / IPv4 **broadcast** — never a unicast node
///     identity.
///
/// Deliberately NOT a mesh-CIDR membership check: `dnsd` has no
/// canonical WG CIDR in its contract (the mesh range is
/// node.conf.mix/deployment provenance — the same class of concern §6
/// defers to P2-D), so constraining to a specific `/24` here would
/// invent a contract the plan does not give and false-reject valid
/// meshes on other ranges.
/// Canonicalise an IPv4-mapped IPv6 address (`::ffff:a.b.c.d`) to its
/// embedded `IpAddr::V4`, leaving every other address untouched. Run
/// before BOTH the [`is_usable_wg_ip`] class checks and the
/// [`citizen_check_bind`] `ip == wg` equality so a mapped form cannot
/// (a) smuggle a wildcard/loopback/multicast/broadcast past the
/// IPv4-only class predicates (`::ffff:0.0.0.0` is `IpAddr::V6`, so
/// `is_unspecified()` is false on it) nor (b) spuriously mismatch the
/// same address written in the other family (`::ffff:192.0.2.5` vs
/// `192.0.2.5`). Deliberately `to_ipv4_mapped` — *only* the `::ffff:`
/// block — NOT `to_ipv4`, because the latter also rewrites `::`/`::1`
/// and would misclassify real IPv6 unspecified/loopback as v4.
fn canonical_ip(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => IpAddr::V4(v4),
            None => IpAddr::V6(v6),
        },
        v4 => v4,
    }
}

/// Class check for "could this be a node's WG unicast identity".
/// Canonicalises first (see [`canonical_ip`]) so IPv4-mapped IPv6
/// degenerates are classified by their embedded v4 address.
fn is_usable_wg_ip(ip: IpAddr) -> bool {
    let ip = canonical_ip(ip);
    if ip.is_unspecified() || ip.is_loopback() || ip.is_multicast() {
        return false;
    }
    if matches!(ip, IpAddr::V4(v4) if v4.is_broadcast()) {
        return false;
    }
    true
}

/// Resolve this node's WireGuard mesh IP from `node.conf.mix`
/// (`config::node::load_node_config` — the standard
/// env/XDG/`/etc` search order). This is P2-B's first real use of the
/// `config` dep the seam staged in P2-A.
///
/// Returns `None` (with a single explanatory WARN) when there is no
/// node config, the load fails, `wg_ip` is not a valid IP, **or it
/// parses but is not a usable mesh unicast address** (see
/// [`is_usable_wg_ip`] — this last case is the load-bearing one: a
/// `node.conf.mix` typo of `wg_ip = "0.0.0.0"` must NOT be allowed to
/// re-open, via the `ip == wg` arm, the wildcard bind P1 hard-forbids).
/// `None` is *not* fatal: it means "cannot trust a WG IP, so cannot
/// discriminate WG from LAN", which [`citizen_check_bind`] handles by
/// degrading to the exact P1 mechanical posture rather than refusing
/// to start (loopback `dig` debugging must keep working; and P1 still
/// hard-rejects the wildcard, so the degraded path is strictly safe).
/// **Postcondition:** every `Some(ip)` returned is *canonical* (any
/// IPv4-mapped IPv6 already folded to its v4) and non-unspecified,
/// non-loopback, non-multicast, non-broadcast — [`citizen_check_bind`]
/// relies on this. The WARN is emitted here exactly once per process,
/// not per `--listen`.
pub fn configured_wg_ip() -> Option<IpAddr> {
    match config::node::load_node_config() {
        Ok(Some(nc)) => match nc.wg_ip.parse::<IpAddr>() {
            Ok(parsed) => {
                // Store the CANONICAL form so the postcondition holds
                // and `citizen_check_bind`'s `ip == wg` equality is
                // family-consistent with a canonicalised listen addr.
                let ip = canonical_ip(parsed);
                if is_usable_wg_ip(ip) {
                    Some(ip)
                } else {
                    tracing::warn!(
                        wg_ip = %nc.wg_ip,
                        %ip,
                        "node.conf.mix wg_ip parses but is not a usable WG mesh unicast address (unspecified/loopback/multicast/broadcast, after IPv4-mapped canonicalisation) — WG-vs-LAN bind validation degraded to the P1 explicit-flag posture; P1 still hard-rejects the wildcard (plan §6)"
                    );
                    None
                }
            }
            Err(e) => {
                tracing::warn!(
                    wg_ip = %nc.wg_ip,
                    %e,
                    "node.conf.mix wg_ip is not a valid IP — WG-vs-LAN bind validation degraded to the P1 explicit-flag posture (plan §6)"
                );
                None
            }
        },
        Ok(None) => {
            tracing::warn!(
                "no node.conf.mix found — the dnsd citizen cannot validate WG-vs-LAN; bind validation degraded to the P1 explicit-flag posture (--allow-non-loopback-listen + WARN, plan §6)"
            );
            None
        }
        Err(e) => {
            tracing::warn!(
                %e,
                "node.conf.mix load failed — WG-vs-LAN bind validation degraded to the P1 explicit-flag posture (plan §6)"
            );
            None
        }
    }
}

/// Citizen-aware bind validation (plan §6). Layered on the P1
/// mechanical [`crate::check_bind`], it does NOT loosen anything:
///
///   * `wg_ip = Some(wg)` (node.conf.mix present): bind is accepted **only**
///     if it is loopback (local `dig` debugging) or exactly `wg`. A
///     LAN IP, a wildcard, or any other address is rejected, and
///     `--allow-non-loopback-listen` no longer overrides this — the
///     WG-IP match is the gate now, which is the whole point of the
///     citizen having config P1 lacked. `wg` is guaranteed by
///     [`configured_wg_ip`]'s postcondition to be a *canonical* usable
///     unicast address (never unspecified/loopback/multicast/broadcast,
///     IPv4-mapped IPv6 already folded to v4), and the listen addr is
///     canonicalised the same way, so the `ip == wg` arm cannot
///     re-admit the wildcard P1 forbids nor mismatch on family.
///   * `wg_ip = None` (no/invalid node.conf.mix): the citizen cannot tell
///     WG from LAN, so it falls back to the *exact* P1 mechanical
///     [`crate::check_bind`] (wildcard always rejected, loopback always
///     allowed, other non-loopback gated on the explicit flag + WARN).
///     The reason was already WARN-logged once by
///     [`configured_wg_ip`]; this path stays behaviour-identical to P1.
pub fn citizen_check_bind(
    sa: &SocketAddr,
    allow_non_loopback: bool,
    wg_ip: Option<IpAddr>,
) -> Result<(), String> {
    let Some(wg) = wg_ip else {
        // No usable node.conf.mix: degrade to the exact P1 posture. This
        // is also the only reference to `crate::check_bind` in the
        // citizen build, so it stays live (no dead_code) here.
        return crate::check_bind(sa, allow_non_loopback);
    };

    // Canonicalise an IPv4-mapped IPv6 listen addr so it is classified
    // and compared by its embedded v4 (matches the canonical `wg`
    // stored by `configured_wg_ip`): a `[::ffff:127.0.0.1]:53` is
    // loopback, and `[::ffff:192.0.2.5]:53` matches a v4 wg_ip.
    let ip = canonical_ip(sa.ip());
    if ip.is_loopback() {
        // Loopback is always fine — `dig @127.0.0.1` debugging, and
        // no conflict with systemd-resolved's 127.0.0.53:53 stub.
        return Ok(());
    }
    if ip == wg {
        tracing::info!(
            %sa,
            %wg,
            "citizen bind = the configured WG mesh IP (node.conf.mix wg_ip); WG-vs-LAN validated (plan §6)"
        );
        return Ok(());
    }
    Err(format!(
        "refusing bind {sa}: the dnsd citizen binds the configured WG mesh IP ({wg}, from node.conf.mix) or loopback only; \
         a LAN / wildcard / other address is rejected and no flag overrides this in the citizen-with-config path (plan §6 WG-vs-LAN)"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Run-time manifest directory rather than the `env!`-baked one: cargo
    /// exports `CARGO_MANIFEST_DIR` into the test process, and that names
    /// the tree cargo is actually running in, whereas `env!` records
    /// whichever tree last *compiled* the binary. The two diverge when one
    /// `CARGO_TARGET_DIR` is shared across several git worktrees of this
    /// repo. Falls back to the compile-time value when run outside cargo.
    fn manifest_dir() -> std::path::PathBuf {
        std::env::var_os("CARGO_MANIFEST_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")))
    }

    #[test]
    fn spec10_identity_matches_checked_in_sysusers_fragment() {
        // Non-circular: cross-check the in-code SPEC-10 facts against
        // the checked-in `services/dnsd/units/dnsd.sysusers.conf` projection of
        // Appendix A (the same fragment `spec10_postcheck.mix`
        // validates). A registry edit that renumbers or renames
        // dnsd trips THIS unit test, not just the out-of-process
        // Mix lint. Asserting a const against its own literal would be
        // tautological; this asserts it against an external artifact.
        let conf = std::fs::read_to_string(manifest_dir().join("units/dnsd.sysusers.conf"))
            .expect("checked-in sysusers fragment must be readable from the workspace");

        // Header line pins the registry version the fragment was
        // generated from: `... daemon-identity v1.4.8.`
        let version = conf
            .lines()
            .find_map(|l| l.split("daemon-identity v").nth(1))
            .map(|v| v.trim().trim_end_matches('.'))
            .expect("sysusers header must name the daemon-identity version");
        assert_eq!(
            version, SPEC10_VERSION,
            "sysusers fragment was generated from a different SPEC-10 version than the daemon pins"
        );

        // The `u dnsd <uid> "GECOS ..." ...` row. The GECOS is
        // quoted and contains spaces, but type/name/uid are the first
        // three whitespace tokens (before the quote), so reading them
        // positionally is safe.
        let row = conf
            .lines()
            .map(|l| l.split_whitespace().collect::<Vec<_>>())
            .find(|f| f.first() == Some(&"u") && f.get(1) == Some(&DAEMON_NAME))
            .expect("sysusers fragment must carry a `u dnsd` daemon-identity row");
        let uid: u32 = row[2]
            .parse()
            .expect("sysusers dnsd uid column must be numeric");
        assert_eq!(
            uid, DAEMON_UID,
            "sysusers fragment assigns dnsd a different UID than the daemon pins"
        );

        // SPEC-10 §2.2: daemon-identity rows are same-numbered (GID ==
        // UID) — the sysusers `u` line carries a single numeric column
        // for exactly that reason. This guards the in-code invariant,
        // not a literal, so it is not circular.
        assert_eq!(DAEMON_GID, DAEMON_UID, "SPEC-10 §2.2: GID == UID");
        let group = conf
            .lines()
            .map(|l| l.split_whitespace().collect::<Vec<_>>())
            .find(|f| f.first() == Some(&"g") && f.get(1) == Some(&DAEMON_NAME))
            .expect("sysusers fragment must carry the dnsd group row");
        assert_eq!(group[2].parse::<u32>().unwrap(), DAEMON_GID);

        // The renamed account, daemon and Bus service share one identity.
        assert_eq!(
            DAEMON_NAME, BUS_SERVICE,
            "Bus service name must match the daemon identity"
        );
    }

    #[test]
    fn effective_ids_reads_self() {
        // /proc/self/status exists on the Linux test host; the parse
        // must yield some (euid, egid) pair. Values are not asserted
        // (they are whatever the test runner runs as) — this guards
        // the parser shape, not the identity.
        let ids = effective_ids();
        assert!(
            ids.is_some(),
            "expected to parse /proc/self/status Uid:/Gid:"
        );
    }

    fn sa(s: &str) -> SocketAddr {
        s.parse().expect("test socket addr literal must parse")
    }

    fn ip(s: &str) -> IpAddr {
        s.parse().expect("test ip literal must parse")
    }

    #[test]
    fn is_usable_wg_ip_rejects_degenerate_classes() {
        // Usable mesh unicast addresses (v4 + v6 ULA).
        assert!(is_usable_wg_ip(ip("192.0.2.5")));
        assert!(is_usable_wg_ip(ip("10.0.0.1")));
        assert!(is_usable_wg_ip(ip("fd00::1")));

        // The load-bearing guard: an unspecified wg_ip must NOT be
        // usable, else the `ip == wg` arm re-opens the wildcard bind
        // P1 hard-forbids (the Codex BLOCKER this closes).
        assert!(!is_usable_wg_ip(ip("0.0.0.0")));
        assert!(!is_usable_wg_ip(ip("::")));
        // Loopback / multicast / broadcast are never a node identity.
        assert!(!is_usable_wg_ip(ip("127.0.0.1")));
        assert!(!is_usable_wg_ip(ip("::1")));
        assert!(!is_usable_wg_ip(ip("224.0.0.1")));
        assert!(!is_usable_wg_ip(ip("ff02::1")));
        assert!(!is_usable_wg_ip(ip("255.255.255.255")));

        // IPv4-mapped IPv6 (`::ffff:a.b.c.d`) must be canonicalised to
        // its embedded v4 BEFORE classification, else a degenerate v4
        // smuggled in mapped form (`IpAddr::V6`, so `is_unspecified()`
        // etc. are false on the raw value) would bypass the class
        // checks — the Codex round-2 MAJOR this closes.
        assert!(!is_usable_wg_ip(ip("::ffff:0.0.0.0")));
        assert!(!is_usable_wg_ip(ip("::ffff:127.0.0.1")));
        assert!(!is_usable_wg_ip(ip("::ffff:224.0.0.1")));
        assert!(!is_usable_wg_ip(ip("::ffff:255.255.255.255")));
        // …and a usable v4 written in mapped form is still usable
        // (canonicalisation folds it to the same v4 unicast).
        assert!(is_usable_wg_ip(ip("::ffff:192.0.2.5")));
    }

    #[test]
    fn citizen_bind_accepts_wg_ip_and_loopback_rejects_lan_and_wildcard() {
        let wg: IpAddr = "192.0.2.5".parse().unwrap();

        // The configured WG mesh IP — accepted, flag irrelevant.
        assert!(citizen_check_bind(&sa("192.0.2.5:53"), false, Some(wg)).is_ok());
        // Loopback — always accepted (local `dig` debugging).
        assert!(citizen_check_bind(&sa("127.0.0.1:53"), false, Some(wg)).is_ok());
        assert!(citizen_check_bind(&sa("[::1]:53"), false, Some(wg)).is_ok());

        // A LAN IP is rejected even WITH the P1 flag set — in the
        // citizen-with-config path the WG-IP match is the gate, the
        // flag no longer overrides (plan §6, the P2-B supersession).
        assert!(citizen_check_bind(&sa("198.51.100.10:53"), true, Some(wg)).is_err());
        // Wildcard is rejected (it is neither loopback nor == wg).
        assert!(citizen_check_bind(&sa("0.0.0.0:53"), true, Some(wg)).is_err());
        assert!(citizen_check_bind(&sa("[::]:53"), true, Some(wg)).is_err());

        // IPv4-mapped IPv6 listen addrs are canonicalised the same way
        // as the stored `wg`, so the family written on the CLI does not
        // change the verdict (Codex round-2 MAJOR): the mapped WG IP
        // matches, mapped loopback is still loopback, and a mapped
        // wildcard is still rejected — no `::ffff:` bypass.
        assert!(citizen_check_bind(&sa("[::ffff:192.0.2.5]:53"), false, Some(wg)).is_ok());
        assert!(citizen_check_bind(&sa("[::ffff:127.0.0.1]:53"), false, Some(wg)).is_ok());
        assert!(citizen_check_bind(&sa("[::ffff:0.0.0.0]:53"), true, Some(wg)).is_err());
        assert!(citizen_check_bind(&sa("[::ffff:198.51.100.10]:53"), true, Some(wg)).is_err());
    }

    #[test]
    fn citizen_bind_without_node_config_is_exactly_the_p1_posture() {
        // wg_ip = None ⇒ delegate to the P1 mechanical check_bind:
        // wildcard always rejected, loopback always allowed, other
        // non-loopback gated on the explicit flag (P1 parity, not a
        // citizen loosening).
        assert!(citizen_check_bind(&sa("0.0.0.0:53"), true, None).is_err());
        assert!(citizen_check_bind(&sa("127.0.0.1:53"), false, None).is_ok());
        assert!(citizen_check_bind(&sa("192.0.2.5:53"), false, None).is_err());
        assert!(citizen_check_bind(&sa("192.0.2.5:53"), true, None).is_ok());
    }
}
