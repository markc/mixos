// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Installed families, read once away from the paint thread.
use std::collections::BTreeSet;
use std::sync::{Once, OnceLock};

#[derive(Default)]
pub struct Families {
    pub all: Vec<String>,
    pub mono: Vec<String>,
}

impl Families {
    fn from_database(database: &fontdb::Database) -> Self {
        let mut all = BTreeSet::new();
        let mut mono = BTreeSet::new();
        for face in database.faces() {
            for (family, _) in &face.families {
                if family.is_empty()
                    || family.len() > 128
                    || family.trim() != family.as_str()
                    || family.chars().any(char::is_control)
                {
                    continue;
                }
                all.insert(family.clone());
                if face.monospaced {
                    mono.insert(family.clone());
                }
            }
        }
        Self {
            all: all.into_iter().collect(),
            mono: mono.into_iter().collect(),
        }
    }
}

pub fn installed(ctx: &egui::Context) -> Option<&'static Families> {
    static STARTED: Once = Once::new();
    static FAMILIES: OnceLock<Families> = OnceLock::new();
    STARTED.call_once(|| {
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let mut database = fontdb::Database::new();
            database.load_system_fonts();
            let _ = FAMILIES.set(Families::from_database(&database));
            ctx.request_repaint();
        });
    });
    FAMILIES.get()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn families_are_sorted_deduplicated_and_filtered_by_the_monospace_flag() {
        let mut database = fontdb::Database::new();
        database.load_font_data(
            include_bytes!("../../../libs/toolkit/assets/fonts/Inter-Regular.ttf").to_vec(),
        );
        database.load_font_data(
            include_bytes!("../../../libs/toolkit/assets/fonts/Inter-Light.ttf").to_vec(),
        );
        database.load_font_data(toolkit::fonts::mono_face().1.to_vec());
        let families = Families::from_database(&database);
        assert_eq!(families.all, ["Inter", "JetBrains Mono"]);
        assert_eq!(families.mono, ["JetBrains Mono"]);
    }
}
