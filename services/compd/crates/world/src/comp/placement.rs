// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Placement is independent of saved floating geometry and fullscreen.

use smithay::utils::{Logical, Point, Rectangle};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Placement {
    #[default]
    Floating,
    Maximized {
        output: String,
    },
    Tiled {
        slot: TileSlot,
        output: String,
    },
}

impl Placement {
    pub fn output(&self) -> Option<&str> {
        match self {
            Self::Floating => None,
            Self::Maximized { output } | Self::Tiled { output, .. } => Some(output),
        }
    }

    pub fn maximized(&self) -> bool {
        matches!(self, Self::Maximized { .. })
    }

    pub fn constrained(&self) -> bool {
        !matches!(self, Self::Floating)
    }

    pub fn on_output(self, output: String) -> Self {
        match self {
            Self::Floating => Self::Floating,
            Self::Maximized { .. } => Self::Maximized { output },
            Self::Tiled { slot, .. } => Self::Tiled { slot, output },
        }
    }
}

/// One shared boundary on each axis prevents gaps on odd-sized outputs.
/// Stored per (output, workspace); stage 1a always uses the default halves.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TileSplit {
    pub x: f64,
    pub y: f64,
}

impl Default for TileSplit {
    fn default() -> Self {
        Self { x: 0.5, y: 0.5 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TileSlot {
    Left,
    Right,
    Top,
    Bottom,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

impl TileSlot {
    pub fn rectangle(
        self,
        area: Rectangle<i32, Logical>,
        split: TileSplit,
    ) -> Rectangle<i32, Logical> {
        let w = area.size.w.max(0);
        let h = area.size.h.max(0);
        let x = (f64::from(w) * split.x.clamp(0.0, 1.0)).round() as i32;
        let y = (f64::from(h) * split.y.clamp(0.0, 1.0)).round() as i32;
        let (left, right) = match self {
            Self::Left | Self::TopLeft | Self::BottomLeft => (0, x),
            Self::Right | Self::TopRight | Self::BottomRight => (x, w),
            Self::Top | Self::Bottom => (0, w),
        };
        let (top, bottom) = match self {
            Self::Top | Self::TopLeft | Self::TopRight => (0, y),
            Self::Bottom | Self::BottomLeft | Self::BottomRight => (y, h),
            Self::Left | Self::Right => (0, h),
        };
        Rectangle::new(
            area.loc + Point::from((left, top)),
            (right - left, bottom - top).into(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_eight_slots_share_rounded_boundaries_on_odd_areas() {
        let area = Rectangle::new((-7, 13).into(), (101, 79).into());
        for (slot, loc, size) in [
            (TileSlot::Left, (-7, 13), (51, 79)),
            (TileSlot::Right, (44, 13), (50, 79)),
            (TileSlot::Top, (-7, 13), (101, 40)),
            (TileSlot::Bottom, (-7, 53), (101, 39)),
            (TileSlot::TopLeft, (-7, 13), (51, 40)),
            (TileSlot::TopRight, (44, 13), (50, 40)),
            (TileSlot::BottomLeft, (-7, 53), (51, 39)),
            (TileSlot::BottomRight, (44, 53), (50, 39)),
        ] {
            assert_eq!(
                slot.rectangle(area, TileSplit::default()),
                Rectangle::new(loc.into(), size.into())
            );
        }
    }

    #[test]
    fn complementary_slots_cover_even_odd_and_empty_areas() {
        for w in 0..8 {
            for h in 0..8 {
                let area = Rectangle::new((3, -9).into(), (w, h).into());
                for split in [TileSplit::default(), TileSplit { x: 0.3, y: 0.7 }] {
                    let left = TileSlot::Left.rectangle(area, split);
                    let right = TileSlot::Right.rectangle(area, split);
                    let top = TileSlot::Top.rectangle(area, split);
                    let bottom = TileSlot::Bottom.rectangle(area, split);
                    assert_eq!(left.loc.x + left.size.w, right.loc.x);
                    assert_eq!(left.size.w + right.size.w, w);
                    assert_eq!(top.loc.y + top.size.h, bottom.loc.y);
                    assert_eq!(top.size.h + bottom.size.h, h);
                }
            }
        }
    }
}
