// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Where a resolved scene mounts: an edge page, dialog seat or toast seat.

use scene::ResolvedScene;
use serde_json::{Value, json};

use crate::seat::Edge;

/// The window envelope: the document's `window` header, else the ports of a
/// `window`-family node.
pub fn mount_config(tree: &ResolvedScene) -> Option<Value> {
    tree.window.clone().or_else(|| {
        tree.nodes
            .values()
            .find(|node| node.family == "window")
            .map(|node| json!(node.ports))
    })
}

/// The mount address: the envelope's `panel` (a declared sub-panel name,
/// which may carry characters the scene-name grammar forbids), else
/// `scene-<name>`.
pub fn page_id(tree: &ResolvedScene) -> String {
    mount_config(tree)
        .as_ref()
        .and_then(|window| window["panel"].as_str())
        .filter(|name| !name.trim().is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| format!("scene-{}", tree.name))
}

/// The authored edge; right when absent or unknown.
pub fn scene_edge(tree: &ResolvedScene) -> Edge {
    match mount_config(tree)
        .as_ref()
        .and_then(|w| w["edge"].as_str())
        .unwrap_or("right")
    {
        "left" => Edge::Left,
        "top" => Edge::Top,
        "bottom" => Edge::Bottom,
        _ => Edge::Right,
    }
}

/// A `kind:"dialog"` window: the centred dialog seat, never an edge page.
pub fn is_dialog(tree: &ResolvedScene) -> bool {
    mount_config(tree).is_some_and(|window| window["kind"] == "dialog")
}

/// A pointer-only overlay, independent of the dialog and edge seats.
pub fn is_toast(tree: &ResolvedScene) -> bool {
    mount_config(tree).is_some_and(|window| window["kind"] == "toast")
}

pub fn is_overlay(tree: &ResolvedScene) -> bool {
    is_dialog(tree) || is_toast(tree)
}

/// Toast size, title and optional frame. Toasts have no chrome by default.
pub fn toast_geometry(tree: &ResolvedScene) -> (f32, f32, Option<String>, bool) {
    let config = mount_config(tree).unwrap_or(Value::Null);
    (
        config["w"].as_f64().unwrap_or(360.0).clamp(240.0, 2048.0) as f32,
        config["h"].as_f64().unwrap_or(120.0).clamp(60.0, 2048.0) as f32,
        config["title"].as_str().map(str::to_owned),
        config["chrome"].as_bool() == Some(true),
    )
}

/// Authored dialog size (240..=2048, default 480), title, and chrome flag
/// (default true).
pub fn dialog_geometry(tree: &ResolvedScene) -> (f32, f32, Option<String>, bool) {
    let config = mount_config(tree).unwrap_or(Value::Null);
    let size = |key: &str| config[key].as_f64().unwrap_or(480.0).clamp(240.0, 2048.0) as f32;
    (
        size("w"),
        size("h"),
        config["title"].as_str().map(str::to_owned),
        config["chrome"].as_bool() != Some(false),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toast_header_and_window_node_mount_without_default_chrome() {
        for source in [
            "---\nscene: 1\nname: toast-probe\ncitizen: c\nwindow: {\"kind\":\"toast\",\"w\":10,\"h\":5000}\n---\n```mix\nroot: {widget: \"column\", children: []}\n```\n",
            "---\nscene: 1\nname: toast-probe\ncitizen: c\n---\n```mix\nroot: {widget: \"window\", kind: \"toast\", w: 10, h: 5000}\n```\n",
        ] {
            let tree = scene::resolve(&scene::parse(source).unwrap()).unwrap();
            assert!(is_toast(&tree) && is_overlay(&tree) && !is_dialog(&tree));
            assert_eq!(toast_geometry(&tree), (240.0, 2048.0, None, false));
        }
        let source = "---\nscene: 1\nname: toast-probe\ncitizen: c\nwindow: {\"kind\":\"toast\",\"w\":5000,\"h\":10,\"chrome\":true,\"title\":\"Probe\"}\n---\n```mix\nroot: {widget: \"column\", children: []}\n```\n";
        let tree = scene::resolve(&scene::parse(source).unwrap()).unwrap();
        assert_eq!(
            toast_geometry(&tree),
            (2048.0, 60.0, Some("Probe".into()), true)
        );
    }
}
