// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>

//! Reserved property event topic names shared by the router and dispatcher.

pub fn records_changed_topic(service: &str) -> String {
    format!("{service}.props.records.changed")
}

pub fn audit_topic(service: &str) -> String {
    format!("{service}.props.audit")
}
