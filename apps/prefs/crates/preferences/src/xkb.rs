// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The keyboard layouts a person can add (Input › Keyboard › Add input
//! source), read from the xkeyboard-config rules the system ships: the
//! `evdev.lst` list for the common layouts and their variants, and
//! `evdev.extras.xml` for the extended ones. Each becomes a live option
//! (`[layout, variant]`, its description) of [`LAYOUTS`] or [`EXTENDED`],
//! in the order the drawer lists them: English (US) first, the custom
//! layout last, the rest by description, each layout followed by its
//! variants.
use crate::engine::Engine;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The live option source of the common layouts.
pub const LAYOUTS: &str = "xkb.layouts";
/// The live option source of the extended layouts.
pub const EXTENDED: &str = "xkb.layouts.extended";

/// One layout and its variants, as the rules describe them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Layout {
    pub name: String,
    pub description: String,
    /// (variant, description).
    pub variants: Vec<(String, String)>,
}

/// The layouts of an `evdev.lst`: its `! layout` and `! variant`
/// sections (a variant's description starts `<layout>: `).
pub fn parse_lst(text: &str) -> Vec<Layout> {
    let mut layouts: BTreeMap<String, Layout> = BTreeMap::new();
    let mut order = Vec::new();
    let mut section = "";
    for line in text.lines() {
        if let Some(name) = line.strip_prefix('!') {
            section = match name.trim() {
                "layout" => "layout",
                "variant" => "variant",
                _ => "",
            };
            continue;
        }
        let line = line.trim();
        let Some((name, description)) = line.split_once(char::is_whitespace) else {
            continue;
        };
        let description = description.trim();
        match section {
            "layout" => {
                order.push(name.to_owned());
                layouts.insert(
                    name.to_owned(),
                    Layout {
                        name: name.to_owned(),
                        description: description.to_owned(),
                        variants: Vec::new(),
                    },
                );
            }
            "variant" => {
                if let Some((layout, description)) = description.split_once(": ")
                    && let Some(entry) = layouts.get_mut(layout)
                {
                    entry
                        .variants
                        .push((name.to_owned(), description.to_owned()));
                }
            }
            _ => {}
        }
    }
    order
        .into_iter()
        .filter_map(|name| layouts.remove(&name))
        .collect()
}

/// The text of every `<tag>…</tag>` directly inside `block`, in order, and
/// the rest of `block` after each.
fn elements<'a>(block: &'a str, tag: &str) -> Vec<&'a str> {
    let (open, close) = (format!("<{tag}>"), format!("</{tag}>"));
    let mut out = Vec::new();
    let mut rest = block;
    while let Some(start) = rest.find(&open) {
        let body = &rest[start + open.len()..];
        let Some(end) = body.find(&close) else { break };
        out.push(&body[..end]);
        rest = &body[end + close.len()..];
    }
    out
}

/// A `configItem`'s name and description (the first of each, so a
/// variant's own come before anything nested).
fn item(block: &str) -> Option<(String, String)> {
    let start = block.find("<configItem")?;
    let end = block[start..].find("</configItem>")? + start;
    let config = &block[start..end];
    let name = elements(config, "name").first()?.trim().to_owned();
    let description = elements(config, "description")
        .first()
        .map_or_else(|| name.clone(), |d| unescape(d.trim()));
    Some((name, description))
}

fn unescape(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// The layouts of an `evdev.extras.xml` (`layoutList` › `layout` ›
/// `configItem`, `variantList` › `variant` › `configItem`).
pub fn parse_xml(text: &str) -> Vec<Layout> {
    let list = elements(text, "layoutList");
    let Some(list) = list.first() else {
        return Vec::new();
    };
    elements(list, "layout")
        .into_iter()
        .filter_map(|layout| {
            let (name, description) = item(layout)?;
            let variants = elements(layout, "variantList")
                .first()
                .map(|list| {
                    elements(list, "variant")
                        .into_iter()
                        .filter_map(item)
                        .collect()
                })
                .unwrap_or_default();
            Some(Layout {
                name,
                description,
                variants,
            })
        })
        .collect()
}

/// The drawer's order and shape: (`[layout, variant]`, description), US
/// first, custom last, the rest by description, variants under their
/// layout by description.
pub fn options(mut layouts: Vec<Layout>) -> Vec<(Value, String)> {
    let rank = |l: &Layout| match l.name.as_str() {
        "us" => 0,
        "custom" => 2,
        _ => 1,
    };
    layouts.sort_by(|a, b| {
        rank(a)
            .cmp(&rank(b))
            .then_with(|| a.description.cmp(&b.description))
    });
    let mut out = Vec::new();
    for mut layout in layouts {
        out.push((json!([layout.name, ""]), layout.description));
        layout.variants.sort_by(|a, b| a.1.cmp(&b.1));
        for (variant, description) in layout.variants {
            out.push((json!([layout.name, variant]), description));
        }
    }
    out
}

/// The rules directory: `$XKB_CONFIG_ROOT/rules`, else the system one.
pub fn rules_dir() -> PathBuf {
    std::env::var_os("XKB_CONFIG_ROOT")
        .map_or_else(|| PathBuf::from("/usr/share/X11/xkb"), PathBuf::from)
        .join("rules")
}

impl Engine {
    /// Fill [`LAYOUTS`] and [`EXTENDED`] from the rules in `dir`; a
    /// missing file leaves its source empty.
    pub fn read_layouts(&mut self, dir: &Path) {
        let read = |name: &str| std::fs::read_to_string(dir.join(name)).unwrap_or_default();
        let common = parse_lst(&read("evdev.lst"));
        let extended = parse_xml(&read("evdev.extras.xml"));
        self.set_options(LAYOUTS, options(common));
        self.set_options(EXTENDED, options(extended));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LST: &str = "! model\n  pc105  Generic 105-key PC\n\n! layout\n  us              English (US)\n  custom          A user-defined custom Layout\n  de              German\n  al              Albanian\n\n! variant\n  nodeadkeys      de: German (no dead keys)\n  dvorak          us: English (Dvorak)\n  colemak         us: English (Colemak)\n\n! option\n  grp                  Switching to another layout\n";

    const XML: &str = r#"<?xml version="1.0"?>
<xkbConfigRegistry><modelList/><layoutList>
<layout><configItem popularity="exotic"><name>ancient</name><shortDescription>xx</shortDescription><description>Ancient</description>
<languageList><iso639Id>got</iso639Id></languageList></configItem>
<variantList><variant><configItem><name>got</name><description>Gothic &amp; more</description></configItem></variant></variantList></layout>
<layout><configItem><name>apl</name><description>APL</description></configItem></layout>
</layoutList><optionList/></xkbConfigRegistry>"#;

    #[test]
    fn the_list_reads_layouts_and_their_variants() {
        let layouts = parse_lst(LST);
        assert_eq!(layouts.len(), 4);
        assert_eq!(layouts[0].description, "English (US)");
        assert_eq!(
            layouts[0].variants,
            [
                ("dvorak".to_owned(), "English (Dvorak)".to_owned()),
                ("colemak".to_owned(), "English (Colemak)".to_owned())
            ]
        );
        assert_eq!(layouts[2].variants[0].0, "nodeadkeys");
    }

    #[test]
    fn options_put_us_first_custom_last_and_variants_under_their_layout() {
        let shown: Vec<String> = options(parse_lst(LST))
            .into_iter()
            .map(|(v, d)| format!("{}/{}={d}", v[0].as_str().unwrap(), v[1].as_str().unwrap()))
            .collect();
        assert_eq!(
            shown,
            [
                "us/=English (US)",
                "us/colemak=English (Colemak)",
                "us/dvorak=English (Dvorak)",
                "al/=Albanian",
                "de/=German",
                "de/nodeadkeys=German (no dead keys)",
                "custom/=A user-defined custom Layout",
            ]
        );
    }

    #[test]
    fn the_extras_read_layouts_variants_and_entities() {
        let layouts = parse_xml(XML);
        assert_eq!(layouts.len(), 2);
        assert_eq!(layouts[0].name, "ancient");
        assert_eq!(layouts[0].description, "Ancient");
        assert_eq!(
            layouts[0].variants,
            [("got".to_owned(), "Gothic & more".to_owned())]
        );
        assert!(layouts[1].variants.is_empty());
    }

    #[test]
    fn the_system_rules_read_when_present() {
        let dir = rules_dir();
        if !dir.join("evdev.lst").exists() {
            eprintln!(
                "SKIP: system evdev.lst absent; checked-in LST/XML parser fixtures still run"
            );
            return;
        }
        let mut e = Engine::new(|key, _| key.to_owned());
        e.read_layouts(&dir);
        let common = e.live_options(LAYOUTS);
        assert!(!common.is_empty(), "system evdev.lst produced no layouts");
        assert_eq!(common[0].0, json!(["us", ""]));
        assert!(common.len() > 100, "{} layouts", common.len());
    }
}
