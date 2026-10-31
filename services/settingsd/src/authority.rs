// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
use crate::store::{Accepted, Store, canonical, valid_operation_id};
use serde_json::{Value, json};
use settings::*;

/// What a settingsd serves from (F1, design §5.4).
pub enum Role {
    /// The machine's one authority: the profile store, the only writer.
    Authority(Store),
    /// A desk's mirror of the authority on `node`: reads from its sealed
    /// cache; writes are the authority's (forwarded in S7, refused until).
    Mirror { node: String },
}

pub struct Authority {
    pub role: Role,
    pub accepted: Accepted,
    pub snapshot: Snapshot,
    pub published: Option<Revision>,
    /// The 1.0.0 topics: each one's content and the revision that last
    /// changed it ([`crate::publication`]).
    pub board: crate::publication::Board,
    /// A mirror that is not attached to its authority: it serves its cache
    /// and reports `stale`. Never set on the authority.
    pub stale: bool,
    /// In-memory sidecar: the accent per context, derived from the same design
    /// as `snapshot.effective`. Never serialised; it is not part of the sealed
    /// `Snapshot` or `Effective`. Rebuilt wherever `snapshot` is rebuilt.
    accents: Accents,
}
impl Authority {
    pub fn new(store: Store, accepted: Accepted) -> anyhow::Result<Self> {
        accepted.check(&accepted.binding)?;
        Self::serving(Role::Authority(store), accepted)
    }
    /// A mirror serving `cache`, which [`crate::mirror::open`] verified.
    pub fn mirror(cache: &crate::mirror::Cache) -> anyhow::Result<Self> {
        Self::serving(
            Role::Mirror {
                node: cache.authority.clone(),
            },
            cache.accepted(),
        )
    }
    fn serving(role: Role, accepted: Accepted) -> anyhow::Result<Self> {
        let (effective, accents) = accepted.interpretation()?;
        let snapshot = snapshot(&accepted, effective)?;
        let stale = matches!(role, Role::Mirror { .. });
        let mut serving = Self {
            role,
            accepted,
            snapshot,
            published: None,
            board: Default::default(),
            stale,
            accents,
        };
        serving.stage();
        Ok(serving)
    }
    /// A mirror takes its authority's pushed record, keeping what it has
    /// published, and stamps each topic as the authority did. Returns the
    /// topics that moved.
    pub fn adopt(
        &mut self,
        cache: &crate::mirror::Cache,
        changed_at: &std::collections::BTreeMap<String, Revision>,
    ) -> anyhow::Result<Vec<String>> {
        anyhow::ensure!(
            matches!(&self.role, Role::Mirror { node } if *node == cache.authority),
            "only a mirror of {} adopts its record",
            cache.authority
        );
        let accepted = cache.accepted();
        let (effective, accents) = accepted.interpretation()?;
        self.snapshot = snapshot(&accepted, effective)?;
        self.accepted = accepted;
        self.accents = accents;
        Ok(self.stage_with(Some(changed_at)))
    }
    /// The desktop's resolved accent, `#rrggbb`.
    pub(crate) fn accent_hex(&self) -> Option<String> {
        self.accents
            .get(settings::appearance::APPEARANCE_CONTEXT)
            .map(hex)
    }
    /// The store's outcome of its last write is unknown: no publication or
    /// mutation until a restart reads it back.
    pub fn recovering(&self) -> bool {
        matches!(&self.role, Role::Authority(store) if store.recovering)
    }
    fn restored(&self) -> bool {
        matches!(&self.role, Role::Authority(store) if store.restored)
    }
    fn target(&self, binding: &Binding) -> Result<(), Value> {
        if binding != &self.accepted.binding {
            Err(json!({"status":"wrong_target","binding":self.accepted.binding}))
        } else {
            Ok(())
        }
    }
    pub fn read(&self, request: ReadRequest) -> Result<Value, Value> {
        self.target(&request.binding)?;
        let pending = self.published != Some(self.accepted.revision);
        Ok(match request.view {
            // S7-TRANSITION: the 0.8.0 views, `full` and `summary`.
            settings::View::Full => {
                let snapshot = self.snapshot_in(request.schema)?;
                json!({"status":"current","snapshot":snapshot,"publication_pending":pending,"recovering":self.recovering(),"restored_from_backup":self.restored()})
            }
            settings::View::Summary => {
                json!({"status":"current","summary":settings::Summary::of(&self.snapshot),"publication_pending":pending,"recovering":self.recovering(),"restored_from_backup":self.restored()})
            }
            settings::View::Values | settings::View::Look | settings::View::Snapshot => {
                return self.read_v1(&request);
            }
        })
    }
    /// Headless appearance for portal consumers: the desktop context's mode,
    /// contrast and accent, tagged with the snapshot identity.
    pub fn appearance(&self, request: ReadRequest) -> Result<Value, Value> {
        self.target(&request.binding)?;
        let accent = self
            .accents
            .get(settings::appearance::APPEARANCE_CONTEXT)
            .map(|colour| [colour.red, colour.green, colour.blue])
            .ok_or_else(|| {
                diagnostic(Diagnostic::new(
                    "missing_accent",
                    "design.pairs.accent",
                    "Resolved design has no accent pair",
                ))
            })?;
        let projection =
            settings::appearance::AppearanceProjection::from_snapshot(&self.snapshot, accent)
                .map_err(diagnostic)?
                .legacy();
        // One atomic read supplies both the shared Consumer's authority evidence
        // and the portal projection, including the private accent source.
        let snapshot = self.snapshot_in(request.schema)?;
        Ok(json!({"status":"current","appearance":projection,"snapshot":snapshot}))
    }
    /// The accepted snapshot in the schema a read asked for: 1 (complete) or
    /// 2 (each distinct design once, [`settings::compact`]).
    pub fn snapshot_in(&self, schema: u32) -> Result<Value, Value> {
        let encoded = match schema {
            settings::SCHEMA => serde_json::to_value(&self.snapshot),
            settings::compact::COMPACT_SCHEMA => {
                settings::compact::encode(&self.snapshot).and_then(serde_json::to_value)
            }
            other => {
                return Err(
                    json!({"status":"unsupported_schema","schema":other,"supported":[settings::SCHEMA, settings::compact::COMPACT_SCHEMA]}),
                );
            }
        };
        encoded.map_err(|e| json!({"status":"validation_failed","message":e.to_string()}))
    }
    pub fn status(&self, binding: &Binding, operation: Option<&str>) -> Result<Value, Value> {
        self.target(binding)?;
        let receipt =
            operation.and_then(|id| self.accepted.receipts.iter().find(|r| r.operation_id == id));
        let (role, authority, stale) = match &self.role {
            Role::Authority(_) => ("authority", self.accepted.binding.instance.as_str(), false),
            // Stale until attached: then each push keeps it current.
            Role::Mirror { node } => ("mirror", node.as_str(), self.stale),
        };
        Ok(
            json!({"status": if operation.is_some() && receipt.is_none() {"unknown_operation"} else {"current"},
            "receipt":receipt,"binding":self.accepted.binding,"incarnation":self.accepted.incarnation,
            "revision":self.accepted.revision,"published_revision":self.published,
            "publication_pending":self.published != Some(self.accepted.revision),"recovering":self.recovering(),
            "restored_from_backup":self.restored(),"role":role,"authority":authority,"stale":stale,
            "consumers":[],"renderer_integration":"pending"}),
        )
    }
    /// Writes are the authority's. A mirror refuses them until S7 forwards
    /// them.
    fn writable(&self) -> Result<(), Value> {
        match &self.role {
            Role::Authority(_) => Ok(()),
            Role::Mirror { node } => Err(json!({"status":"authority_unreachable","node":node})),
        }
    }
    /// The values `request` would leave, through the registry's one write.
    fn patched(&self, request: &ApplyRequest) -> Result<registry::Values, Value> {
        registry::apply(&self.accepted.values, &request.changes, &request.reset)
            .map(canonical)
            .map_err(diagnostics)
    }
    pub fn validate(&self, request: &ApplyRequest) -> Result<Value, Value> {
        self.target(&request.binding)?;
        self.writable()?;
        self.fence(request)?;
        let mut candidate = self.accepted.clone();
        candidate.values = self.patched(request)?;
        let (effective, accents) =
            resolve::values::resolve(&candidate.values, &candidate.embedded_source)
                .map_err(diagnostics)?;
        let candidate_snapshot = snapshot(&candidate, effective).map_err(snapshot_error)?;
        // The would-be content of every topic the batch moves, as its topic
        // would carry it (§5.2): each prefix's values with defaults filled
        // and, for `appearance`, the resolved mode and accent. Compared by
        // content, so a `state.*` change also moves `appearance` and `look`.
        let resolved = settings::topics::Resolved {
            mode: settings::topics::resolved_mode(&candidate.values),
            accent: accents
                .get(settings::appearance::APPEARANCE_CONTEXT)
                .map(hex),
        };
        let mut touched = serde_json::Map::new();
        let mut moved = Vec::new();
        for (prefix, content) in settings::topics::contents(&candidate.values, &resolved) {
            let content = serde_json::to_value(content).unwrap_or(Value::Null);
            if self.board.content(prefix) != Some(&content) {
                moved.push(prefix);
                touched.insert(prefix.to_owned(), content);
            }
        }
        let look = settings::topics::look_content(
            &candidate.values,
            &resolved,
            &candidate_snapshot.source_digest,
        );
        let look = serde_json::to_value(look).unwrap_or(Value::Null);
        if self.board.content(crate::publication::LOOK) != Some(&look) {
            moved.push(crate::publication::LOOK);
        }
        Ok(
            json!({"status":"valid","incarnation":self.accepted.incarnation,"revision":self.accepted.revision,"source_digest":candidate_snapshot.source_digest,"effective":candidate_snapshot.effective,"values":touched,"moved":moved}),
        )
    }
    fn fence(&self, request: &ApplyRequest) -> Result<(), Value> {
        if request.expected_incarnation != self.accepted.incarnation
            || request.expected_revision != self.accepted.revision
        {
            Err(
                json!({"status":"conflict","incarnation":self.accepted.incarnation,"revision":self.accepted.revision}),
            )
        } else {
            Ok(())
        }
    }
    /// Serialized by the owning worker. Deduplication before revision fencing
    /// makes a lost reply retry retrieve its original receipt after later edits.
    pub fn apply(&mut self, request: ApplyRequest) -> Result<Value, Value> {
        self.target(&request.binding)?;
        self.writable()?;
        if self.recovering() {
            return Err(
                json!({"status":"outcome_unknown","message":"Recovery required before another mutation"}),
            );
        }
        if request.expected_incarnation != self.accepted.incarnation {
            return Err(
                json!({"status":"conflict","incarnation":self.accepted.incarnation,"revision":self.accepted.revision}),
            );
        }
        if !valid_operation_id(&request.operation_id) {
            return Err(json!({"status":"validation_failed","path":"operation_id"}));
        }
        let digest = request
            .digest()
            .map_err(|e| json!({"status":"validation_failed","message":e.to_string()}))?;
        if request
            .request_digest
            .as_ref()
            .is_some_and(|given| given != &digest)
        {
            return Err(
                json!({"status":"validation_failed","path":"request_digest","message":"Digest mismatch"}),
            );
        }
        if let Some(receipt) = self
            .accepted
            .receipts
            .iter()
            .find(|r| r.operation_id == request.operation_id)
        {
            if receipt.request_digest != digest {
                return Err(json!({"status":"operation_id_reused","receipt":receipt}));
            }
            return Ok(
                json!({"status":receipt.outcome,"receipt":receipt,"publication_pending":self.published != Some(self.accepted.revision),"replayed":true}),
            );
        }
        self.fence(&request)?;
        let values = self.patched(&request)?;
        let unchanged = values == self.accepted.values;
        let mut next = self.accepted.clone();
        if !unchanged {
            next.revision = Revision(
                next.revision
                    .0
                    .checked_add(1)
                    .ok_or_else(|| json!({"status":"revision_exhausted"}))?,
            );
            if touches_design(&self.accepted.values, &values) {
                next.design_revision = Revision(
                    next.design_revision
                        .0
                        .checked_add(1)
                        .ok_or_else(|| json!({"status":"revision_exhausted"}))?,
                );
            }
        }
        next.values = values;
        let (effective, accents) = if unchanged {
            (self.snapshot.effective.clone(), self.accents.clone())
        } else {
            resolve::values::resolve(&next.values, &next.embedded_source).map_err(diagnostics)?
        };
        let next_snapshot = snapshot(&next, effective).map_err(snapshot_error)?;
        next.effective_digest = settings::digest(&next_snapshot.effective)
            .map_err(|e| json!({"status":"validation_failed","message":e.to_string()}))?;
        let receipt = Receipt {
            operation_id: request.operation_id,
            request_digest: digest,
            incarnation: next.incarnation.clone(),
            revision: next.revision,
            outcome: if unchanged {
                Outcome::Unchanged
            } else {
                Outcome::Changed
            },
        };
        next.receipts.push(receipt.clone());
        if next.receipts.len() > MAX_RECEIPTS {
            next.receipts.remove(0);
        }
        next.seal()
            .map_err(|e| json!({"status":"storage_failed","message":e.to_string()}))?;
        let Role::Authority(store) = &mut self.role else {
            return Err(json!({"status":"authority_unreachable"}));
        };
        if let Err(error) = store.commit(&self.accepted, &next) {
            return Err(
                json!({"status":if store.recovering {"outcome_unknown"} else {"storage_failed"},"message":error.to_string()}),
            );
        }
        self.accepted = next;
        self.snapshot = next_snapshot;
        self.accents = accents;
        self.stage();
        Ok(
            json!({"status":receipt.outcome,"receipt":receipt,"publication_pending":self.published != Some(self.accepted.revision),"replayed":false}),
        )
    }
}
/// Whether a write changed the design's inputs (§2.2): a changed key whose
/// registry domains include paint, text or layout, a `tune.style` token by
/// the tunable's own domains. This covers format 4's `appearance` and
/// `apps` (the look, framing, density, per-app looks) and also the app
/// domains and `accessibility.text_scale`.
pub fn touches_design(before: &registry::Values, after: &registry::Values) -> bool {
    use registry::Domain::{Layout, Paint, Text};
    let design = |domain: &registry::Domain| matches!(domain, Paint | Text | Layout);
    before
        .keys()
        .chain(after.keys())
        .filter(|path| before.get(*path) != after.get(*path))
        .any(|path| {
            let tunable = path
                .strip_prefix("tune.")
                .and_then(registry::tunable)
                .is_some_and(|t| {
                    t.domains
                        .iter()
                        .filter_map(|name| registry::Domain::from_name(name))
                        .any(|d| design(&d))
                });
            tunable || registry::lookup(path).is_ok_and(|m| m.def.domains.iter().any(design))
        })
}
/// `#rrggbb`, as the topics carry a resolved accent.
fn hex(colour: &design::SrgbColour) -> String {
    colour.to_hex()[..7].to_ascii_lowercase()
}
fn diagnostic(error: Diagnostic) -> Value {
    diagnostics(vec![error])
}
fn diagnostics(errors: Vec<Diagnostic>) -> Value {
    json!({"status":"validation_failed","diagnostics":errors})
}
#[derive(Debug)]
struct SnapshotTooLarge {
    bytes: usize,
}
impl std::fmt::Display for SnapshotTooLarge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "snapshot {} bytes exceeds inline budget {}; native artifacts required",
            self.bytes, MAX_SNAPSHOT_BYTES
        )
    }
}
impl std::error::Error for SnapshotTooLarge {}
fn snapshot_error(error: anyhow::Error) -> Value {
    if let Some(limit) = error.downcast_ref::<SnapshotTooLarge>() {
        json!({"status":"snapshot_too_large","bytes":limit.bytes,"maximum":MAX_SNAPSHOT_BYTES,"message":error.to_string()})
    } else {
        json!({"status":"validation_failed","message":error.to_string()})
    }
}
/// The published snapshot. Its `desktop` is the format-4 projection of the
/// values ([`settings::migrate::reverse`]) for the consumers that still read
/// it; S7's schema 3 carries the values themselves.
pub(crate) fn snapshot(
    accepted: &Accepted,
    effective: std::collections::BTreeMap<String, Effective>,
) -> anyhow::Result<Snapshot> {
    let desktop = settings::migrate::reverse(&accepted.values).desktop;
    let source_digest = settings::source_digest(
        desktop
            .appearance
            .source
            .as_deref()
            .unwrap_or(&accepted.embedded_source),
    );
    let result = Snapshot {
        schema: SCHEMA,
        binding: accepted.binding.clone(),
        incarnation: accepted.incarnation.clone(),
        revision: accepted.revision,
        design_revision: accepted.design_revision,
        source_digest,
        desktop,
        effective,
    };
    let bytes = result.encoded_len()?;
    if bytes > MAX_SNAPSHOT_BYTES {
        return Err(SnapshotTooLarge { bytes }.into());
    }
    // The compact form is published too, and native consumers hold it to
    // the budget plus its bounded overhead: refuse what they would refuse.
    let compact = serde_json::to_vec(&settings::compact::encode(&result)?)?.len();
    if compact > MAX_SNAPSHOT_BYTES + settings::compact::MAX_OVERHEAD_BYTES {
        return Err(SnapshotTooLarge { bytes: compact }.into());
    }
    Ok(result)
}
