// SPDX-License-Identifier: MIT OR Apache-2.0
//! Shared, strict-data window state. Apps own only their versioned `app` value;
//! toolkit owns the frame and named widgets. Pass `None` to [`Store::load`] in
//! tests and headless runs. No environment is consulted by the store itself.
//!
//! Call [`Store::tick`] after drawing and [`Store::flush`] before exit. Failed
//! envelopes are poison pills: defaults are usable but the file is never written.
//! Individual sections decode independently. See the toolkit adoption guide.

pub mod frame;
pub mod widgets;

pub use frame::Frame;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};
pub use widgets::Widgets;

/// One window kind's persistence target and live state.
pub struct Store<A> {
    pub window: Frame,
    pub widgets: Widgets,
    pub app: A,
    path: Option<PathBuf>,
    writable: bool,
    version: u32,
    written: String,
    last_write: Option<Instant>,
    writes: usize,
    app_loaded: bool,
}

impl<A: Serialize + DeserializeOwned + Default> Store<A> {
    /// `directory` is the complete window-kind directory, not a parent root.
    /// Production callers use `AppDirs::resolve(app).map(|d| d.state())`.
    pub fn load(directory: Option<PathBuf>, version: u32) -> Self {
        let path = directory.map(|p| p.join("window.conf.mix"));
        let mut store = Self {
            window: Frame::default(),
            widgets: Widgets::default(),
            app: A::default(),
            path,
            writable: true,
            version,
            written: String::new(),
            last_write: None,
            writes: 0,
            app_loaded: false,
        };
        if let Some(path) = &store.path {
            match std::fs::read_to_string(path) {
                Ok(raw) => match strict::from_str::<Value>(&raw) {
                    Ok(value)
                        if value.is_object() && value["schema_version"].as_f64() == Some(1.0) =>
                    {
                        store.window = decode(&value["window"]);
                        store.widgets = decode(&value["widgets"]);
                        if value["app"]["version"].as_f64() == Some(f64::from(version))
                            && let Some(app) = try_decode(&value["app"]["value"])
                        {
                            store.app = app;
                            store.app_loaded = true;
                        }
                    }
                    _ => store.writable = false,
                },
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => store.writable = false,
            }
            if !store.writable {
                eprintln!(
                    "toolkit: saved window state is unreadable or unsupported; preserving original file"
                );
            }
        }
        store.written = store.serialise().unwrap_or_default();
        store
    }

    /// Data suitable for an app's `info`/describe response, without its path.
    pub fn describe(&self) -> Value {
        json!({"schema_version":1,"window":self.window,"widgets":self.widgets,
            "app":{"version":self.version,"value":self.app},"writable":self.writable})
    }

    fn serialise(&self) -> Result<String, String> {
        strict::to_string_pretty(&json!({"schema_version":1,"window":self.window,
            "widgets":self.widgets,"app":{"version":self.version,"value":self.app}}))
        .map_err(|e| e.to_string())
    }

    /// Coalesce changes; never write while any pointer button is held.
    pub fn tick(&mut self, now: Instant, pointer_down: bool) -> Result<bool, String> {
        if pointer_down
            || self
                .last_write
                .is_some_and(|last| now.saturating_duration_since(last) < Duration::from_secs(1))
        {
            return Ok(false);
        }
        self.write(now)
    }

    /// Final save, bypassing the coalescing interval.
    pub fn flush(&mut self) -> Result<bool, String> {
        self.write(Instant::now())
    }

    fn write(&mut self, now: Instant) -> Result<bool, String> {
        if !self.writable {
            return Ok(false);
        }
        let content = self.serialise()?;
        if content == self.written {
            return Ok(false);
        }
        if let Some(path) = &self.path {
            let parent = path.parent().ok_or("missing saved-state directory")?;
            config::atomic::create_directory(parent).map_err(|e| e.to_string())?;
            config::atomic::replace(path, content.as_bytes()).map_err(|e| e.to_string())?;
            self.writes += 1;
        }
        self.written = content;
        self.last_write = Some(now);
        Ok(true)
    }

    /// Successful disk replacements, useful for diagnostics and write budgets.
    pub fn writes(&self) -> usize {
        self.writes
    }
    /// Whether a compatible app section was decoded, so first-launch defaults
    /// can differ from deliberately saved empty selections or collapsed trees.
    pub fn app_loaded(&self) -> bool {
        self.app_loaded
    }
    /// Schedule a save timer only while a changed value is waiting.
    pub fn pending(&self) -> bool {
        self.writable && self.serialise().is_ok_and(|s| s != self.written)
    }
}

fn decode<T: DeserializeOwned + Default>(value: &Value) -> T {
    try_decode(value).unwrap_or_default()
}

fn try_decode<T: DeserializeOwned>(value: &Value) -> Option<T> {
    // Strict data represents numbers as f64; decode typed integers through its
    // checked integer deserialiser rather than serde_json's f64 rejection.
    strict::to_string_pretty(value)
        .ok()
        .and_then(|raw| strict::from_str(&raw).ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Default, Serialize, serde::Deserialize, PartialEq, Debug)]
    struct App {
        selected: String,
    }
    #[test]
    fn round_trip_and_unchanged_values_do_not_write() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = Store::<App>::load(Some(dir.path().into()), 2);
        s.app.selected = "row".into();
        s.window.size = Some([900.0, 600.0]);
        s.window.maximised = true;
        assert!(s.flush().unwrap());
        assert!(!s.flush().unwrap());
        assert_eq!(s.writes(), 1);
        let back = Store::<App>::load(Some(dir.path().into()), 2);
        assert!(back.app_loaded());
        assert_eq!(back.app, s.app);
        assert_eq!(back.window, s.window);
    }
    #[test]
    fn malformed_and_future_schema_are_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("window.conf.mix");
        for raw in ["schema_version: $run", "schema_version: 99\n"] {
            std::fs::write(&path, raw).unwrap();
            let mut s = Store::<App>::load(Some(dir.path().into()), 2);
            assert_eq!(s.app, App::default());
            s.app.selected = "new".into();
            assert!(!s.flush().unwrap());
            assert_eq!(std::fs::read_to_string(&path).unwrap(), raw);
        }
    }
    #[test]
    fn bad_section_drops_only_itself() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("window.conf.mix"), "schema_version: 1\nwindow: false\nwidgets: {}\napp: {version: 2, value: {selected: \"kept\"}}\n").unwrap();
        let s = Store::<App>::load(Some(dir.path().into()), 2);
        assert_eq!(s.window, Frame::default());
        assert_eq!(s.app.selected, "kept");
    }
    #[test]
    fn pointer_and_interval_coalesce_writes_and_memory_is_isolated() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = Store::<App>::load(Some(dir.path().into()), 2);
        let now = Instant::now();
        s.app.selected = "one".into();
        assert!(!s.tick(now, true).unwrap());
        assert_eq!(s.writes(), 0);
        assert!(s.tick(now, false).unwrap());
        s.app.selected = "two".into();
        assert!(!s.tick(now + Duration::from_millis(999), false).unwrap());
        assert!(s.tick(now + Duration::from_secs(1), false).unwrap());
        assert_eq!(s.writes(), 2);
        let mut memory = Store::<App>::load(None, 2);
        memory.app.selected = "memory".into();
        memory.flush().unwrap();
        assert_eq!(memory.writes(), 0);
        assert!(memory.path.is_none());
    }
}
