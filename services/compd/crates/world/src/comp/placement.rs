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
    pub fn name(&self) -> &'static str {
        match self {
            Self::Floating => "floating",
            Self::Maximized { .. } => "maximized",
            Self::Tiled { slot, .. } => slot.name(),
        }
    }

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
    pub const ALL: [Self; 8] = [
        Self::Left,
        Self::Right,
        Self::Top,
        Self::Bottom,
        Self::TopLeft,
        Self::TopRight,
        Self::BottomLeft,
        Self::BottomRight,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Right => "right",
            Self::Top => "top",
            Self::Bottom => "bottom",
            Self::TopLeft => "top_left",
            Self::TopRight => "top_right",
            Self::BottomLeft => "bottom_left",
            Self::BottomRight => "bottom_right",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|slot| slot.name() == name)
    }

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

/// A crossing mirrors the slot on the crossed axis. No arrow restores floating.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TileTransition {
    Slot(TileSlot),
    Cross {
        direction: comp_model::request::TileDirection,
        slot: TileSlot,
    },
}

pub fn tile_transition(
    current: &Placement,
    key: comp_model::request::TileDirection,
) -> TileTransition {
    use TileSlot::{
        Bottom as B, BottomLeft as BL, BottomRight as BR, Left as L, Right as R, Top as T,
        TopLeft as TL, TopRight as TR,
    };
    use TileTransition::{Cross, Slot};
    use comp_model::request::TileDirection::{Down, Left, Right, Up};
    let Placement::Tiled { slot, .. } = current else {
        return Slot(match key {
            Left => L,
            Right => R,
            Up => T,
            Down => B,
        });
    };
    match (*slot, key) {
        (L, Left) => Cross {
            direction: Left,
            slot: R,
        },
        (R, Right) => Cross {
            direction: Right,
            slot: L,
        },
        (T, Up) => Cross {
            direction: Up,
            slot: B,
        },
        (B, Down) => Cross {
            direction: Down,
            slot: T,
        },
        (TL, Left) => Cross {
            direction: Left,
            slot: TR,
        },
        (TL, Up) => Cross {
            direction: Up,
            slot: BL,
        },
        (TR, Right) => Cross {
            direction: Right,
            slot: TL,
        },
        (TR, Up) => Cross {
            direction: Up,
            slot: BR,
        },
        (BL, Left) => Cross {
            direction: Left,
            slot: BR,
        },
        (BL, Down) => Cross {
            direction: Down,
            slot: TL,
        },
        (BR, Right) => Cross {
            direction: Right,
            slot: BL,
        },
        (BR, Down) => Cross {
            direction: Down,
            slot: TR,
        },
        (L, Right) => Slot(R),
        (L, Up) => Slot(TL),
        (L, Down) => Slot(BL),
        (R, Left) => Slot(L),
        (R, Up) => Slot(TR),
        (R, Down) => Slot(BR),
        (T, Left) => Slot(TL),
        (T, Right) => Slot(TR),
        (T, Down) => Slot(B),
        (B, Left) => Slot(BL),
        (B, Right) => Slot(BR),
        (B, Up) => Slot(T),
        (TL | TR, Down) => Slot(if *slot == TL { L } else { R }),
        (TL, Right) | (TR, Left) => Slot(T),
        (BL | BR, Up) => Slot(if *slot == BL { L } else { R }),
        (BL, Right) | (BR, Left) => Slot(B),
    }
}

/// Nearest output in the requested direction, from full output rectangles.
/// Prefer outputs overlapping the perpendicular axis, then the closest centre.
pub fn adjacent_output<'a>(
    current: Rectangle<i32, Logical>,
    direction: comp_model::request::TileDirection,
    outputs: impl Iterator<Item = (&'a str, Rectangle<i32, Logical>)>,
) -> Option<&'a str> {
    use comp_model::request::TileDirection::{Down, Left, Right, Up};
    let centre = |r: Rectangle<i32, Logical>| {
        (
            i64::from(r.loc.x) * 2 + i64::from(r.size.w),
            i64::from(r.loc.y) * 2 + i64::from(r.size.h),
        )
    };
    let (cx, cy) = centre(current);
    outputs
        .filter_map(|(name, area)| {
            let (x, y) = centre(area);
            let horizontal = matches!(direction, Left | Right);
            let ahead = match direction {
                Left => x < cx,
                Right => x > cx,
                Up => y < cy,
                Down => y > cy,
            };
            if !ahead {
                return None;
            }
            let overlap = if horizontal {
                i64::from(area.loc.y) < i64::from(current.loc.y) + i64::from(current.size.h)
                    && i64::from(current.loc.y) < i64::from(area.loc.y) + i64::from(area.size.h)
            } else {
                i64::from(area.loc.x) < i64::from(current.loc.x) + i64::from(current.size.w)
                    && i64::from(current.loc.x) < i64::from(area.loc.x) + i64::from(area.size.w)
            };
            let dx = i128::from(x - cx);
            let dy = i128::from(y - cy);
            Some(((!overlap, dx * dx + dy * dy, name), name))
        })
        .min_by_key(|(score, _)| *score)
        .map(|(_, name)| name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyboard_transition_all_forty_cells() {
        use TileSlot::{
            Bottom as B, BottomLeft as BL, BottomRight as BR, Left as L, Right as R, Top as T,
            TopLeft as TL, TopRight as TR,
        };
        use TileTransition::{Cross as C, Slot as S};
        use comp_model::request::TileDirection::{Down, Left, Right, Up};
        let rows = [
            (Placement::Floating, [S(L), S(R), S(T), S(B)]),
            (
                Placement::Maximized { output: "a".into() },
                [S(L), S(R), S(T), S(B)],
            ),
            (
                Placement::Tiled {
                    slot: L,
                    output: "a".into(),
                },
                [
                    C {
                        direction: Left,
                        slot: R,
                    },
                    S(R),
                    S(TL),
                    S(BL),
                ],
            ),
            (
                Placement::Tiled {
                    slot: R,
                    output: "a".into(),
                },
                [
                    S(L),
                    C {
                        direction: Right,
                        slot: L,
                    },
                    S(TR),
                    S(BR),
                ],
            ),
            (
                Placement::Tiled {
                    slot: T,
                    output: "a".into(),
                },
                [
                    S(TL),
                    S(TR),
                    C {
                        direction: Up,
                        slot: B,
                    },
                    S(B),
                ],
            ),
            (
                Placement::Tiled {
                    slot: B,
                    output: "a".into(),
                },
                [
                    S(BL),
                    S(BR),
                    S(T),
                    C {
                        direction: Down,
                        slot: T,
                    },
                ],
            ),
            (
                Placement::Tiled {
                    slot: TL,
                    output: "a".into(),
                },
                [
                    C {
                        direction: Left,
                        slot: TR,
                    },
                    S(T),
                    C {
                        direction: Up,
                        slot: BL,
                    },
                    S(L),
                ],
            ),
            (
                Placement::Tiled {
                    slot: TR,
                    output: "a".into(),
                },
                [
                    S(T),
                    C {
                        direction: Right,
                        slot: TL,
                    },
                    C {
                        direction: Up,
                        slot: BR,
                    },
                    S(R),
                ],
            ),
            (
                Placement::Tiled {
                    slot: BL,
                    output: "a".into(),
                },
                [
                    C {
                        direction: Left,
                        slot: BR,
                    },
                    S(B),
                    S(L),
                    C {
                        direction: Down,
                        slot: TL,
                    },
                ],
            ),
            (
                Placement::Tiled {
                    slot: BR,
                    output: "a".into(),
                },
                [
                    S(B),
                    C {
                        direction: Right,
                        slot: BL,
                    },
                    S(R),
                    C {
                        direction: Down,
                        slot: TR,
                    },
                ],
            ),
        ];
        for (placement, expected) in rows {
            for (key, expected) in [Left, Right, Up, Down].into_iter().zip(expected) {
                assert_eq!(
                    tile_transition(&placement, key),
                    expected,
                    "{placement:?} {key:?}"
                );
            }
        }
    }

    #[test]
    fn adjacent_outputs_follow_geometry_on_both_axes() {
        use comp_model::request::TileDirection::{Down, Left, Right, Up};
        let centre = Rectangle::new((0, 0).into(), (1920, 1080).into());
        for (key, reverse, loc) in [
            (Left, Right, (-1920, 0)),
            (Right, Left, (1920, 0)),
            (Up, Down, (0, -1080)),
            (Down, Up, (0, 1080)),
        ] {
            let other = Rectangle::new(loc.into(), centre.size);
            assert_eq!(
                adjacent_output(centre, key, [("a", centre), ("b", other)].into_iter()),
                Some("b")
            );
            assert_eq!(
                adjacent_output(other, reverse, [("a", centre), ("b", other)].into_iter()),
                Some("a")
            );
            assert_eq!(
                adjacent_output(centre, reverse, [("a", centre), ("b", other)].into_iter()),
                None
            );
        }
    }

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
