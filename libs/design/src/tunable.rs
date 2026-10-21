// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Metadata for the writable style and chrome vocabulary.

use serde::Serialize;

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(untagged)]
pub enum TunableRange {
    Number { min: f64, max: f64, step: f64 },
    Choice { variants: Vec<String> },
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Tunable {
    pub name: String,
    pub kind: &'static str,
    pub range: Option<TunableRange>,
    pub since: &'static str,
    pub aliases: Vec<String>,
    pub retired: Option<&'static str>,
    pub domains: Vec<&'static str>,
    pub group: String,
    pub order: usize,
    pub label: String,
}

pub(crate) fn style_entry(name: &str, ty: &str, min: f64, max: f64, variants: Option<Vec<String>>) -> Tunable {
    let kind = if variants.is_some() { "choice" } else {
        match ty { "bool" => "flag", "u8" => "radius", "StyleColour" => "colour", _ => "number" }
    };
    let range = if let Some(variants) = variants { Some(TunableRange::Choice { variants }) }
        else if kind == "flag" { None }
        else { Some(TunableRange::Number { min, max, step: if ty == "u8" { 1.0 } else { 0.01 } }) };
    let domains = match name {
        "resize_edge" | "resize_corner" | "scrub_rate" | "fine_scrub_rate" | "double_click_time" => vec!["input"],
        "fine_span" => vec!["input", "text", "layout"],
        "hot_distance" => vec!["input", "paint"],
        "icon_mask" => vec!["paint", "resources"],
        "bevels" | "canvas_dots" => vec!["paint"],
        "widgets" | "faces" | "panel_groups" | "document_tabs" => vec!["paint", "text", "layout", "resources"],
        "animation_time" | "tooltip_delay" | "tooltip_grace" | "menu_scroll_speed" => vec!["motion"],
        _ if name.starts_with("md_") || name.ends_with("_size") => vec!["text", "layout"],
        _ if kind == "colour" || name.contains("alpha") || name.ends_with("_mix") => vec!["paint"],
        _ if name.contains("radius") => vec!["layout", "paint"],
        _ => vec!["layout"],
    };
    let group = if name.starts_with("field_") || name.starts_with("value_") { "fields" }
        else if name.starts_with("scroll_") || name.starts_with("canvas_") { "scroll" }
        else if name.starts_with("md_") { "markdown" } else { "style" };
    Tunable {
        name: format!("style.{name}"), kind, range, since: "0.9.0",
        aliases: if name == "field_radius" { vec!["style.fieldradius".into()] } else { Vec::new() },
        retired: None, domains, group: group.into(), order: 0,
        label: format!("tune-{}", name.replace('_', "-")),
    }
}

pub fn tunables() -> &'static [Tunable] {
    static ENTRIES: std::sync::OnceLock<Vec<Tunable>> = std::sync::OnceLock::new();
    ENTRIES.get_or_init(|| {
        let mut entries = crate::family::style::declarations();
        entries.extend(crate::family::chrome::ROLES.iter().map(|role| Tunable {
            name: format!("chrome.{role}"), kind: "colour", range: None, since: "0.9.0",
            aliases: Vec::new(), retired: None, domains: vec!["paint"], group: "colours".into(),
            order: 0, label: format!("tune-{}", role.replace('_', "-")),
        }));
        for (order, entry) in entries.iter_mut().enumerate() { entry.order = order; }
        entries
    })
}
