// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Typed views of the authored [`Values`] (key space v2, §1.6): what the Rust
//! consumers read instead of parsing dotted paths at every use. Each view is
//! decoded from the values with the registry's defaults filled, so a key
//! that was never authored reads as its default.
//!
//! Every stored registry key is read by exactly one view, unless it is
//! `export_only` (read by the Mix exporters over the Bus) or live (never
//! stored): the coverage test below proves it by recording each read.
//!
//! The look half of S4 adds `Look`, the follower view, beside these. It is
//! composed of views that already exist here ([`Appearance`], [`Window`]'s
//! framing, [`Accessibility`], [`AppLooks`], [`Tune`]) plus what only the
//! derivation knows (the resolved accent), so it reads no key of its own.
//!
//! Views assume validated values (the store validates on load and every
//! write goes through [`registry::apply`]): a value of the wrong shape reads
//! as the default.
use crate::registry::{self, Values};
use serde_json::Value;
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

mod accessibility;
mod app_domains;
mod appearance;
mod apps;
mod displays;
mod environment;
mod input;
mod keymaps;
mod panels;
#[cfg(test)]
mod tests;
mod wallpaper;
mod window;
mod workspaces;

pub use accessibility::{Accessibility, Magnifier, Osk};
pub use app_domains::AppDomains;
pub use appearance::{Accent, AppLook, AppLooks, Appearance, Density, Frosted, State, Tune};
pub use apps::{Applications, Xwayland};
pub use displays::{Display, Displays};
pub use environment::{Power, Region, Sound, Time, TimeApplet, TimeFormat};
pub use input::{Input, Keyboard, Mouse, Tablet, Touchpad};
pub use keymaps::{Keymaps, Spawn};
pub use panels::{Applets, Panel, Panels};
pub use wallpaper::{Folders, Slideshow, Wallpaper};
pub use window::{Controls, Framing, Window};
pub use workspaces::Workspaces;

/// A view of the values: decoded once per delivery.
pub trait View: Sized {
    fn read(values: &Reader) -> Self;

    fn of(values: &Values) -> Self {
        Self::read(&Reader::new(values))
    }
}

/// Typed reads of the values, authored or default. In tests it records the
/// registry pattern of every key read, for the coverage test.
pub struct Reader<'a> {
    values: &'a Values,
    seen: Option<RefCell<Vec<&'static str>>>,
}

impl<'a> Reader<'a> {
    pub fn new(values: &'a Values) -> Self {
        Self { values, seen: None }
    }

    #[cfg(test)]
    fn recording(values: &'a Values) -> Self {
        Self {
            values,
            seen: Some(RefCell::new(Vec::new())),
        }
    }

    #[cfg(test)]
    fn seen(self) -> Vec<&'static str> {
        self.seen.map(RefCell::into_inner).unwrap_or_default()
    }

    /// The authored values.
    pub fn values(&self) -> &'a Values {
        self.values
    }

    /// Note a read of `path`'s registry entry.
    fn note(&self, path: &str) {
        if let Some(seen) = &self.seen
            && let Ok(m) = registry::lookup(path)
        {
            seen.borrow_mut().push(m.def.path);
        }
    }

    /// The value of `path`: authored, else the registry default; null where
    /// there is neither (an owner-supplied default never authored).
    pub fn value(&self, path: &str) -> Value {
        self.note(path);
        registry::effective(self.values, path).unwrap_or(Value::Null)
    }

    /// The authored value only, for keys whose default is the owner's.
    pub fn authored(&self, path: &str) -> Option<Value> {
        self.note(path);
        self.values.get(path).cloned()
    }

    /// The registry default of `path`, for a value of the wrong shape.
    fn fallback(path: &str) -> Value {
        registry::lookup(path)
            .ok()
            .and_then(|m| m.default_value())
            .unwrap_or(Value::Null)
    }

    pub fn bool(&self, path: &str) -> bool {
        self.value(path)
            .as_bool()
            .or_else(|| Self::fallback(path).as_bool())
            .unwrap_or(false)
    }

    pub fn opt_bool(&self, path: &str) -> Option<bool> {
        self.value(path).as_bool()
    }

    pub fn int(&self, path: &str) -> i64 {
        self.value(path)
            .as_i64()
            .or_else(|| Self::fallback(path).as_i64())
            .unwrap_or(0)
    }

    pub fn opt_int(&self, path: &str) -> Option<i64> {
        self.value(path).as_i64()
    }

    /// A whole number that is never negative in the registry (its kind's
    /// minimum is 0 or more).
    pub fn uint(&self, path: &str) -> u32 {
        u32::try_from(self.int(path)).unwrap_or(0)
    }

    pub fn opt_uint(&self, path: &str) -> Option<u32> {
        self.opt_int(path).and_then(|n| u32::try_from(n).ok())
    }

    pub fn number(&self, path: &str) -> f64 {
        self.value(path)
            .as_f64()
            .or_else(|| Self::fallback(path).as_f64())
            .unwrap_or(0.0)
    }

    pub fn opt_number(&self, path: &str) -> Option<f64> {
        self.value(path).as_f64()
    }

    pub fn text(&self, path: &str) -> String {
        match self.value(path) {
            Value::String(s) => s,
            _ => Self::fallback(path).as_str().unwrap_or_default().to_owned(),
        }
    }

    pub fn opt_text(&self, path: &str) -> Option<String> {
        match self.value(path) {
            Value::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn texts(&self, path: &str) -> Vec<String> {
        strings(&self.value(path))
    }

    /// The members of a pattern family present in the values: for
    /// `displays.` and segment 1, each connector with an authored key.
    pub fn members(&self, prefix: &str, segment: usize) -> BTreeSet<String> {
        self.values
            .keys()
            .filter(|k| k.starts_with(prefix))
            .filter_map(|k| k.split('.').nth(segment))
            .map(str::to_owned)
            .collect()
    }
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str().map(str::to_owned))
        .collect()
}

/// Every view of the values, decoded together.
#[derive(Clone, Debug, PartialEq)]
pub struct Views {
    pub appearance: Appearance,
    pub state: State,
    pub window: Window,
    pub panels: Panels,
    pub wallpaper: Wallpaper,
    pub workspaces: Workspaces,
    pub displays: Displays,
    pub input: Input,
    pub keymaps: Keymaps,
    pub sound: Sound,
    pub power: Power,
    pub time: Time,
    pub region: Region,
    pub accessibility: Accessibility,
    pub applications: Applications,
    pub xwayland: Xwayland,
    pub app_looks: AppLooks,
    pub tune: Tune,
    pub app_domains: AppDomains,
}

impl View for Views {
    fn read(r: &Reader) -> Self {
        Self {
            appearance: View::read(r),
            state: View::read(r),
            window: View::read(r),
            panels: View::read(r),
            wallpaper: View::read(r),
            workspaces: View::read(r),
            displays: View::read(r),
            input: View::read(r),
            keymaps: View::read(r),
            sound: View::read(r),
            power: View::read(r),
            time: View::read(r),
            region: View::read(r),
            accessibility: View::read(r),
            applications: View::read(r),
            xwayland: View::read(r),
            app_looks: View::read(r),
            tune: View::read(r),
            app_domains: View::read(r),
        }
    }
}

/// A map of concrete members, each decoded by `one`.
fn family<T>(
    r: &Reader,
    prefix: &str,
    segment: usize,
    one: impl Fn(&str) -> T,
) -> BTreeMap<String, T> {
    r.members(prefix, segment)
        .into_iter()
        .map(|name| {
            let view = one(&name);
            (name, view)
        })
        .collect()
}
