//! Reads the systemd state of the MixOS units the tray shows.
//!
//! Each query is a direct call on systemd's D-Bus manager. Nothing here runs
//! on a timer: the tray calls `probe_all` when its menu opens.

use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::OwnedObjectPath;

const SYSTEMD: &str = "org.freedesktop.systemd1";
const SYSTEMD_PATH: &str = "/org/freedesktop/systemd1";
const MANAGER: &str = "org.freedesktop.systemd1.Manager";
const UNIT: &str = "org.freedesktop.systemd1.Unit";
const NO_SUCH_UNIT: &str = "org.freedesktop.systemd1.NoSuchUnit";

/// The units shown in the menu. Templated units (`maild@.service`,
/// `noded@.service`) need an instance name, so they are not listed yet.
const WATCHED: &[(&str, Manager)] = &[
    ("hostd.service", Manager::System),
    ("monitord.service", Manager::System),
    ("webd.service", Manager::System),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Manager {
    System,
    User,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UnitState {
    /// The systemd `ActiveState`, such as "active" or "failed".
    Known(String),
    /// systemd has no such unit loaded.
    NotInstalled,
    /// The query itself failed, for example when a bus is unreachable.
    Error(String),
}

impl UnitState {
    pub fn label(&self) -> String {
        match self {
            Self::Known(state) => state.clone(),
            Self::NotInstalled => "not installed".into(),
            Self::Error(error) => format!("unavailable ({error})"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unit {
    pub name: &'static str,
    pub state: UnitState,
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
                    Ok(Some(state)) => UnitState::Known(state),
                    Ok(None) => UnitState::NotInstalled,
                    Err(error) => UnitState::Error(error),
                },
                Err(error) => UnitState::Error(error.to_string()),
            };
            Unit { name, state }
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

/// One line for the tray tooltip, such as "2 of 3 active, 1 failed".
pub fn summary(units: &[Unit]) -> String {
    if units.is_empty() {
        return "No MixOS units checked yet".into();
    }
    let active = units
        .iter()
        .filter(|u| u.state == UnitState::Known("active".into()))
        .count();
    let failed = units
        .iter()
        .filter(|u| u.state == UnitState::Known("failed".into()))
        .count();
    let mut line = format!("{active} of {} active", units.len());
    if failed > 0 {
        line.push_str(&format!(", {failed} failed"));
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit(name: &'static str, state: UnitState) -> Unit {
        Unit { name, state }
    }

    #[test]
    fn labels_show_systemd_state_or_the_reason_it_is_missing() {
        assert_eq!(UnitState::Known("active".into()).label(), "active");
        assert_eq!(UnitState::NotInstalled.label(), "not installed");
        assert_eq!(
            UnitState::Error("no bus".into()).label(),
            "unavailable (no bus)"
        );
    }

    #[test]
    fn summary_counts_active_and_failed_units() {
        let units = [
            unit("hostd.service", UnitState::Known("active".into())),
            unit("webd.service", UnitState::Known("failed".into())),
            unit("monitord.service", UnitState::NotInstalled),
        ];
        assert_eq!(summary(&units), "1 of 3 active, 1 failed");
    }

    #[test]
    fn summary_before_the_first_probe_says_so() {
        assert_eq!(summary(&[]), "No MixOS units checked yet");
    }

    #[test]
    fn watched_units_are_plain_service_names() {
        for (name, _) in WATCHED {
            assert!(name.ends_with(".service"), "{name}");
            assert!(!name.contains('@'), "templated units need an instance: {name}");
        }
    }
}
