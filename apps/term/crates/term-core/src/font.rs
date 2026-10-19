// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Runtime font sizing: foot's `font-increase` / `font-decrease` /
//! `font-reset`, as state a frontend owns and a raster is rebuilt from.
//!
//! Toolkit-free on purpose. The chords that drive it are the frontend's
//! business (they depend on how the toolkit reports keys and wheels); the
//! arithmetic is not, and both frontends must agree on it.
//!
//! foot steps by **0.5 pt**. The terminal's configured size is in logical
//! pixels (`font_px`), so the step is 0.5 pt at the 96 dpi that logical
//! pixels are defined against: 0.5 × 96 / 72 = ⅔ px.
//!
//! The current size is held as a whole number of steps from the configured
//! size, never as a running float. Stepping up fifty times and down fifty
//! times therefore lands exactly on the configured size, and `reset` is exact
//! by construction rather than by rounding.

use crate::config::valid_font;

/// One `font-increase` / `font-decrease`: 0.5 pt, in logical pixels.
pub const STEP_PX: f32 = 0.5 * 96.0 / 72.0;

/// The configured size (the settings' value, which `reset` returns to), the
/// size zoom steps count from, and how far the user has zoomed from that.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FontSize {
    configured: f32,
    base: f32,
    steps: i32,
}

impl FontSize {
    /// `configured` is the startup size, already validated; it is what
    /// `reset` returns to until the settings name another.
    pub fn new(configured: f32) -> Self {
        Self {
            configured,
            base: configured,
            steps: 0,
        }
    }

    /// The size to rasterise at now, in logical pixels.
    pub fn current(&self) -> f32 {
        self.base + self.steps as f32 * STEP_PX
    }

    pub fn configured(&self) -> f32 {
        self.configured
    }

    /// Draw at `px` (a size the settings chose): it becomes the configured
    /// size, and zoom steps count from it. False when the size drawn does
    /// not change; out of range, nothing changes.
    pub fn set(&mut self, px: f32) -> bool {
        if !valid_font(px) {
            return false;
        }
        let changed = px != self.current();
        (self.configured, self.base, self.steps) = (px, px, 0);
        changed
    }

    /// Record `px` (the settings' size) as the size `reset` returns to,
    /// leaving the size drawn alone: a zoom not yet written back stands.
    /// Out of range, nothing changes.
    pub fn configure(&mut self, px: f32) {
        if valid_font(px) {
            self.configured = px;
        }
    }

    /// One step larger. False, and no change, when the next step would leave
    /// the range `terminal.font.size` accepts, so zoom cannot reach a size
    /// the settings could not hold.
    pub fn increase(&mut self) -> bool {
        self.step(1)
    }

    /// One step smaller; false at the lower bound.
    pub fn decrease(&mut self) -> bool {
        self.step(-1)
    }

    /// Back to the configured size. False if already there.
    pub fn reset(&mut self) -> bool {
        let changed = self.current() != self.configured;
        (self.base, self.steps) = (self.configured, 0);
        changed
    }

    /// Apply `steps` single steps in one direction (a wheel that reports
    /// several notches at once), stopping at the bound. Returns whether the
    /// size changed at all.
    pub fn step_by(&mut self, steps: i32) -> bool {
        let mut changed = false;
        for _ in 0..steps.unsigned_abs() {
            if !self.step(steps.signum()) {
                break;
            }
            changed = true;
        }
        changed
    }

    fn step(&mut self, direction: i32) -> bool {
        let Some(next) = self.steps.checked_add(direction) else {
            return false;
        };
        if !valid_font(self.base + next as f32 * STEP_PX) {
            return false;
        }
        self.steps = next;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reset_preserves_fractional_config_and_design_default() {
        for configured in [21.333, crate::config::Config::default().font_px] {
            let mut size = FontSize::new(configured);
            assert!(size.step_by(3));
            assert!(size.reset());
            assert_eq!(size.current(), configured);
        }
    }

    #[test]
    fn a_step_is_half_a_point_at_96_dpi() {
        let mut size = FontSize::new(13.0);
        assert!(size.increase());
        assert!((size.current() - (13.0 + 2.0 / 3.0)).abs() < 1e-5);
        assert!(size.decrease());
        assert!(size.decrease());
        assert!((size.current() - (13.0 - 2.0 / 3.0)).abs() < 1e-5);
    }

    /// Held as steps, not as a float that accumulates: many round trips must
    /// land on the configured size EXACTLY, or `reset` and "back where I
    /// started" would disagree by a rounding error and re-rasterise.
    #[test]
    fn round_trips_are_exact_and_reset_returns_to_the_configured_size() {
        let mut size = FontSize::new(13.0);
        for _ in 0..20 {
            assert!(size.increase());
        }
        for _ in 0..20 {
            assert!(size.decrease());
        }
        assert_eq!(size.current(), 13.0);
        assert!(!size.reset(), "already at the configured size");

        size.step_by(7);
        assert_ne!(size.current(), 13.0);
        assert!(size.reset());
        assert_eq!(size.current(), 13.0);
        assert_eq!(size.configured(), 13.0);
    }

    /// A size the settings chose is where zoom steps count from, and where
    /// reset returns: the profile's size, not the startup size.
    #[test]
    fn a_set_size_is_the_base_for_steps_and_the_reset_target() {
        let mut size = FontSize::new(12.5);
        assert!(size.increase());
        assert!(size.set(14.0));
        assert_eq!(size.current(), 14.0);
        assert_eq!(size.configured(), 14.0);
        assert!(!size.set(14.0), "already current");
        assert!(!size.set(5.0) && !size.set(f32::NAN), "out of range");
        assert_eq!(size.configured(), 14.0);
        assert!(size.increase());
        assert!((size.current() - (14.0 + STEP_PX)).abs() < 1e-5);
        assert!(size.decrease());
        assert_eq!(size.current(), 14.0);
        assert!(size.step_by(3));
        assert!(size.reset());
        assert_eq!(size.current(), 14.0, "the profile's size");
        assert!(!size.reset());

        // The settings landing on the size already drawn (a zoom written
        // back) make it the reset target without redrawing.
        assert!(size.step_by(2));
        let zoomed = size.current();
        assert!(!size.set(zoomed));
        assert_eq!(size.configured(), zoomed);
        assert!(!size.reset());
    }

    /// `configure` moves the reset target and leaves the drawn size (a zoom
    /// not yet written back) alone.
    #[test]
    fn configure_moves_only_the_reset_target() {
        let mut size = FontSize::new(12.5);
        assert!(size.step_by(2));
        let zoomed = size.current();
        size.configure(16.0);
        assert_eq!((size.current(), size.configured()), (zoomed, 16.0));
        size.configure(100.0);
        assert_eq!(size.configured(), 16.0, "out of range is ignored");
        assert!(size.reset());
        assert_eq!(size.current(), 16.0);
    }

    /// The bounds are the settings' own: zoom must not reach a size that
    /// `terminal.font.size` would be refused for.
    #[test]
    fn stepping_stops_inside_the_configured_range() {
        let mut size = FontSize::new(47.5);
        assert!(!size.increase(), "47.5 + 2/3 is past 48");
        assert_eq!(size.current(), 47.5);

        let mut size = FontSize::new(6.5);
        assert!(!size.decrease(), "6.5 - 2/3 is below 6");
        assert_eq!(size.current(), 6.5);

        // A multi-notch wheel stops AT the bound, not before it, and still
        // reports the steps it did take.
        let mut size = FontSize::new(13.0);
        assert!(size.step_by(-100));
        assert!(valid_font(size.current()));
        assert!(!valid_font(size.current() - STEP_PX));
        assert!(!size.step_by(-1));
        assert!(!size.step_by(0));
    }
}
