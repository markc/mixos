//! The COS tab's unit states: systemd's `ActiveState` for each MixOS unit,
//! read straight from systemd's D-Bus manager. Nothing here runs on a timer;
//! the daemon reads them when the popup asks for a snapshot, and `watch`
//! follows systemd's change signals for the same units.

use serde::Serialize;
use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::OwnedObjectPath;

pub const SYSTEMD: &str = "org.freedesktop.systemd1";
pub const SYSTEMD_PATH: &str = "/org/freedesktop/systemd1";
pub const MANAGER: &str = "org.freedesktop.systemd1.Manager";
/// Where systemd publishes one object per loaded unit.
pub const UNIT_PATH_PREFIX: &str = "/org/freedesktop/systemd1/unit/";
const UNIT: &str = "org.freedesktop.systemd1.Unit";
const NO_SUCH_UNIT: &str = "org.freedesktop.systemd1.NoSuchUnit";

/// The units the COS tab lists: (unit, which manager runs it).
pub const WATCHED: &[(&str, Manager)] = &[
    ("noded.service", Manager::System),
    ("hostd.service", Manager::System),
    ("maild.service", Manager::System),
    ("monitord.service", Manager::System),
    ("webd.service", Manager::System),
    ("mixtrayd.service", Manager::User),
];

/// systemd's object path for a unit: every byte that is not an ASCII letter
/// or digit becomes `_` and two lower-case hex digits, so `noded.service` is
/// `/org/freedesktop/systemd1/unit/noded_2eservice`.
pub fn object_path(unit: &str) -> String {
    let mut path = String::from(UNIT_PATH_PREFIX);
    for byte in unit.bytes() {
        if byte.is_ascii_alphanumeric() {
            path.push(byte as char);
        } else {
            path.push_str(&format!("_{byte:02x}"));
        }
    }
    path
}

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
    /// Whether the COS tab offers start, stop and restart for this unit.
    pub controllable: bool,
}

/// Units the popup never controls: hostd is the one that grants control, and
/// stopping the popup's own data service would leave the popup with nothing.
const NEVER_CONTROLLED: &[&str] = &["hostd.service", "mixtrayd.service"];

/// Whether a request may act on `name`: a system unit in the COS list that is
/// not hostd or this daemon. hostd's grant is the final say; this keeps the
/// popup from asking for anything the grant would refuse anyway.
pub fn is_controllable(name: &str) -> bool {
    WATCHED
        .iter()
        .any(|&(unit, manager)| unit == name && manager == Manager::System)
        && !NEVER_CONTROLLED.contains(&name)
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
                controllable: is_controllable(name),
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
            assert!(
                !name.contains('@'),
                "templated units need an instance: {name}"
            );
        }
    }

    #[test]
    fn object_paths_use_systemd_escaping() {
        assert_eq!(
            object_path("noded.service"),
            "/org/freedesktop/systemd1/unit/noded_2eservice"
        );
        assert_eq!(
            object_path("getty@tty1.service"),
            "/org/freedesktop/systemd1/unit/getty_40tty1_2eservice"
        );
        assert_eq!(
            object_path("user-runtime-dir.service"),
            "/org/freedesktop/systemd1/unit/user_2druntime_2ddir_2eservice"
        );
    }

    #[test]
    fn maild_is_controllable_and_hostd_is_not() {
        assert!(is_controllable("maild.service"));
        assert!(!is_controllable("hostd.service"));
    }

    #[test]
    fn manager_labels_match_systemd_scopes() {
        assert_eq!(Manager::System.label(), "system");
        assert_eq!(Manager::User.label(), "user");
    }
}
