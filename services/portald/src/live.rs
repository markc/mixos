// SPDX-License-Identifier: MIT OR Apache-2.0
//! The live portal. Startup requests the bus name with cached or default
//! values immediately, independently of the asynchronous authority feed.
//! Accepted projections replace the served values atomically and emit
//! `SettingChanged` for the changed keys only.
use crate::appearance::{NAMESPACE, Values};
use crate::cache;
use crate::portal::{self, Portal, PortalSignals};
use crate::status::{self, Origin, Shared};
use settings::appearance::AppearanceProjection;
use settings::{Binding, Revision};
use std::path::PathBuf;
use std::sync::{Arc, PoisonError, RwLock};
use tokio::sync::mpsc;
use zbus::fdo::{RequestNameFlags, RequestNameReply};
use zbus::zvariant::Value;

pub struct Config {
    pub address: String,
    pub state_dir: Option<PathBuf>,
    pub binding: Binding,
}

/// The evidence identity of the values held. Ordering within one incarnation
/// is by revision, then by design revision.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Identity {
    incarnation: String,
    revision: Revision,
    design_revision: Revision,
}

impl Identity {
    fn of(projection: &AppearanceProjection) -> Self {
        let (incarnation, revision, design_revision) = projection.identity();
        Self {
            incarnation: incarnation.to_owned(),
            revision,
            design_revision,
        }
    }
}

pub type Change = (&'static str, Value<'static>);

/// The values served and the evidence behind them.
pub struct State {
    identity: Option<Identity>,
    values: Values,
    origin: Origin,
}

impl State {
    pub fn initial(cached: Option<&AppearanceProjection>) -> Self {
        match cached {
            Some(projection) => Self {
                identity: Some(Identity::of(projection)),
                values: Values::from_projection(projection),
                origin: Origin::Cache,
            },
            None => Self {
                identity: None,
                values: Values::defaults(),
                origin: Origin::Defaults,
            },
        }
    }

    pub fn origin(&self) -> Origin {
        self.origin
    }

    pub fn values(&self) -> &Values {
        &self.values
    }

    /// Adopts a projection unless it is older than the held one within the
    /// same incarnation. Returns the changed keys, which may be empty when a
    /// cache or default origin is merely confirmed. `None` means stale.
    pub fn accept(&mut self, projection: &AppearanceProjection) -> Option<Vec<Change>> {
        let next = Identity::of(projection);
        if let Some(current) = &self.identity
            && current.incarnation == next.incarnation
            && (next.revision, next.design_revision) < (current.revision, current.design_revision)
        {
            return None;
        }
        let values = Values::from_projection(projection);
        let changes = self.values.changed(&values);
        self.values = values;
        self.identity = Some(next);
        self.origin = Origin::Settingsd;
        Some(changes)
    }
}

/// Applies one projection to the served values, the status and the cache.
fn adopt(
    state: &mut State,
    values: &RwLock<Values>,
    config: &Config,
    status: &Shared,
    projection: &AppearanceProjection,
) -> Option<Vec<Change>> {
    if projection.binding != config.binding || projection.validate().is_err() {
        status::update(status, |s| s.rejected += 1);
        return None;
    }
    let Some(changes) = state.accept(projection) else {
        status::update(status, |s| s.stale += 1);
        return None;
    };
    *values.write().unwrap_or_else(PoisonError::into_inner) = state.values().clone();
    let (incarnation, revision, design_revision) = projection.identity();
    status::update(status, |s| {
        s.origin = state.origin();
        s.incarnation = Some(incarnation.to_owned());
        s.revision = Some(revision);
        s.design_revision = Some(design_revision);
        s.settings_fault = None;
        s.accepted += 1;
    });
    if state.origin() == Origin::Settingsd
        && let Some(dir) = &config.state_dir
        && let Err(error) = cache::save(dir, projection)
    {
        tracing::warn!(%error, "appearance cache not saved");
    }
    Some(changes)
}

pub async fn serve(
    config: Config,
    mut projections: mpsc::Receiver<AppearanceProjection>,
    status: Shared,
) -> anyhow::Result<()> {
    config
        .binding
        .validate()
        .map_err(|fault| anyhow::anyhow!(fault.message))?;
    let cached = config
        .state_dir
        .as_deref()
        .and_then(|dir| cache::load(dir, &config.binding));
    let mut state = State::initial(cached.as_ref());
    let values = Arc::new(RwLock::new(state.values().clone()));
    status::update(&status, |s| s.origin = state.origin());

    // Export and acquire the name before touching the authority receiver.
    let mut source_open = true;

    let connection = zbus::connection::Builder::address(config.address.as_str())?
        .serve_at(portal::OBJECT_PATH, Portal::new(values.clone()))?
        .build()
        .await?;
    // Interfaces are exported before the name, and an existing owner is refused
    // rather than queued or replaced.
    match connection
        .request_name_with_flags(portal::BUS_NAME, RequestNameFlags::DoNotQueue.into())
        .await?
    {
        RequestNameReply::PrimaryOwner => {}
        _ => anyhow::bail!("{} is owned by another service", portal::BUS_NAME),
    }
    status::update(&status, |s| s.name_owned = true);
    tracing::info!(origin = ?state.origin(), "serving {}", portal::BUS_NAME);

    let interface = connection
        .object_server()
        .interface::<_, Portal>(portal::OBJECT_PATH)
        .await?;
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let mut sigint = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
    loop {
        tokio::select! {
            received = projections.recv(), if source_open => {
                let Some(projection) = received else {
                    source_open = false;
                    continue;
                };
                if let Some(changes) = adopt(&mut state, &values, &config, &status, &projection) {
                    for (key, value) in changes {
                        if let Err(error) = interface
                            .signal_emitter()
                            .setting_changed(NAMESPACE, key, &value)
                            .await
                        {
                            tracing::warn!(%error, key, "SettingChanged not emitted");
                        }
                    }
                }
            }
            // zbus closes the connection on peer disconnect, socket error or
            // restart of the bus. Nothing can serve or hold the name after that,
            // so exit non-zero and let the unit's Restart=on-failure reconnect.
            _ = connection.closed() => {
                status::update(&status, |s| s.name_owned = false);
                anyhow::bail!(
                    "session bus closed: {} released; exiting so the unit restarts",
                    portal::BUS_NAME
                );
            }
            _ = sigterm.recv() => break,
            _ = sigint.recv() => break,
        }
    }
    tracing::info!("portal stopping");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use settings::Binding;

    fn projection(incarnation: &str, revision: u64, mode: &str) -> AppearanceProjection {
        AppearanceProjection {
            schema: settings::appearance::APPEARANCE_SCHEMA,
            binding: Binding {
                instance: "host".into(),
                profile: "default".into(),
            },
            incarnation: incarnation.into(),
            revision: Revision(revision),
            design_revision: Revision(1),
            mode: mode.into(),
            contrast: "normal".into(),
            accent: [0.1, 0.2, 0.3],
        }
    }

    #[test]
    fn stale_revisions_are_refused_and_confirmation_is_quiet() {
        let mut state = State::initial(None);
        assert_eq!(state.origin(), Origin::Defaults);
        let first = state
            .accept(&projection("a", 2, "light"))
            .expect("accepted");
        assert_eq!(first.len(), 2, "color-scheme and the first accent");
        assert!(state.accept(&projection("a", 1, "dark")).is_none());
        assert!(
            state
                .accept(&projection("a", 2, "light"))
                .expect("same identity")
                .is_empty()
        );
        assert_eq!(state.values().color_scheme, 2);
    }

    #[test]
    fn a_new_incarnation_is_adopted_even_at_a_lower_revision() {
        let mut state = State::initial(None);
        state
            .accept(&projection("a", 5, "light"))
            .expect("accepted");
        let changes = state
            .accept(&projection("b", 1, "dark"))
            .expect("new incarnation");
        assert_eq!(changes.len(), 1);
        assert_eq!(state.values().color_scheme, 1);
    }

    #[test]
    fn a_cache_is_promoted_by_matching_authority_without_signals() {
        let cached = projection("a", 2, "light");
        let mut state = State::initial(Some(&cached));
        assert_eq!(state.origin(), Origin::Cache);
        assert!(state.accept(&cached).expect("accepted").is_empty());
        assert_eq!(state.origin(), Origin::Settingsd);
    }
}
