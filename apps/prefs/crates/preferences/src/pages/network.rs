// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Network & wireless (spec 1 §4.14; the matrix §15). MixOS runs no
//! NetworkManager: networking is hostd's. Wi-Fi is the pattern for a page
//! whose backend MixOS lacks: shown, its controls disabled, a line naming
//! the backend (hostd Wi-Fi: scan, connect, forget, share), no error
//! dialog.
use super::{Body, Control, Extra, Page, row, section, untitled};

pub(super) fn network() -> Page {
    super::page("network", "wifi", None, Body::List)
}

pub(super) fn children() -> Vec<Page> {
    let mut wifi = super::page(
        "wifi",
        "wifi",
        Some("network"),
        Body::Sections {
            sections: vec![
                untitled(
                    "radio",
                    vec![row(
                        "enabled",
                        Control::Live {
                            key: "network.wifi.enabled",
                        },
                    )],
                ),
                section("known", vec![row("known_none", Control::Info)]),
                section("visible", vec![row("visible_none", Control::Info)]),
            ],
            drawers: Vec::new(),
        },
    );
    wifi.backend = Some("backend-hostd-wifi");
    wifi.extras = vec![Extra {
        id: "add_network",
        command: None,
        link: true,
        backend: Some("backend-hostd-wifi"),
    }];
    vec![
        super::page("vpn", "shield", Some("network"), Body::Pending),
        wifi,
        super::page("wired", "ethernet-port", Some("network"), Body::Pending),
    ]
}
