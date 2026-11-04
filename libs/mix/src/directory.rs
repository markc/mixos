// SPDX-License-Identifier: MIT OR Apache-2.0
//! Lab: one bounded native directory/metadata pass, independent of any GUI.
use crate::{
    error::{MixError, MixResult},
    value::Value,
};
use std::{fs::Metadata, os::unix::fs::MetadataExt};

const FIELDS: &[&str] = &[
    "name",
    "uid",
    "gid",
    "nlink",
    "size",
    "mode",
    "perm",
    "ino",
    "dev",
    "ctime",
    "mtime",
    "atime",
    "ctime_nsec",
    "mtime_nsec",
    "atime_nsec",
    "is_file",
    "is_dir",
    "is_symlink",
];

fn invalid(message: impl Into<String>) -> MixError {
    MixError::structured("OPTION_INVALID", message)
}
fn field(key: &str, name: &str, m: &Metadata) -> Value {
    match key {
        "name" => Value::String(name.into()),
        "ino" => Value::String(m.ino().to_string()),
        "dev" => Value::String(m.dev().to_string()),
        "is_file" => Value::Bool(m.is_file()),
        "is_dir" => Value::Bool(m.is_dir()),
        "is_symlink" => Value::Bool(m.file_type().is_symlink()),
        other => Value::Number(match other {
            "uid" => m.uid() as f64,
            "gid" => m.gid() as f64,
            "nlink" => m.nlink() as f64,
            "size" => m.size() as f64,
            "mode" => m.mode() as f64,
            "perm" => (m.mode() & 0o7777) as f64,
            "ctime" => m.ctime() as f64,
            "mtime" => m.mtime() as f64,
            "atime" => m.atime() as f64,
            "ctime_nsec" => m.ctime_nsec() as f64,
            "mtime_nsec" => m.mtime_nsec() as f64,
            "atime_nsec" => m.atime_nsec() as f64,
            _ => unreachable!(),
        }),
    }
}

pub(crate) fn entries(args: Vec<Value>) -> MixResult<Option<Value>> {
    if args.is_empty() || args.len() > 2 {
        return Err(invalid("dir_entries expects path and optional options"));
    }
    let Value::String(path) = &args[0] else {
        return Err(invalid("dir_entries path must be a string"));
    };
    let mut hidden = true;
    let mut limit = 100000usize;
    let mut columns = None;
    let mut count_only = false;
    if let Some(opts) = args.get(1) {
        match opts {
            Value::Nil => {}
            Value::Map(map) => {
                for (key, value) in map.iter() {
                    match (key.as_str(), value) {
                        ("hidden", Value::Bool(b)) => hidden = *b,
                        ("count", Value::Bool(b)) => count_only = *b,
                        ("max_entries", Value::Number(n))
                            if n.is_finite()
                                && *n >= 0.0
                                && *n <= 1_000_000.0
                                && n.fract() == 0.0 =>
                        {
                            limit = *n as usize
                        }
                        ("columns", Value::List(values))
                            if !values.is_empty() && values.len() <= FIELDS.len() =>
                        {
                            let mut names = Vec::new();
                            for value in values.iter() {
                                let Value::String(name) = value else {
                                    return Err(invalid("columns must contain field names"));
                                };
                                if !FIELDS.contains(&name.as_str()) || names.contains(name) {
                                    return Err(invalid("unknown or repeated directory column"));
                                }
                                names.push(name.clone());
                            }
                            columns = Some(names);
                        }
                        _ => return Err(invalid(format!("invalid dir_entries option {key}"))),
                    }
                }
            }
            _ => return Err(invalid("dir_entries options must be a map or nil")),
        }
    }
    if count_only && columns.is_some() {
        return Err(invalid("count and columns cannot be combined"));
    }
    let io_error = |e: std::io::Error| {
        MixError::structured("DIR_ENTRIES_IO", format!("dir_entries {path}: {e}"))
    };
    let mut rows = Vec::new();
    let mut count = 0usize;
    for (scanned, entry) in std::fs::read_dir(path).map_err(io_error)?.enumerate() {
        if crate::interrupt::is_interrupted() {
            return Err(MixError::structured(
                "DIR_ENTRIES_INTERRUPTED",
                "directory scan interrupted",
            ));
        }
        let entry = entry.map_err(io_error)?;
        // Count inspected names, including hidden ones, before stat/allocation.
        if scanned >= limit {
            return Err(MixError::structured(
                "DIR_ENTRIES_LIMIT",
                "directory inspection limit exceeded",
            ));
        }
        let os_name = entry.file_name();
        let name = os_name.to_str().ok_or_else(|| {
            MixError::structured("DIR_ENTRIES_NAME", "directory name is not UTF-8")
        })?;
        if !hidden && name.starts_with('.') {
            continue;
        }
        if count_only {
            count += 1;
            continue;
        }
        let metadata = std::fs::symlink_metadata(entry.path()).map_err(io_error)?;
        let value = if let Some(columns) = &columns {
            Value::list(
                columns
                    .iter()
                    .map(|key| field(key, name, &metadata))
                    .collect(),
            )
        } else {
            Value::map(
                FIELDS
                    .iter()
                    .map(|key| (key.to_string(), field(key, name, &metadata)))
                    .collect(),
            )
        };
        rows.push((name.to_owned(), value));
    }
    if count_only {
        return Ok(Some(Value::Number(count as f64)));
    }
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(Some(Value::list(
        rows.into_iter().map(|(_, v)| v).collect(),
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn opts(pairs: Vec<(&str, Value)>) -> Value {
        Value::map(pairs.into_iter().map(|(k, v)| (k.into(), v)).collect())
    }
    #[test]
    fn directory_entries_match_lstat_and_project_columns_without_following_links() {
        let _guard = crate::interrupt::TEST_LOCK.lock().unwrap();
        crate::interrupt::_test_clear();
        let root = std::env::temp_dir().join(format!(
            "mix-directory-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("file"), b"abc").unwrap();
        std::fs::write(root.join(".hidden"), b"h").unwrap();
        std::os::unix::fs::symlink("missing", root.join("link")).unwrap();
        let path = Value::String(root.to_str().unwrap().into());
        assert_eq!(
            entries(vec![
                path.clone(),
                opts(vec![
                    ("hidden", Value::Bool(false)),
                    ("count", Value::Bool(true))
                ])
            ])
            .unwrap(),
            Some(Value::Number(2.0))
        );
        let result = entries(vec![
            path.clone(),
            opts(vec![("hidden", Value::Bool(false))]),
        ])
        .unwrap()
        .unwrap();
        let Value::List(rows) = &result else { panic!() };
        assert_eq!(rows.len(), 2);
        let Value::Map(file) = &rows[0] else { panic!() };
        assert_eq!(file["size"], Value::Number(3.0));
        assert!(matches!(file["ino"], Value::String(_)));
        let Value::Map(link) = &rows[1] else { panic!() };
        assert_eq!(link["is_symlink"], Value::Bool(true));
        assert_eq!(link["is_dir"], Value::Bool(false));
        let columns = Value::list(vec![
            Value::String("name".into()),
            Value::String("size".into()),
        ]);
        let projected = entries(vec![
            path.clone(),
            opts(vec![("hidden", Value::Bool(false)), ("columns", columns)]),
        ])
        .unwrap()
        .unwrap();
        let Value::List(projected) = &projected else {
            panic!()
        };
        let Value::List(first) = &projected[0] else {
            panic!()
        };
        assert_eq!(
            first.as_ref(),
            &vec![Value::String("file".into()), Value::Number(3.0)]
        );
        let error = entries(vec![
            path.clone(),
            opts(vec![("max_entries", Value::Number(1.0))]),
        ])
        .unwrap_err();
        assert_eq!(error.info().unwrap().code, "DIR_ENTRIES_LIMIT");
        for value in [
            Value::Number(-1.0),
            Value::Number(1.5),
            Value::String("2".into()),
        ] {
            assert!(entries(vec![path.clone(), opts(vec![("max_entries", value)])]).is_err());
        }
        assert!(
            entries(vec![
                path.clone(),
                opts(vec![("hiden", Value::Bool(false))])
            ])
            .is_err()
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
