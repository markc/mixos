// SPDX-License-Identifier: MIT OR Apache-2.0
//! Shared, strict-data window state. Apps own only their versioned `app` value;
//! toolkit owns the frame and named widgets. Pass `None` to [`Store::load`] in
//! tests and headless runs. No environment is consulted by the store itself.
//!
//! Call [`Store::tick`] after drawing and [`Store::flush`] before exit. Failed
//! envelopes are poison pills: defaults are usable but the file is never written.
//! Individual sections decode independently. See the toolkit adoption guide.

pub mod frame;
pub mod secondary;
pub mod widgets;

pub use frame::Frame;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    time::{Duration, Instant},
};
pub use widgets::Widgets;

#[derive(Serialize)]
struct Envelope<'a, A> {
    schema_version: u32,
    window: Section<'a, &'a Frame>,
    widgets: Section<'a, &'a Widgets>,
    windows: Section<'a, &'a BTreeMap<String, Frame>>,
    app: Section<'a, AppSection<'a, A>>,
}

#[derive(Serialize)]
#[serde(untagged)]
enum Section<'a, T> {
    Raw(&'a Value),
    Current(T),
}
impl<'a, T> Section<'a, T> {
    fn new(raw: Option<&'a Value>, current: T) -> Self {
        raw.map_or(Self::Current(current), Self::Raw)
    }
}

#[derive(Serialize)]
struct AppSection<'a, A> {
    version: u32,
    value: &'a A,
}

/// One window kind's persistence target and live state.
pub struct Store<A> {
    pub window: Frame,
    pub widgets: Widgets,
    pub app: A,
    pub windows: BTreeMap<String, Frame>,
    preserved: BTreeMap<String, Value>,
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
            windows: BTreeMap::new(),
            preserved: BTreeMap::new(),
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
                        if let Some(window) = try_decode(&value["window"]) {
                            store.window = window;
                        } else if let Some(raw) = value.get("window") {
                            store.preserved.insert("window".into(), raw.clone());
                        }
                        if let Some(widgets) = try_decode(&value["widgets"]) {
                            store.widgets = widgets;
                        } else if let Some(raw) = value.get("widgets") {
                            store.preserved.insert("widgets".into(), raw.clone());
                        }
                        if let Some(windows) = try_decode(&value["windows"]) {
                            store.windows = windows;
                        } else if let Some(raw) = value.get("windows") {
                            store.preserved.insert("windows".into(), raw.clone());
                        }
                        if value["app"]["version"].as_f64() == Some(f64::from(version))
                            && let Some(app) = try_decode(&value["app"]["value"])
                        {
                            store.app = app;
                            store.app_loaded = true;
                        } else if let Some(raw) = value.get("app") {
                            // Older versions have no migration contract and may
                            // be replaced by current defaults. Future or broken
                            // sections belong to their writer: retain the value.
                            if !raw["version"].as_f64().is_some_and(|v| v >= 0.0 && v.fract() == 0.0 && v < f64::from(version)) {
                                store.preserved.insert("app".into(), raw.clone());
                            }
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
        match self.value() {
            Ok(mut value) => {
                value["writable"] = json!(self.writable);
                value
            }
            Err(_) => {
                json!({"schema_version":1,"writable":self.writable,"error":"saved state could not be serialised"})
            }
        }
    }

    fn envelope(&self) -> Envelope<'_, A> {
        Envelope {
            schema_version: 1,
            window: Section::new(self.preserved.get("window"), &self.window),
            widgets: Section::new(self.preserved.get("widgets"), &self.widgets),
            windows: Section::new(self.preserved.get("windows"), &self.windows),
            app: Section::new(self.preserved.get("app"), AppSection {
                version: self.version,
                value: &self.app,
            }),
        }
    }

    fn serialise(&self) -> Result<String, String> {
        strict::to_string_pretty(&self.value()?).map_err(|e| e.to_string())
    }

    fn value(&self) -> Result<Value, String> {
        serde_json::to_value(self.envelope()).map_err(|e| e.to_string())
    }

    /// Share secondary frames with toolkit viewports in this root context.
    pub fn install_windows(&self, ctx: &egui::Context, app: &str) {
        // The root store is the sole writer when it owns persistence. Child
        // passes wake it and publish frames through the context collection.
        secondary::install(ctx, app, None, self.windows.clone());
    }

    pub fn collect_windows(&mut self, ctx: &egui::Context, app: &str) {
        if let Some(windows) = secondary::collect(ctx, app) {
            self.windows = windows;
        }
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
        // Budget attempts as well as successes, so a full or unwritable disk
        // cannot turn every repaint into another atomic replacement attempt.
        self.last_write = Some(now);
        if let Some(path) = &self.path {
            let parent = path.parent().ok_or("missing saved-state directory")?;
            let held = config::atomic::create_directory(parent).map_err(|e| e.to_string())?;
            if let Err(error) = config::atomic::replace(path, content.as_bytes()) {
                let landed = error.may_have_replaced
                    && path.file_name().is_some_and(|name| {
                        config::atomic::read_in(&held, name, content.len())
                            .is_ok_and(|actual| actual.as_slice() == content.as_bytes())
                    });
                if landed {
                    self.written = content;
                    self.writes += 1;
                    return Err(format!(
                        "{error} (replacement confirmed on disk; directory durability is unconfirmed)"
                    ));
                }
                return Err(error.to_string());
            }
            self.writes += 1;
        }
        self.written = content;
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
    fn newer_app_section_survives_an_older_apps_save() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("window.conf.mix");
        std::fs::write(&path, "schema_version: 1\nwindow: {}\nwidgets: {}\napp: {version: 3, value: {future: [1, 2]}}\n").unwrap();
        let original: Value = strict::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let mut store = Store::<App>::load(Some(dir.path().into()), 2);
        assert!(!store.app_loaded());
        store.window.size = Some([800.0, 500.0]);
        assert!(store.flush().unwrap());
        let saved: Value = strict::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(saved["app"], original["app"]);
        assert_eq!(saved["window"]["size"], json!([800.0, 500.0]));
    }
    #[test]
    fn undecodable_sections_survive_a_save() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("window.conf.mix");
        for section in ["window", "widgets", "app", "windows"] {
            let mut raw = json!({"schema_version":1,"window":{},"widgets":{},"app":{"version":2,"value":{"selected":"row"}},"windows":{}});
            raw[section] = json!({"unexpected": [false, "future"]});
            if section == "window" { raw[section] = json!(false); }
            if section == "widgets" { raw[section] = json!({"panels": false}); }
            std::fs::write(&path, strict::to_string_pretty(&raw).unwrap()).unwrap();
            let mut store = Store::<App>::load(Some(dir.path().into()), 2);
            store.window.maximised = true;
            store.app.selected = "changed".into();
            store.widgets.panels.insert("left".into(), 0.4);
            store.flush().unwrap();
            let saved: Value = strict::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            assert_eq!(saved[section], raw[section], "{section}");
        }
        for version in [-1.0, 0.5] {
            let raw = json!({"schema_version":1,"app":{"version":version,"value":false}});
            std::fs::write(&path, strict::to_string_pretty(&raw).unwrap()).unwrap();
            let mut store = Store::<App>::load(Some(dir.path().into()), 2);
            store.window.maximised = true;
            store.flush().unwrap();
            let saved: Value = strict::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            assert_eq!(saved["app"], raw["app"], "an invalid version is not an older version");
        }
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
    #[test]
    fn serialisation_errors_do_not_panic_or_write() {
        #[derive(Default, serde::Deserialize)]
        struct Refuses;
        impl Serialize for Refuses {
            fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
                Err(serde::ser::Error::custom("refused"))
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::<Refuses>::load(Some(dir.path().into()), 1);
        assert!(store.flush().is_err());
        assert!(!dir.path().join("window.conf.mix").exists());
        assert!(store.describe()["error"].is_string());
        // A preserved section must not even attempt to serialise the older
        // app's unusable default while saving the independently owned frame.
        std::fs::write(dir.path().join("window.conf.mix"), "schema_version: 1\napp: {version: 2, value: {future: true}}\n").unwrap();
        let mut preserved = Store::<Refuses>::load(Some(dir.path().into()), 1);
        preserved.window.size = Some([800.0, 500.0]);
        assert!(preserved.flush().unwrap());
        assert_eq!(preserved.describe()["app"]["value"]["future"], true);
    }
    #[test]
    fn failed_write_attempts_are_coalesced_and_retry_after_repair() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::<App>::load(Some(dir.path().into()), 1);
        store.app.selected = "changed".into();
        let path = dir.path().join("window.conf.mix");
        std::fs::create_dir(&path).unwrap();
        let now = Instant::now();
        assert!(store.tick(now, false).is_err());
        assert!(!store.tick(now + Duration::from_millis(999), false).unwrap());
        assert!(store.tick(now + Duration::from_secs(1), false).is_err());
        std::fs::remove_dir(path).unwrap();
        assert!(store.tick(now + Duration::from_secs(2), false).unwrap());
        assert_eq!(store.writes(), 1);
    }
}
