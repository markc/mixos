// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! `workspaces.*`: compd's workspaces and overview.
use super::{Reader, View};

#[derive(Clone, Debug, PartialEq)]
pub struct Workspaces {
    /// `span` or `per_display`.
    pub mode: String,
    /// `horizontal` or `vertical`.
    pub orientation: String,
    /// `none`, `launcher` or `applications`.
    pub overview_typing: String,
    pub wraparound: bool,
}

impl View for Workspaces {
    fn read(r: &Reader) -> Self {
        Self {
            mode: r.text("workspaces.mode"),
            orientation: r.text("workspaces.orientation"),
            overview_typing: r.text("workspaces.overview_typing"),
            wraparound: r.bool("workspaces.wraparound"),
        }
    }
}
