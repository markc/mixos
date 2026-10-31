// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! `Look`: the follower view (§1.6, §5.1), what a toolkit app draws from and
//! what resolution compiles. It is composed of the look's views and reads no
//! key of its own; the accent it draws is the derivation's, from the compile.
use super::{AppLooks, Appearance, Framing, Reader, State, Tune, View};
use crate::resolve::values::{Mode, effective_mode};
use serde::{Deserialize, Serialize};

/// Serialisable for S7's `settingsd.desktop.look.<profile>` topic, in the
/// registry's own names (`comfortable`, `slightly_round`, `auto`, a swatch
/// name or `#rrggbb`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Look {
    pub appearance: Appearance,
    /// What the `auto` mode resolves from ([`Self::mode`]).
    pub state: State,
    pub framing: Framing,
    /// `accessibility.text_scale`, 0.5 to 3.0 (R5 step 7).
    pub text_scale: f64,
    /// `accessibility.reduced_motion`.
    pub reduced_motion: bool,
    pub apps: AppLooks,
    pub tune: Tune,
}

impl View for Look {
    fn read(r: &Reader) -> Self {
        Self {
            appearance: View::read(r),
            state: View::read(r),
            framing: View::read(r),
            // The two look keys of `Accessibility`, read as it reads them.
            text_scale: r.number("accessibility.text_scale"),
            reduced_motion: r.bool("accessibility.reduced_motion"),
            apps: View::read(r),
            tune: View::read(r),
        }
    }
}

impl Look {
    /// The desktop's resolved mode, never `auto`, with the key that decided
    /// it ([`effective_mode`]). The values are validated, so a name outside
    /// the registry cannot reach here; it reads as `auto` with no daylight.
    pub fn mode(&self) -> (Mode, &'static str) {
        effective_mode(&self.appearance.mode, &self.state)
            .unwrap_or((Mode::Dark, "auto_without_daylight"))
    }

    /// Roundness and density, laid over the compiled style by
    /// `design::geometry` (R5 step 2a).
    pub fn geometry(&self) -> design::geometry::Geometry {
        design::geometry::Geometry {
            roundness: self.appearance.roundness,
            density: self.appearance.density,
        }
    }
}
