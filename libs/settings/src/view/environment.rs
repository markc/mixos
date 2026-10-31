// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The stored halves of `sound.*` and `power.*` (their devices and levels
//! are live, hostd's and the audio service's), and `time.*`, `region.*`.
use super::{Reader, View};

/// `sound.*`'s two settings.
#[derive(Clone, Debug, PartialEq)]
pub struct Sound {
    pub output_amplify: bool,
    pub input_amplify: bool,
}

impl View for Sound {
    fn read(r: &Reader) -> Self {
        Self {
            output_amplify: r.bool("sound.output.amplify"),
            input_amplify: r.bool("sound.input.amplify"),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Power {
    /// `battery`, `balanced` or `performance`.
    pub profile: String,
    /// Seconds; None is never.
    pub screen_off_s: Option<u32>,
    pub suspend_ac_s: Option<u32>,
    pub suspend_battery_s: Option<u32>,
}

impl View for Power {
    fn read(r: &Reader) -> Self {
        Self {
            profile: r.text("power.profile"),
            screen_off_s: r.opt_uint("power.screen_off_s"),
            suspend_ac_s: r.opt_uint("power.suspend_ac_s"),
            suspend_battery_s: r.opt_uint("power.suspend_battery_s"),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Time {
    /// The IANA zone; None until one is authored (the migration takes the
    /// host's).
    pub zone: Option<String>,
    pub ntp: bool,
    pub format: TimeFormat,
    pub applet: TimeApplet,
}

/// None follows the locale.
#[derive(Clone, Debug, PartialEq)]
pub struct TimeFormat {
    pub h24: Option<bool>,
    /// `monday`, `friday`, `saturday` or `sunday`.
    pub first_day: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TimeApplet {
    pub show_date: bool,
    pub show_weekday: bool,
    pub show_seconds: bool,
}

impl View for Time {
    fn read(r: &Reader) -> Self {
        Self {
            zone: r
                .authored("time.zone")
                .and_then(|v| v.as_str().map(str::to_owned)),
            ntp: r.bool("time.ntp"),
            format: TimeFormat {
                h24: r.opt_bool("time.format.24h"),
                first_day: r.opt_text("time.format.first_day"),
            },
            applet: TimeApplet {
                show_date: r.bool("time.applet.show_date"),
                show_weekday: r.bool("time.applet.show_weekday"),
                show_seconds: r.bool("time.applet.show_seconds"),
            },
        }
    }
}

/// None until authored: the migration takes the host's.
#[derive(Clone, Debug, PartialEq)]
pub struct Region {
    pub languages: Option<Vec<String>>,
    pub formats: Option<String>,
}

impl View for Region {
    fn read(r: &Reader) -> Self {
        Self {
            languages: r.authored("region.languages").map(|v| super::strings(&v)),
            formats: r
                .authored("region.formats")
                .and_then(|v| v.as_str().map(str::to_owned)),
        }
    }
}
