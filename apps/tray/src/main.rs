//! MixOS tray: a windowless StatusNotifierItem that shows MixOS unit state.
//!
//! The item registers with Plasma's own tray host. The unit states are read
//! when the menu opens, so nothing refreshes in the background.

mod units;

use std::sync::mpsc;

use ksni::blocking::TrayMethods as _;
use ksni::menu::StandardItem;
use ksni::{Category, MenuItem, Status, ToolTip, Tray};
use units::Unit;

const APP_ID: &str = "dev.mixos.tray";

struct MixosTray {
    units: Vec<Unit>,
}

impl Tray for MixosTray {
    const MENU_ON_ACTIVATE: bool = true;

    fn id(&self) -> String {
        APP_ID.into()
    }

    fn title(&self) -> String {
        "MixOS".into()
    }

    fn category(&self) -> Category {
        Category::SystemServices
    }

    fn status(&self) -> Status {
        Status::Active
    }

    fn icon_name(&self) -> String {
        "dev.mixos.tray".into()
    }

    fn tool_tip(&self) -> ToolTip {
        ToolTip {
            title: "MixOS".into(),
            description: units::summary(&self.units),
            ..Default::default()
        }
    }

    fn menu_about_to_show(&mut self) {
        self.units = units::probe_all();
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        let mut items: Vec<MenuItem<Self>> = self.units.iter().map(unit_item).collect();
        if items.is_empty() {
            items.push(disabled("No MixOS units checked"));
        }
        items.push(MenuItem::Separator);
        items.push(
            StandardItem {
                label: "Quit".into(),
                icon_name: "application-exit".into(),
                activate: Box::new(|_| std::process::exit(0)),
                ..Default::default()
            }
            .into(),
        );
        items
    }
}

fn unit_item(unit: &Unit) -> MenuItem<MixosTray> {
    disabled(format!("{}: {}", unit.name, unit.state.label()))
}

fn disabled(label: impl Into<String>) -> MenuItem<MixosTray> {
    StandardItem {
        label: label.into(),
        enabled: false,
        ..Default::default()
    }
    .into()
}

fn main() {
    let tray = MixosTray {
        units: units::probe_all(),
    };
    let handle = match tray.assume_sni_available(true).spawn() {
        Ok(handle) => handle,
        Err(error) => {
            eprintln!("tray: cannot register StatusNotifierItem: {error}");
            std::process::exit(1);
        }
    };

    // ksni runs the item on its own worker thread. Keep this process alive
    // without a timer: nothing wakes it except the channel closing.
    let (_keepalive, stopped) = mpsc::channel::<()>();
    let _handle = handle;
    let _ = stopped.recv();
}

#[cfg(test)]
mod tests {
    use super::*;
    use units::UnitState;

    #[test]
    fn unit_rows_show_name_and_state_and_are_not_clickable() {
        let unit = Unit {
            name: "webd.service",
            state: UnitState::Known("active".into()),
        };
        let item = unit_item(&unit);
        let MenuItem::Standard(item) = item else {
            panic!("unit row is not a standard item");
        };
        assert_eq!(item.label, "webd.service: active");
        assert!(!item.enabled);
    }
}
