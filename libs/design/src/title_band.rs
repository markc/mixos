// SPDX-License-Identifier: MIT OR Apache-2.0
//! Renderer-independent title band geometry. Inputs are in edge stacking order.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Alignment {
    Left,
    #[default]
    Center,
    Right,
}

impl Alignment {
    pub const fn name(self) -> &'static str {
        match self { Self::Left => "left", Self::Center => "center", Self::Right => "right" }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Settings {
    pub title: crate::TitleAlign,
    pub actions: crate::ActionsAlign,
    pub place: crate::ActionsPlace,
    pub style: crate::WindowStyle,
}

impl Settings {
    pub fn from_names(title: &str, actions: &str, place: &str, style: &str) -> Self {
        Self {
            title: crate::TitleAlign::resolve(title), actions: crate::ActionsAlign::resolve(actions),
            place: crate::ActionsPlace::resolve(place), style: crate::WindowStyle::resolve(style),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Group {
    pub preferred: f32,
    pub minimum: f32,
    pub align: Alignment,
    /// Lower values give up space first; None means fixed width.
    pub shrink: Option<u8>,
    /// Reading order within the centred pack.
    pub order: u8,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub width: f32,
}

/// Controls, menu, actions, title are passed in that order. The returned
/// rectangles have the same indices. Fixed groups retain their footprint even
/// when the band is narrower than their combined minimum.
pub fn layout(width: f32, groups: &[Group]) -> Vec<Rect> {
    let width = width.max(0.0);
    let mut widths: Vec<f32> = groups.iter().map(|g| g.preferred.max(g.minimum).max(0.0)).collect();
    let mut excess = (widths.iter().sum::<f32>() - width).max(0.0);
    let mut shrinking: Vec<usize> = (0..groups.len()).filter(|&i| groups[i].shrink.is_some()).collect();
    shrinking.sort_by_key(|&i| groups[i].shrink);
    for i in shrinking {
        let take = excess.min((widths[i] - groups[i].minimum.max(0.0)).max(0.0));
        widths[i] -= take;
        excess -= take;
    }
    let mut result = vec![Rect::default(); groups.len()];
    let (mut left, mut right) = (0.0, width);
    for (i, g) in groups.iter().enumerate() {
        match g.align {
            Alignment::Left => {
                result[i] = Rect { x: left, width: widths[i] };
                left += widths[i];
            }
            Alignment::Right => {
                right -= widths[i];
                result[i] = Rect { x: right, width: widths[i] };
            }
            Alignment::Center => {}
        }
    }
    let mut centre: Vec<usize> = (0..groups.len()).filter(|&i| groups[i].align == Alignment::Center).collect();
    centre.sort_by_key(|&i| groups[i].order);
    let packed: f32 = centre.iter().map(|&i| widths[i]).sum();
    let full = (width - packed) / 2.0;
    let mut x = if full >= left && full + packed <= right {
        full
    } else {
        left + ((right - left - packed) / 2.0).max(0.0)
    };
    for i in centre {
        result[i] = Rect { x, width: widths[i] };
        x += widths[i];
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    fn groups(aligns: [Alignment; 4]) -> [Group; 4] {
        std::array::from_fn(|i| Group {
            preferred: [90.0, 30.0, 150.0, 200.0][i],
            minimum: [90.0, 30.0, 60.0, 0.0][i],
            align: aligns[i], shrink: [None, None, Some(1), Some(0)][i],
            order: [3, 0, 2, 1][i],
        })
    }
    #[test]
    fn every_combination_is_disjoint() {
        let sides = [Alignment::Left, Alignment::Center, Alignment::Right];
        for a in sides { for b in sides { for c in sides { for d in sides {
            let out = layout(800.0, &groups([a,b,c,d]));
            for (i, r) in out.iter().enumerate() {
                assert!(r.x >= 0.0 && r.x + r.width <= 800.0);
                for s in &out[i+1..] { assert!(r.x + r.width <= s.x || s.x + s.width <= r.x); }
            }
        }}}}
    }
    #[test]
    fn overflow_shrinks_title_then_actions() {
        let g = groups([Alignment::Left; 4]);
        let out = layout(300.0, &g);
        assert_eq!(out[3].width, 30.0);
        assert_eq!(out[2].width, 150.0);
        let out = layout(200.0, &g);
        assert_eq!(out[3].width, 0.0);
        assert_eq!(out[2].width, 80.0);
        assert_eq!(out[0].width, 90.0);
        assert_eq!(out[1].width, 30.0);
    }
    #[test]
    fn mac_title_centres_on_full_band_then_free_gap() {
        let g = groups([Alignment::Left, Alignment::Left, Alignment::Right, Alignment::Center]);
        let out = layout(800.0, &g);
        assert_eq!(out[3].x + out[3].width / 2.0, 400.0);
        let out = layout(480.0, &g);
        assert_eq!(out[3].x + out[3].width / 2.0, 225.0);
    }
}
