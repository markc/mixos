//! The COS tab's unit states: systemd's `ActiveState` for each MixOS unit,
//! read straight from systemd's D-Bus manager. Nothing here runs on a timer;
//! the daemon reads them when the popup asks for a snapshot.

use serde::Serialize;
use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::OwnedObjectPath;

const SYSTEMD: &str = "org.freedesktop.systemd1";
const SYSTEMD_PATH: &str = "/org/freedesktop/systemd1";
const MANAGER: &str = "org.freedesktop.systemd1.Manager";
const UNIT: &str = "org.freedesktop.systemd1.Unit";
const NO_SUCH_UNIT: &str = "org.freedesktop.systemd1.NoSuchUnit";

/// The units the COS tab lists: (unit, which manager runs it).
pub const WATCHED: &[(&str, Manager)] = &[
    ("noded.service", Manager::System),
    ("hostd.service", Manager::System),
    ("monitord.service", Manager::System),
    ("webd.service", Manager::System),
    ("mixtrayd.service", Manager::User),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Manager {
    System,
    User,
}

impl Manager {
    fn label(self) -> &'static str {
        match self {
            Manager::System => "system",
            Manager::User => "user",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Unit {
    pub name: &'static str,
    pub manager: &'static str,
    /// systemd's `ActiveState`, or `not installed` or `unavailable`.
    pub state: String,
}

/// Queries every watched unit once, opening each bus at most once.
pub fn probe_all() -> Vec<Unit> {
    let system = Connection::system();
    let session = Connection::session();
    WATCHED
        .iter()
        .map(|&(name, manager)| {
            let bus = match manager {
                Manager::System => system.as_ref(),
                Manager::User => session.as_ref(),
            };
            let state = match bus {
                Ok(conn) => match active_state(conn, name) {
                    Ok(Some(state)) => state,
                    Ok(None) => "not installed".to_string(),
                    Err(error) => format!("unavailable ({error})"),
                },
                Err(error) => format!("unavailable ({error})"),
            };
            Unit {
                name,
                manager: manager.label(),
                state,
            }
        })
        .collect()
}

/// `Ok(None)` when systemd does not know the unit.
fn active_state(conn: &Connection, name: &str) -> Result<Option<String>, String> {
    let manager = Proxy::new(conn, SYSTEMD, SYSTEMD_PATH, MANAGER).map_err(|e| e.to_string())?;
    let path: OwnedObjectPath = match manager.call("GetUnit", &(name,)) {
        Ok(path) => path,
        Err(zbus::Error::MethodError(error, _, _)) if error.as_str() == NO_SUCH_UNIT => {
            return Ok(None);
        }
        Err(error) => return Err(error.to_string()),
    };
    let unit = Proxy::new(conn, SYSTEMD, path.as_str(), UNIT).map_err(|e| e.to_string())?;
    unit.get_property::<String>("ActiveState")
        .map(Some)
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watched_units_are_plain_service_names() {
        for (name, _) in WATCHED {
            assert!(name.ends_with(".service"), "{name}");
            assert!(!name.contains('@'), "templated units need an instance: {name}");
        }
    }

    #[test]
    fn manager_labels_match_systemd_scopes() {
        assert_eq!(Manager::System.label(), "system");
        assert_eq!(Manager::User.label(), "user");
    }
}
