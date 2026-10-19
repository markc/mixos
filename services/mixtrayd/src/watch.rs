//! Live updates for the COS tab. One task per systemd manager (system and
//! user) subscribes to systemd's signals and bumps a revision whenever a
//! watched unit's properties change. `mixtrayd.wait` blocks on that revision,
//! so the popup hears about a change as soon as systemd announces it, and
//! nothing runs on a timer.

use std::collections::HashSet;
use std::time::Duration;

use futures::StreamExt;
use tokio::sync::watch;
use zbus::message::Type;
use zbus::{Connection, MatchRule, MessageStream};

use crate::units::{self, Manager};

/// How long a watcher waits before reopening a bus it lost.
const RECONNECT: Duration = Duration::from_secs(5);
/// Signals queued for a slow reader before the oldest are dropped. Any one
/// signal is enough to bump the revision, so losing some is harmless.
const QUEUE: usize = 64;

/// Starts both watchers. Each reconnects on its own if its bus goes away.
pub fn spawn(rev: watch::Sender<u64>) {
    for manager in [Manager::System, Manager::User] {
        let rev = rev.clone();
        tokio::spawn(async move {
            loop {
                if let Err(error) = follow(manager, &rev).await {
                    eprintln!("mixtrayd: {manager:?} unit watch ended: {error}");
                }
                tokio::time::sleep(RECONNECT).await;
            }
        });
    }
}

/// The object paths of the watched units that `manager` runs.
pub fn watched_paths(manager: Manager) -> HashSet<String> {
    units::WATCHED
        .iter()
        .filter(|(_, m)| *m == manager)
        .map(|(name, _)| units::object_path(name))
        .collect()
}

async fn follow(manager: Manager, rev: &watch::Sender<u64>) -> zbus::Result<()> {
    let conn = match manager {
        Manager::System => Connection::system().await?,
        Manager::User => Connection::session().await?,
    };
    let paths = watched_paths(manager);
    let rule = MatchRule::builder()
        .msg_type(Type::Signal)
        .sender(units::SYSTEMD)?
        .interface("org.freedesktop.DBus.Properties")?
        .member("PropertiesChanged")?
        .path_namespace(units::UNIT_PATH_PREFIX.trim_end_matches('/'))?
        .build();
    let mut stream = MessageStream::for_match_rule(rule, &conn, Some(QUEUE)).await?;
    // systemd only emits unit signals once some client has subscribed.
    conn.call_method(
        Some(units::SYSTEMD),
        units::SYSTEMD_PATH,
        Some(units::MANAGER),
        "Subscribe",
        &(),
    )
    .await?;
    // Count the new subscription itself as a change, so a popup waiting
    // across a reconnect rereads the units it may have missed.
    rev.send_modify(|r| *r += 1);
    while let Some(message) = stream.next().await {
        let message = message?;
        let header = message.header();
        if header.path().is_some_and(|p| paths.contains(p.as_str())) {
            rev.send_modify(|r| *r += 1);
        }
    }
    Ok(())
}

/// Whether a `mixtrayd.wait` caller must wait: only when it has already seen
/// the current revision. A caller behind (or ahead, after a daemon restart)
/// gets a snapshot at once.
pub fn must_wait(since: Option<u64>, current: u64) -> bool {
    since == Some(current)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_manager_watches_only_its_own_units() {
        let system = watched_paths(Manager::System);
        let user = watched_paths(Manager::User);
        assert!(system.contains("/org/freedesktop/systemd1/unit/noded_2eservice"));
        assert!(system.contains("/org/freedesktop/systemd1/unit/maild_2eservice"));
        assert!(user.contains("/org/freedesktop/systemd1/unit/mixtrayd_2eservice"));
        assert!(!system.contains("/org/freedesktop/systemd1/unit/mixtrayd_2eservice"));
    }

    #[test]
    fn a_caller_waits_only_when_it_has_the_current_revision() {
        assert!(must_wait(Some(4), 4));
        assert!(!must_wait(Some(3), 4));
        assert!(!must_wait(Some(9), 4));
        assert!(!must_wait(None, 4));
    }
}
