// SPDX-License-Identifier: MIT OR Apache-2.0
//! Standing grants: who may call which write verbs, within which limits,
//! until when. Default deny. The policy is a strict-data document
//! (`/etc/hostd/policy.mx`), root-owned and not writable by anyone else,
//! read again for every write request so an edit takes effect at once.
//!
//! ```text
//! {
//!   schema: 1,
//!   grants: [
//!     {name: "desk-network",
//!      caller: {cgroup: "/machine.slice/systemd-nspawn@desk.service/*"},
//!      verbs: ["net.*"],
//!      limits: {links: ["eth0"], ssids: ["example"]},
//!      expires: "2027-01-01T00:00:00Z",
//!      rate: {count: 3, per_s: 600}}
//!   ]
//! }
//! ```
use crate::caller::Caller;
use anyhow::{Context, bail};
use serde::Deserialize;
use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    os::unix::fs::MetadataExt,
    path::Path,
    time::{Duration, Instant},
};

pub const POLICY: &str = "/etc/hostd/policy.mx";
const SCHEMA: u32 = 1;
/// Verbs no grant may name: thawing is for local root only.
const UNGRANTABLE: &[&str] = &["hostd.thaw"];

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    pub schema: u32,
    #[serde(default)]
    pub grants: Vec<Grant>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Grant {
    pub name: String,
    pub caller: CallerMatch,
    /// Exact verb names, or a family as `prefix.*`.
    pub verbs: Vec<String>,
    /// Named allow-lists a verb checks its arguments against (`links`,
    /// `ssids`, …). A key the grant doesn't list is unrestricted.
    #[serde(default)]
    pub limits: BTreeMap<String, Vec<String>>,
    /// RFC 3339. A grant past its expiry matches nothing.
    #[serde(default)]
    pub expires: Option<String>,
    #[serde(default)]
    pub rate: Option<Rate>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CallerMatch {
    #[serde(default)]
    pub uid: Option<u32>,
    /// An exact cgroup path, or a subtree as `path/*`. Name a subtree the
    /// caller can't move itself into, such as one under `machine.slice`.
    pub cgroup: String,
}

#[derive(Debug, Deserialize, Clone, Copy)]
#[serde(deny_unknown_fields)]
pub struct Rate {
    pub count: u32,
    pub per_s: u64,
}

/// Why a request was refused, for the reply and the audit record.
#[derive(Debug, PartialEq, Eq)]
pub enum Refusal {
    Unverified,
    NoGrant,
    RateLimited { grant: String },
}

impl Refusal {
    pub fn status(&self) -> &'static str {
        match self {
            Self::Unverified => "caller_unverified",
            Self::NoGrant => "no_grant",
            Self::RateLimited { .. } => "rate_limited",
        }
    }
}

impl Policy {
    /// Loads and validates the policy. A missing file is an empty policy.
    ///
    /// With `require_root`, the file is opened once, without following a
    /// symlink, and its ownership, mode and type are checked on that open
    /// file, whose bytes are then parsed: nothing can swap the file between
    /// the check and the read. Every directory above it must be root's and
    /// writable by root only, so nobody else can replace it either.
    pub fn load(path: &Path, require_root: bool) -> anyhow::Result<Self> {
        use std::{io::Read, os::unix::fs::OpenOptionsExt};
        let name = || path.display().to_string();
        let mut file = match std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self {
                    schema: SCHEMA,
                    grants: vec![],
                });
            }
            Err(error) => return Err(error).with_context(name),
        };
        let meta = file.metadata().with_context(name)?;
        if !meta.is_file() {
            bail!("{} is not a regular file", path.display());
        }
        if require_root {
            if meta.uid() != 0 || meta.mode() & 0o022 != 0 {
                bail!(
                    "{} must be owned by root and writable by root only",
                    path.display()
                );
            }
            for dir in path.ancestors().skip(1) {
                let meta =
                    std::fs::symlink_metadata(dir).with_context(|| dir.display().to_string())?;
                if !meta.is_dir() || meta.uid() != 0 || meta.mode() & 0o022 != 0 {
                    bail!(
                        "{} must be a root-owned directory writable by root only",
                        dir.display()
                    );
                }
            }
        }
        let mut text = String::new();
        (&mut file)
            .take(1024 * 1024)
            .read_to_string(&mut text)
            .with_context(name)?;
        let policy: Self = strict::from_str(&text).with_context(name)?;
        policy.validate()?;
        Ok(policy)
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        if self.schema != SCHEMA {
            bail!("policy schema {} (this hostd reads {SCHEMA})", self.schema);
        }
        let mut names = std::collections::HashSet::new();
        for grant in &self.grants {
            if !names.insert(&grant.name) {
                bail!("grant {:?} appears twice", grant.name);
            }
            if !grant.caller.cgroup.starts_with('/') {
                bail!("grant {:?}: cgroup must be an absolute path", grant.name);
            }
            if grant.verbs.is_empty() {
                bail!("grant {:?} names no verbs", grant.name);
            }
            for verb in &grant.verbs {
                let family = verb.strip_suffix(".*");
                let bare = family.unwrap_or(verb);
                if bare.is_empty() || bare.contains('*') || bare.split('.').any(str::is_empty) {
                    bail!(
                        "grant {:?}: {verb:?} is not a verb or a `prefix.*` family",
                        grant.name
                    );
                }
                if UNGRANTABLE.iter().any(|u| covers(verb, u)) {
                    bail!(
                        "grant {:?}: {verb:?} would grant a root-only verb",
                        grant.name
                    );
                }
            }
            if let Some(expires) = &grant.expires {
                chrono::DateTime::parse_from_rfc3339(expires)
                    .with_context(|| format!("grant {:?}: expires {expires:?}", grant.name))?;
            }
            if let Some(rate) = grant.rate
                && (rate.count == 0 || rate.per_s == 0)
            {
                bail!("grant {:?}: rate needs a count and a period", grant.name);
            }
        }
        Ok(())
    }

    /// The first unexpired grant that matches this caller and verb.
    pub fn grant_for(
        &self,
        caller: &Caller,
        verb: &str,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<&Grant, Refusal> {
        if !caller.verified {
            return Err(Refusal::Unverified);
        }
        self.grants
            .iter()
            .find(|g| g.matches(caller) && g.covers(verb) && !g.expired(now))
            .ok_or(Refusal::NoGrant)
    }

    /// Every unexpired grant that matches this caller and verb, in policy
    /// order, so the caller can pick the first whose allow-lists fit.
    pub fn grants_for(
        &self,
        caller: &Caller,
        verb: &str,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<Vec<&Grant>, Refusal> {
        if !caller.verified {
            return Err(Refusal::Unverified);
        }
        let found: Vec<&Grant> = self
            .grants
            .iter()
            .filter(|g| g.matches(caller) && g.covers(verb) && !g.expired(now))
            .collect();
        if found.is_empty() {
            Err(Refusal::NoGrant)
        } else {
            Ok(found)
        }
    }

    /// Names of the unexpired grants this caller holds, for `hostd.status`.
    pub fn held_by(&self, caller: &Caller, now: chrono::DateTime<chrono::Utc>) -> Vec<&str> {
        if !caller.verified {
            return vec![];
        }
        self.grants
            .iter()
            .filter(|g| g.matches(caller) && !g.expired(now))
            .map(|g| g.name.as_str())
            .collect()
    }
}

impl Grant {
    fn matches(&self, caller: &Caller) -> bool {
        let Some(cgroup) = caller.cgroup.as_deref() else {
            return false;
        };
        // `X/*` is X itself and everything below it: a unit's own cgroup and
        // the sub-cgroups it creates.
        self.caller.uid.is_none_or(|uid| uid == caller.uid)
            && match self.caller.cgroup.strip_suffix("/*") {
                Some(subtree) => cgroup
                    .strip_prefix(subtree)
                    .is_some_and(|rest| rest.is_empty() || rest.starts_with('/')),
                None => cgroup == self.caller.cgroup,
            }
    }

    pub fn covers(&self, verb: &str) -> bool {
        self.verbs.iter().any(|pattern| covers(pattern, verb))
    }

    fn expired(&self, now: chrono::DateTime<chrono::Utc>) -> bool {
        self.expires
            .as_deref()
            .and_then(|e| chrono::DateTime::parse_from_rfc3339(e).ok())
            .is_some_and(|e| now >= e)
    }

    /// Whether `value` is inside the grant's `key` allow-list.
    pub fn allows(&self, key: &str, value: &str) -> bool {
        self.limits
            .get(key)
            .is_none_or(|list| list.iter().any(|v| v == value))
    }
}

fn covers(pattern: &str, verb: &str) -> bool {
    match pattern.strip_suffix(".*") {
        Some(family) => verb
            .strip_prefix(family)
            .is_some_and(|rest| rest.starts_with('.')),
        None => pattern == verb,
    }
}

/// How many callers' worth of calls a whole grant may make: one caller can't
/// use up a shared grant, and many callers (or sub-cgroups made to dodge
/// the per-caller limit) can't multiply it without bound.
const GRANT_SHARE: u32 = 4;

/// Calls per grant, verb and caller inside a sliding window, plus a
/// grant-wide cap. Held in memory: a restart forgets the window, which the
/// deadlines, rollback and circuit breaker keep safe.
#[derive(Default)]
pub struct RateLimiter {
    seen: HashMap<(String, String, String), VecDeque<Instant>>,
}

impl RateLimiter {
    /// Records the call and says whether it is within the grant's rate.
    /// `who` identifies the caller: its verified cgroup.
    pub fn admit(
        &mut self,
        grant: &Grant,
        verb: &str,
        who: &str,
        now: Instant,
    ) -> Result<(), Refusal> {
        let Some(rate) = grant.rate else {
            return Ok(());
        };
        let window = Duration::from_secs(rate.per_s);
        let refused = || Refusal::RateLimited {
            grant: grant.name.clone(),
        };
        let caller_key = (grant.name.clone(), verb.to_owned(), who.to_owned());
        // The empty caller key holds the grant-wide window.
        let grant_key = (grant.name.clone(), verb.to_owned(), String::new());
        for (key, limit) in [
            (&caller_key, rate.count),
            (&grant_key, rate.count.saturating_mul(GRANT_SHARE)),
        ] {
            let calls = self.seen.entry(key.clone()).or_default();
            while calls
                .front()
                .is_some_and(|t| now.duration_since(*t) >= window)
            {
                calls.pop_front();
            }
            if calls.len() >= limit as usize {
                return Err(refused());
            }
        }
        for key in [caller_key, grant_key] {
            self.seen.entry(key).or_default().push_back(now);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caller(cgroup: &str, uid: u32, verified: bool) -> Caller {
        Caller {
            uid,
            gid: uid,
            pid: Some(1),
            cgroup: Some(cgroup.into()),
            verified,
            host_ns: false,
        }
    }

    fn policy(text: &str) -> anyhow::Result<Policy> {
        let policy: Policy = strict::from_str(text)?;
        policy.validate()?;
        Ok(policy)
    }

    const DESK: &str = r#"{schema: 1, grants: [
        {name: "desk", caller: {cgroup: "/machine.slice/systemd-nspawn@desk.service/*"},
         verbs: ["net.*", "power.suspend"], limits: {links: ["eth0"]},
         expires: "2030-01-01T00:00:00Z", rate: {count: 2, per_s: 60}}
    ]}"#;

    fn now() -> chrono::DateTime<chrono::Utc> {
        "2026-10-12T00:00:00Z".parse().unwrap()
    }

    #[test]
    fn matching_by_subtree_family_and_expiry() {
        let p = policy(DESK).unwrap();
        let inside = caller(
            "/machine.slice/systemd-nspawn@desk.service/payload",
            0,
            true,
        );
        assert_eq!(
            p.grant_for(&inside, "net.link.apply", now()).unwrap().name,
            "desk"
        );
        assert_eq!(
            p.grant_for(&inside, "power.suspend", now()).unwrap().name,
            "desk"
        );
        assert_eq!(
            p.grant_for(&inside, "power.reboot", now()).unwrap_err(),
            Refusal::NoGrant
        );
        // `net.*` is a family: it does not cover a verb that merely starts with "net".
        assert_eq!(
            p.grant_for(&inside, "network.x", now()).unwrap_err(),
            Refusal::NoGrant
        );
        // A sibling whose name extends the subtree's is outside it.
        let sibling = caller(
            "/machine.slice/systemd-nspawn@desk.service2/payload",
            0,
            true,
        );
        assert_eq!(
            p.grant_for(&sibling, "net.link.apply", now()).unwrap_err(),
            Refusal::NoGrant
        );
        let late = "2030-01-01T00:00:00Z".parse().unwrap();
        assert_eq!(
            p.grant_for(&inside, "net.link.apply", late).unwrap_err(),
            Refusal::NoGrant
        );
    }

    #[test]
    fn unverified_callers_match_nothing() {
        let p = policy(DESK).unwrap();
        let c = caller(
            "/machine.slice/systemd-nspawn@desk.service/payload",
            0,
            false,
        );
        assert_eq!(
            p.grant_for(&c, "net.link.apply", now()).unwrap_err(),
            Refusal::Unverified
        );
        assert!(p.held_by(&c, now()).is_empty());
    }

    #[test]
    fn limits_restrict_only_the_keys_they_name() {
        let p = policy(DESK).unwrap();
        let g = &p.grants[0];
        assert!(g.allows("links", "eth0"));
        assert!(!g.allows("links", "wg0"));
        assert!(g.allows("ssids", "anything"));
    }

    #[test]
    fn validation_refuses_bad_policies() {
        let bad = [
            r#"{schema: 2, grants: []}"#,
            r#"{schema: 1, grants: [{name: "a", caller: {cgroup: "/x"}, verbs: ["*"]}]}"#,
            r#"{schema: 1, grants: [{name: "a", caller: {cgroup: "/x"}, verbs: ["net.*.x"]}]}"#,
            r#"{schema: 1, grants: [{name: "a", caller: {cgroup: "/x"}, verbs: ["hostd.*"]}]}"#,
            r#"{schema: 1, grants: [{name: "a", caller: {cgroup: "/x"}, verbs: ["hostd.thaw"]}]}"#,
            r#"{schema: 1, grants: [{name: "a", caller: {cgroup: "x"}, verbs: ["net.*"]}]}"#,
            r#"{schema: 1, grants: [{name: "a", caller: {cgroup: "/x"}, verbs: []}]}"#,
            r#"{schema: 1, grants: [{name: "a", caller: {cgroup: "/x"}, verbs: ["net.*"], expires: "soon"}]}"#,
            r#"{schema: 1, grants: [{name: "a", caller: {cgroup: "/x"}, verbs: ["net.*"]},
                                    {name: "a", caller: {cgroup: "/y"}, verbs: ["net.*"]}]}"#,
            r#"{schema: 1, grants: [{name: "a", caller: {cgroup: "/x"}, verbs: ["net.*"], extra: 1}]}"#,
        ];
        for text in bad {
            assert!(policy(text).is_err(), "accepted: {text}");
        }
    }

    #[test]
    fn missing_policy_is_empty_and_loose_files_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("policy.mx");
        assert!(Policy::load(&path, true).unwrap().grants.is_empty());
        std::fs::write(&path, DESK).unwrap();
        assert_eq!(Policy::load(&path, false).unwrap().grants.len(), 1);
        // Writable by others: refused, whoever owns it (tests may run as root).
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o666)).unwrap();
        assert!(Policy::load(&path, true).is_err());
        // A temporary directory is never root-only, so even a strict file
        // inside it is refused under require_root.
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(Policy::load(&path, true).is_err());
    }

    #[test]
    fn a_symlinked_policy_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real.mx");
        std::fs::write(&real, DESK).unwrap();
        let link = dir.path().join("policy.mx");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert!(Policy::load(&link, false).is_err());
    }

    #[test]
    fn rate_limit_slides() {
        let p = policy(DESK).unwrap();
        let g = &p.grants[0];
        let mut limiter = RateLimiter::default();
        let t0 = Instant::now();
        let verb = "net.link.apply";
        assert!(limiter.admit(g, verb, "/a", t0).is_ok());
        assert!(limiter.admit(g, verb, "/a", t0).is_ok());
        assert!(limiter.admit(g, verb, "/a", t0).is_err());
        // Another verb, and another caller under the same grant, have their own windows.
        assert!(limiter.admit(g, "net.wifi.connect", "/a", t0).is_ok());
        assert!(limiter.admit(g, verb, "/b", t0).is_ok());
        assert!(
            limiter
                .admit(g, verb, "/a", t0 + Duration::from_secs(60))
                .is_ok()
        );
    }

    #[test]
    fn many_callers_share_a_grant_wide_cap() {
        let p = policy(DESK).unwrap();
        let g = &p.grants[0];
        let mut limiter = RateLimiter::default();
        let t0 = Instant::now();
        // count 2 per caller, grant-wide 2 × GRANT_SHARE.
        let admitted = (0..20)
            .filter(|i| {
                limiter
                    .admit(g, "net.link.apply", &format!("/c{i}"), t0)
                    .is_ok()
            })
            .count();
        assert_eq!(admitted, (2 * GRANT_SHARE) as usize);
    }

    #[test]
    fn a_subtree_includes_its_own_cgroup() {
        let p = policy(DESK).unwrap();
        let unit = caller("/machine.slice/systemd-nspawn@desk.service", 0, true);
        assert!(p.grant_for(&unit, "net.link.apply", now()).is_ok());
    }
}
