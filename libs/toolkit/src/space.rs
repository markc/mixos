// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The installed spacing scale, in logical points.

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Space {
    pub xs: f32,
    pub sm: f32,
    pub md: f32,
    pub lg: f32,
    pub xl: f32,
}

pub fn of(ctx: &egui::Context) -> Space {
    let tokens = crate::chrome::Chrome::of(ctx).style;
    Space {
        xs: tokens.space_xs as f32,
        sm: tokens.space_sm as f32,
        md: tokens.space_md as f32,
        lg: tokens.space_lg as f32,
        xl: tokens.space_xl as f32,
    }
}
