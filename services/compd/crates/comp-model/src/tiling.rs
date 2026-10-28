// SPDX-License-Identifier: MIT OR Apache-2.0
//! Live drag-tiling configuration and the shared props write boundary.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::observation::{PropValue, SetValidationError, invalid_value};

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TilingConfig {
    pub edge_tiling: bool,
    pub top_maximize: bool,
    pub corner_ratio: f64,
    pub side_band_px: f64,
    pub top_band_px: f64,
    pub shared_edge_dwell_ms: u64,
    pub preview_delay_ms: u64,
    pub preview_animation_ms: u64,
}

impl Default for TilingConfig {
    fn default() -> Self {
        Self {
            edge_tiling: true,
            top_maximize: true,
            corner_ratio: 0.25,
            side_band_px: 20.0,
            top_band_px: 5.0,
            shared_edge_dwell_ms: 250,
            preview_delay_ms: 0,
            preview_animation_ms: 180,
        }
    }
}

impl TilingConfig {
    pub const LEAVES: [&str; 8] = [
        "edge_tiling", "top_maximize", "corner_ratio", "side_band_px", "top_band_px",
        "shared_edge_dwell_ms", "preview_delay_ms", "preview_animation_ms",
    ];

    pub fn value(self, leaf: &str) -> Option<PropValue> {
        Some(match leaf {
            "edge_tiling" => PropValue::Bool(self.edge_tiling),
            "top_maximize" => PropValue::Bool(self.top_maximize),
            "corner_ratio" => PropValue::F64(self.corner_ratio),
            "side_band_px" => PropValue::F64(self.side_band_px),
            "top_band_px" => PropValue::F64(self.top_band_px),
            "shared_edge_dwell_ms" => PropValue::U64(self.shared_edge_dwell_ms),
            "preview_delay_ms" => PropValue::U64(self.preview_delay_ms),
            "preview_animation_ms" => PropValue::U64(self.preview_animation_ms),
            _ => return None,
        })
    }

    /// Validate before mutation, both at Bus admission and in the host.
    pub fn set(&mut self, path: &str, value: &Value) -> Result<(PropValue, PropValue), SetValidationError> {
        let leaf = path.strip_prefix("tiling.").ok_or(SetValidationError::UnknownPath)?;
        let old = self.value(leaf).ok_or(SetValidationError::UnknownPath)?;
        let boolean = || value.as_bool().ok_or_else(|| invalid_value(path, "bool", "true|false"));
        let integer = |min, max, range| value.as_u64().filter(|v| (min..=max).contains(v))
            .ok_or_else(|| invalid_value(path, "integer", range));
        match leaf {
            "edge_tiling" => self.edge_tiling = boolean()?,
            "top_maximize" => self.top_maximize = boolean()?,
            "corner_ratio" => self.corner_ratio = value.as_f64()
                .filter(|v| v.is_finite() && (0.05..=0.5).contains(v))
                .ok_or_else(|| invalid_value(path, "finite number", "0.05..=0.5"))?,
            "side_band_px" | "top_band_px" => {
                let number = value.as_f64().filter(|v| v.is_finite() && (1.0..=200.0).contains(v))
                    .ok_or_else(|| invalid_value(path, "finite number", "1..=200"))?;
                if leaf == "side_band_px" { self.side_band_px = number; }
                else { self.top_band_px = number; }
            }
            "shared_edge_dwell_ms" => self.shared_edge_dwell_ms = integer(0, 2000, "0..=2000")?,
            "preview_delay_ms" => self.preview_delay_ms = integer(0, 2000, "0..=2000")?,
            "preview_animation_ms" => self.preview_animation_ms = integer(0, 1000, "0..=1000")?,
            _ => unreachable!(),
        }
        Ok((old, self.value(leaf).expect("validated leaf")))
    }

    /// Older files use defaults; invalid individual saved leaves do too.
    pub fn from_saved(saved: &serde_json::Map<String, Value>) -> Self {
        let mut config = Self::default();
        for (leaf, value) in saved {
            let _ = config.set(&format!("tiling.{leaf}"), value);
        }
        config
    }
}
