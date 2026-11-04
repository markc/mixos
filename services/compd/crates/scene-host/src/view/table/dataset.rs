// SPDX-License-Identifier: MIT OR Apache-2.0
//! Lab: immutable, bounded source snapshots owned by the native table.
use super::model::Row;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs::File,
    io::Read,
    path::{Component, Path},
    sync::Arc,
};

const BYTES: u64 = 16 * 1024 * 1024;
const ROWS: usize = 100_000;
type Packed = (String, String, u64, u64, f64, bool, bool, u8);

#[derive(Debug, Deserialize)]
pub(super) struct Dataset {
    pub version: u32,
    pub token: String,
    pub session: String,
    pub generation: u64,
    pub base: String,
    pub rows: Vec<Packed>,
    pub expanded: BTreeMap<String, bool>,
}

impl Dataset {
    pub fn load(
        source: &Value,
        session: &str,
        generation: u64,
        total: usize,
    ) -> Result<Arc<Self>, String> {
        let path = source["path"].as_str().ok_or("source path missing")?;
        let file = File::open(path).map_err(|e| e.to_string())?;
        let meta = file.metadata().map_err(|e| e.to_string())?;
        if !meta.is_file() || meta.len() > BYTES {
            return Err("source exceeds file budget".into());
        }
        // IoRead over an unbuffered File can issue a syscall per JSON byte.
        // Read once under the byte cap, then parse from the bounded slice.
        let mut bytes=Vec::with_capacity(meta.len() as usize);
        file.take(BYTES+1).read_to_end(&mut bytes).map_err(|e|e.to_string())?;
        if bytes.len() as u64>BYTES {return Err("source grew beyond file budget".into());}
        let data: Self=serde_json::from_slice(&bytes).map_err(|e|e.to_string())?;
        if data.version != 1
            || data.session != session
            || data.generation != generation
            || source["token"].as_str() != Some(data.token.as_str())
            || data.token.is_empty()
            || data.rows.len() != total
            || total > ROWS
            || !Path::new(&data.base).is_absolute()
        {
            return Err("source identity/count mismatch".into());
        }
        let mut ids = std::collections::BTreeSet::new();
        for (id, name, _, _, modified, _, _, depth) in &data.rows {
            let path = Path::new(id);
            if id.is_empty()
                || id.len() > 4096
                || name.len() > 4096
                || !modified.is_finite()
                || *depth > 16
                || id.split('/').any(|p| p.is_empty() || p == "." || p == "..")
                || path
                    .components()
                    .any(|c| !matches!(c, Component::Normal(_)))
                || path.file_name().and_then(|n| n.to_str()) != Some(name.as_str())
                || !ids.insert(id)
            {
                return Err("invalid source row".into());
            }
        }
        Ok(Arc::new(data))
    }

    pub fn id(&self, index: usize) -> Option<String> {
        self.rows.get(index).map(|r| {
            Path::new(&self.base)
                .join(&r.0)
                .to_string_lossy()
                .into_owned()
        })
    }

    pub fn row(&self, index: usize) -> Option<Arc<Row>> {
        let (relative, name, size, _, modified, dir, _, depth) = self.rows.get(index)?;
        let id = Path::new(&self.base)
            .join(relative)
            .to_string_lossy()
            .into_owned();
        let marker = if *dir {
            if self.expanded.get(&id).copied().unwrap_or(false) {
                "▾ "
            } else {
                "▸ "
            }
        } else {
            "  "
        };
        let cells = vec![
            format!("{}{marker}{name}", "  ".repeat(*depth as usize)),
            if *dir {
                "<DIR>".into()
            } else {
                size.to_string()
            },
            modified.to_string(),
        ];
        let raw = json!({"id":id,"cells":cells,"edit":name,"dir":dir,"depth":depth});
        Some(Arc::new(Row {
            id,
            cells,
            edit: name.clone(),
            raw,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn immutable_source_checks_identity_rows_and_survives_unlink() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("model.json");
        let data = json!({"version":1,"token":"t","session":"s","generation":2,"base":"/fixture","expanded":{},"rows":[["a","a",3,0,1.5,false,false,0]]});
        std::fs::write(&path, data.to_string()).unwrap();
        let source = json!({"path":path,"token":"t"});
        assert!(Dataset::load(&source, "other", 2, 1).is_err());
        assert!(Dataset::load(&source, "s", 1, 1).is_err());
        assert!(Dataset::load(&source, "s", 2, 2).is_err());
        let model = Dataset::load(&source, "s", 2, 1).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(model.id(0).as_deref(), Some("/fixture/a"));
        assert_eq!(model.row(0).unwrap().cells[1], "3");
        assert!(Dataset::load(&source, "s", 2, 1).is_err());
        let mut bad = data;
        bad["rows"][0][0] = json!("../a");
        std::fs::write(&path, bad.to_string()).unwrap();
        assert!(Dataset::load(&source, "s", 2, 1).is_err());
    }
}
