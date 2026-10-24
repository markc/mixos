// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! `shell.scene.layout`: per-node geometry, as drawn.
//!
//! The reply is Quoin's frozen shape (shell-commands.json `shell.scene.layout`):
//! `{scene, revision, applied_revision, visible, surface:{kind, edge, output,
//! x, y, w, h}, nodes:{id:{x,y,w,h,hidden}}, instances:{list:{item:{x,y,w,h}}},
//! chrome:{close:{x,y,w,h}}}`, logical px relative to the scene's surface.
//! An unmapped scene answers `visible:false` with empty `nodes`,
//! `instances` and `chrome`. compd adds `page`: the opaque `#rrggbbaa` it
//! paints under the scene (the design `base` surface), null while unmapped,
//! so a pixel check reads the colour from the host instead of assuming one.
//!
//! The rects are the ones the scene was laid out, drawn and hit-tested at
//! (`crate::view` lays scenes out with taffy, as Quoin does), with each
//! list's scroll applied to its rows. A document node that is not drawn
//! (hidden, inside a hidden parent, or a template) reports `hidden:true` at
//! zero size.

use std::collections::HashMap;

use egui::Rect;
use serde_json::{Map, Value, json};

use crate::templates::rows;

/// The rects one scene was drawn at, by what the reply names them.
#[derive(Debug, Default)]
pub struct Found {
    pub nodes: HashMap<String, Rect>,
    pub rows: HashMap<(String, String), Rect>,
    pub close: Option<Rect>,
}

impl Found {
    /// What `ui` drew in its latest pass, rows moved by their list's scroll.
    pub(crate) fn of(ui: &crate::view::SceneUi) -> Self {
        let layout = ui.layout();
        let mut found = Found {
            nodes: layout.nodes.clone().into_iter().collect(),
            rows: HashMap::new(),
            close: layout.close,
        };
        for (list, items) in &layout.instances {
            let offset = if layout.lists.contains_key(list) {
                ui.scrolled(list)
            } else {
                0.0
            };
            for (item, rect) in items {
                found.rows.insert(
                    (list.clone(), item.clone()),
                    rect.translate(egui::vec2(0.0, -offset)),
                );
            }
        }
        found
    }
}

fn rect_json(rect: &Rect, factor: f32, hidden: Option<bool>) -> Value {
    let round = |v: f32| ((v * factor) as f64 * 100.0).round() / 100.0;
    let mut out = json!({
        "x": round(rect.min.x),
        "y": round(rect.min.y),
        "w": round(rect.width()),
        "h": round(rect.height()),
    });
    if let Some(hidden) = hidden {
        out["hidden"] = json!(hidden);
    }
    out
}

/// `nodes`, `instances` and `chrome` from what was drawn of `tree`.
/// `factor` converts the instance's logical px to the document's (its UI
/// scale over the output scale; 1 when they agree). `node` narrows to one
/// document node and, for a list, its rows.
pub fn measured(
    tree: &scene::ResolvedScene,
    found: &Found,
    factor: f32,
    node: Option<&str>,
) -> (Value, Value, Value) {
    let mut nodes = Map::new();
    let mut instances = Map::new();
    let templates = crate::templates::template_ids(tree);
    for (id, n) in &tree.nodes {
        if node.is_some_and(|wanted| wanted != id.as_str()) || templates.contains(id) {
            continue;
        }
        let entry = match found.nodes.get(id) {
            Some(rect) => rect_json(rect, factor, Some(false)),
            None => json!({"x": 0.0, "y": 0.0, "w": 0.0, "h": 0.0, "hidden": true}),
        };
        nodes.insert(id.clone(), entry);
        if n.family == "list" {
            let mut items = Map::new();
            for item in rows(n) {
                let item_id = item["id"].as_str().unwrap_or_default();
                if let Some(rect) = found.rows.get(&(id.clone(), item_id.to_owned())) {
                    items.insert(item_id.to_owned(), rect_json(rect, factor, None));
                }
            }
            if !items.is_empty() {
                instances.insert(id.clone(), Value::Object(items));
            }
        }
    }
    let mut chrome = Map::new();
    if let Some(rect) = &found.close {
        chrome.insert("close".into(), rect_json(rect, factor, None));
    }
    (
        Value::Object(nodes),
        Value::Object(instances),
        Value::Object(chrome),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect::from_min_size(egui::pos2(x, y), egui::vec2(w, h))
    }

    #[test]
    fn rects_report_drawn_nodes_hidden_ones_and_rows() {
        let tree = scene::resolve(&scene::parse(scene::fixtures::CLIPPANEL).unwrap()).unwrap();
        let mut found = Found::default();
        found
            .nodes
            .insert("root".into(), rect(0.0, 32.0, 720.0, 488.0));
        // A scrolled row is reported where it is drawn.
        found
            .rows
            .insert(("table".into(), "e1".into()), rect(14.0, 90.0, 692.0, 38.0));
        found.close = Some(rect(681.0, 5.0, 28.0, 24.0));
        let (nodes, instances, chrome) = measured(&tree, &found, 1.0, None);
        assert_eq!(
            nodes["root"],
            json!({"x":0.0,"y":32.0,"w":720.0,"h":488.0,"hidden":false})
        );
        assert_eq!(
            nodes["remote_title"]["hidden"], true,
            "a node not drawn is reported hidden"
        );
        assert!(
            nodes.get("entry_row").is_none(),
            "templates are rows, not nodes"
        );
        assert_eq!(
            instances["table"]["e1"],
            json!({"x":14.0,"y":90.0,"w":692.0,"h":38.0})
        );
        assert_eq!(
            chrome["close"],
            json!({"x":681.0,"y":5.0,"w":28.0,"h":24.0})
        );
        // Narrowed to one node.
        let (nodes, instances, _) = measured(&tree, &found, 1.0, Some("root"));
        assert_eq!(nodes.as_object().unwrap().len(), 1);
        assert!(instances.as_object().unwrap().is_empty());
        // The factor converts the instance's px to the document's.
        let (nodes, _, _) = measured(&tree, &found, 0.5, Some("root"));
        assert_eq!(
            nodes["root"],
            json!({"x":0.0,"y":16.0,"w":360.0,"h":244.0,"hidden":false})
        );
    }
}
