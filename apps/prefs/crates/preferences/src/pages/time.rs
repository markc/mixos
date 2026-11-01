// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Time & language (spec 1 §4.11; the matrix §10). Date & time: the zone
//! and NTP are hostd's timedated, so their rows wait on it; the clock format
//! and the panel clock are settingsd keys, live now. Region & language: the
//! languages and the formats are hostd's locale1, so the whole page waits on
//! it, and the one reason is said once under its title.
use super::{
    Body, ChoiceStyle, Control, Drawer, Opt, Page, PickList, opt, row, section, untitled,
};

const FIRST_DAY: &[Opt] = &[
    opt("friday", "opt-friday"),
    opt("saturday", "opt-saturday"),
    opt("sunday", "opt-sunday"),
    opt("monday", "opt-monday"),
];

pub(super) fn date_time() -> Page {
    let applet = Drawer {
        id: "applet",
        sections: vec![untitled(
            "applet",
            vec![
                row(
                    "show_date",
                    Control::Toggle {
                        key: "time.applet.show_date",
                    },
                ),
                row(
                    "show_weekday",
                    Control::Toggle {
                        key: "time.applet.show_weekday",
                    },
                ),
                row(
                    "show_seconds",
                    Control::Toggle {
                        key: "time.applet.show_seconds",
                    },
                ),
            ],
        )],
    };
    super::page(
        "date_time",
        "calendar-clock",
        Some("time"),
        Body::Sections {
            sections: vec![
                untitled(
                    "now",
                    vec![row("now", Control::Info).value_from("time.now")],
                ),
                untitled(
                    "zone",
                    vec![
                        row(
                            "zone",
                            Control::Pick {
                                key: "time.zone",
                                list: PickList::TimeZones,
                            },
                        )
                        .backend("backend-hostd-timedated"),
                        row("ntp", Control::Toggle { key: "time.ntp" })
                            .backend("backend-hostd-timedated"),
                    ],
                ),
                untitled(
                    "format",
                    vec![
                        row(
                            "24h",
                            Control::Toggle {
                                key: "time.format.24h",
                            },
                        ),
                        row(
                            "first_day",
                            Control::Choice {
                                key: "time.format.first_day",
                                style: ChoiceStyle::Dropdown,
                                options: FIRST_DAY,
                            },
                        ),
                        row("applet", Control::Drawer { drawer: "applet" }),
                    ],
                ),
            ],
            drawers: vec![applet],
        },
    )
}

pub(super) fn region() -> Page {
    let add_language = Drawer {
        id: "add_language",
        sections: vec![untitled(
            "add",
            vec![row(
                "add",
                Control::Append {
                    key: "region.languages",
                    source: "locales",
                    more: None,
                },
            )],
        )],
    };
    let mut region = super::page(
        "region",
        "languages",
        Some("time"),
        Body::Sections {
            sections: vec![
                section(
                    "languages",
                    vec![
                        row("order", Control::Info),
                        row(
                            "languages",
                            Control::Ordered {
                                key: "region.languages",
                                sources: &["locales"],
                                add: Some("add_language"),
                            },
                        ),
                    ],
                ),
                section(
                    "formatting",
                    vec![row(
                        "formats",
                        Control::Pick {
                            key: "region.formats",
                            list: PickList::Locales,
                        },
                    )],
                ),
            ],
            drawers: vec![add_language],
        },
    );
    region.backend = Some("backend-hostd-locale1");
    region
}
