// SPDX-License-Identifier: MIT OR Apache-2.0
//! Shared native window sizing policy. Initial and saved sizes are independent.

/// Logical points, small enough for a quarter of a 1280×720 desktop with panels.
pub const MIN_SIZE: [f32; 2] = [360.0, 240.0];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_app_uses_the_shared_quarter_tile_minimum() {
        let viewport = crate::titlebar::viewport("dev.mixos.example", "Example");
        assert_eq!(viewport.min_inner_size, Some(egui::Vec2::from(MIN_SIZE)));
        let minimum = viewport.min_inner_size.unwrap();
        assert!(minimum.x <= 640.0 && minimum.y <= 320.0);
        // Guard the actual native builder call sites against app overrides.
        for source in [
            include_str!("../../../apps/ced/src/main.rs"),
            include_str!("../../../apps/dopus/src/main.rs"),
            include_str!("../../../apps/email/src/lib.rs"),
            include_str!("../../../apps/monitor/src/main.rs"),
            include_str!("../../../apps/prefs/src/main.rs"),
            include_str!("../../../apps/screenshot/src/main.rs"),
            include_str!("../../../apps/term/src/main.rs"),
            include_str!("../../../apps/workshop/src/main.rs"),
            include_str!("../../../apps/email/src/composer.rs"),
        ] {
            assert!(source.contains("titlebar::viewport("));
            assert!(!source.contains(".with_min_inner_size("));
        }
    }
}
