// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Power & battery (spec 1 §4.8). The battery and the devices are read from
//! UPower by hostd; the power mode goes to hostd's power-profiles bridge; the
//! screen-off timer is compd's idle and DPMS; the automatic suspends are
//! hostd's idle and suspend. Each row waits on its own backend, so each names
//! it on its own row, and no write runs until that backend lands.
use super::{ChoiceStyle, Control, Lit, Opt, Page, row, section};

const PROFILES: &[Opt] = &[
    super::opt("battery", "opt-profile-battery"),
    super::opt("balanced", "opt-profile-balanced"),
    super::opt("performance", "opt-profile-performance"),
];

/// A timer in seconds, with its label.
const fn after(seconds: i64, label: &'static str) -> Opt {
    Opt {
        value: Lit::I(seconds),
        label,
    }
}

/// Null is the registry's unset value: the timer never fires.
const NEVER: Opt = Opt {
    value: Lit::Null,
    label: "opt-never",
};

const SCREEN_OFF: &[Opt] = &[
    after(120, "opt-minutes-2"),
    after(300, "opt-minutes-5"),
    after(600, "opt-minutes-10"),
    after(900, "opt-minutes-15"),
    after(1800, "opt-minutes-30"),
    NEVER,
];

/// The suspend list: 15 minutes to 2 hours on the registry's 5-minute grid.
const SUSPEND: &[Opt] = &[
    after(900, "opt-minutes-15"),
    after(1200, "opt-minutes-20"),
    after(1500, "opt-minutes-25"),
    after(1800, "opt-minutes-30"),
    after(2700, "opt-minutes-45"),
    after(3600, "opt-hour-1"),
    after(4800, "opt-minutes-80"),
    after(5400, "opt-minutes-90"),
    after(6000, "opt-minutes-100"),
    after(7200, "opt-hours-2"),
    NEVER,
];

pub(super) fn page() -> Page {
    super::page(
        "power",
        "battery-medium",
        None,
        super::sections(vec![
            section(
                "battery",
                vec![row("charge", Control::Info).backend("backend-hostd-upower")],
            ),
            section(
                "devices",
                vec![row("devices", Control::Info).backend("backend-hostd-upower")],
            ),
            section(
                "mode",
                vec![
                    row(
                        "profile",
                        Control::Choice {
                            key: "power.profile",
                            style: ChoiceStyle::Radio,
                            options: PROFILES,
                        },
                    )
                    .backend("backend-hostd-power-profiles"),
                ],
            ),
            section(
                "saving",
                vec![
                    row(
                        "screen_off",
                        Control::Choice {
                            key: "power.screen_off_s",
                            style: ChoiceStyle::Dropdown,
                            options: SCREEN_OFF,
                        },
                    )
                    .backend("backend-compd-idle"),
                    row(
                        "suspend_ac",
                        Control::Choice {
                            key: "power.suspend_ac_s",
                            style: ChoiceStyle::Dropdown,
                            options: SUSPEND,
                        },
                    )
                    .backend("backend-hostd-suspend"),
                    row(
                        "suspend_battery",
                        Control::Choice {
                            key: "power.suspend_battery_s",
                            style: ChoiceStyle::Dropdown,
                            options: SUSPEND,
                        },
                    )
                    .backend("backend-hostd-suspend"),
                ],
            ),
        ]),
    )
}
