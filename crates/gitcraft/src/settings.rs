//! Preferences, open tabs and drafts, kept in the `settings.json` GitCraft has always
//! used, so earlier installations keep their preferences.
//!
//! The document is held as JSON and only the keys this interface knows are read or changed:
//! everything else (drafts, profiles, preferences from other versions) is written back
//! exactly as it was read.

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// Wait this long after the last change before writing, so typing doesn't write on every key.
const SAVE_AFTER: Duration = Duration::from_millis(400);

/// The zoom steps offered by View > Increase and Decrease Zoom (metrics `zoom_factors`).
pub const ZOOM_FACTORS: [f32; 12] = [3.0, 2.5, 2.0, 1.75, 1.5, 1.4, 1.3, 1.2, 1.1, 1.0, 0.9, 0.8];

pub struct Store {
    /// `None` for picture-taking runs, which must neither read nor change the user's file.
    path: Option<PathBuf>,
    doc: Value,
    changed: Option<Instant>,
    /// Bumped by every change, so others can tell the preferences moved.
    pub revision: u64,
    /// Shown once at start-up: what was recovered or could not be read.
    pub warning: Option<String>,
    /// False in a repository window of its own, whose tabs aren't the ones to reopen.
    pub shares_tabs: bool,
}

pub fn file() -> PathBuf {
    gitcraft_core::sample::data_dir().join("settings.json")
}

impl Store {
    pub fn open() -> Store {
        let path = file();
        let (doc, warning) = match gitcraft_core::settings_file::load(&path) {
            Ok(Value::Null) => (Value::Object(Map::new()), None),
            Ok(mut doc) => {
                let warning = doc.as_object_mut().and_then(|o| o.remove("recoveryWarning")).and_then(|w| w.as_str().map(str::to_owned));
                (doc, warning)
            }
            Err(e) => (Value::Object(Map::new()), Some(e)),
        };
        // A file that could not be read is left alone rather than overwritten with defaults.
        let path = if warning.as_deref().is_some_and(|w| w.starts_with("Could not read")) { None } else { Some(path) };
        Store { path, doc, changed: None, revision: 1, warning, shares_tabs: true }
    }

    /// Defaults only, saved nowhere.
    pub fn in_memory() -> Store {
        Store { path: None, doc: Value::Object(Map::new()), changed: None, revision: 1, warning: None, shares_tabs: true }
    }

    fn settings(&self) -> Option<&Map<String, Value>> {
        self.doc.get("settings")?.as_object()
    }

    /// A preference, or `None` when it is missing or of another type.
    pub fn get<T: DeserializeOwned>(&self, key: &str) -> Option<T> {
        serde_json::from_value(self.settings()?.get(key)?.clone()).ok()
    }

    pub fn set(&mut self, key: &str, value: impl Serialize) {
        let Ok(value) = serde_json::to_value(value) else { return };
        let root = object(&mut self.doc);
        let settings = object(root.entry("settings").or_insert_with(|| Value::Object(Map::new())));
        if settings.get(key) != Some(&value) {
            settings.insert(key.to_owned(), value);
            self.touch();
        }
    }

    /// A number preference.
    pub fn num(&self, key: &str, default: f64) -> f64 {
        self.get::<f64>(key).filter(|n| n.is_finite()).unwrap_or(default)
    }

    /// A text preference.
    pub fn string(&self, key: &str, default: &str) -> String {
        self.get::<String>(key).unwrap_or_else(|| default.to_owned())
    }

    /// A key at the top of the document (beside `settings`), such as `drafts` or `profiles`.
    pub fn get_root<T: DeserializeOwned>(&self, key: &str) -> Option<T> {
        serde_json::from_value(self.doc.get(key)?.clone()).ok()
    }

    pub fn set_root(&mut self, key: &str, value: impl Serialize) {
        let Ok(value) = serde_json::to_value(value) else { return };
        let root = object(&mut self.doc);
        if root.get(key) != Some(&value) {
            root.insert(key.to_owned(), value);
            self.touch();
        }
    }

    pub fn flag(&self, key: &str, default: bool) -> bool {
        self.get(key).unwrap_or(default)
    }

    pub fn toggle(&mut self, key: &str, default: bool) {
        let now = !self.flag(key, default);
        self.set(key, now);
    }

    pub fn theme(&self) -> String {
        self.get("theme").unwrap_or_else(|| "system".into())
    }

    pub fn zoom(&self) -> f32 {
        self.get::<f32>("zoom").filter(|z| z.is_finite() && *z > 0.0).unwrap_or(1.0)
    }

    pub fn recents(&self) -> Vec<String> {
        self.get("recents").unwrap_or_default()
    }

    /// The alias the user gave a repository tab, if any.
    pub fn alias(&self, path: &str) -> Option<String> {
        self.get::<Map<String, Value>>("aliases")?.get(path)?.as_str().filter(|a| !a.is_empty()).map(str::to_owned)
    }

    pub fn tab_width(&self, path: &str) -> Option<f32> {
        self.get::<Map<String, Value>>("tabWidths")?.get(path)?.as_f64().map(|w| w as f32)
    }

    pub fn set_tab_width(&mut self, path: &str, width: f32) {
        let mut widths: Map<String, Value> = self.get("tabWidths").unwrap_or_default();
        widths.insert(path.to_owned(), Value::from(width.round()));
        self.set("tabWidths", widths);
    }

    fn profiles(&self) -> &[Value] {
        self.doc.get("profiles").and_then(Value::as_array).map_or(&[], Vec::as_slice)
    }

    pub fn profile_names(&self) -> Vec<String> {
        self.profiles().iter().filter_map(|p| p.get("name")?.as_str().map(str::to_owned)).collect()
    }

    pub fn active_profile_index(&self) -> usize {
        let active = self.doc.get("activeProfile");
        self.profiles().iter().position(|p| p.get("id") == active).unwrap_or(0)
    }

    /// The name and icon number of the profile in use.
    pub fn active_profile(&self) -> (String, usize) {
        let profile = self.profiles().get(self.active_profile_index());
        let name = profile.and_then(|p| p.get("name")?.as_str()).unwrap_or("Default Profile").to_owned();
        let icon = profile.and_then(|p| p.get("icon")?.as_u64()).unwrap_or(0) as usize;
        (name, icon)
    }

    /// The repository tabs that were open last time, and which of them was in front.
    pub fn tabs(&self) -> (Vec<String>, usize) {
        let paths = self.doc.get("tabs").and_then(Value::as_array).map(|tabs| tabs.iter().filter_map(|t| t.as_str().map(str::to_owned)).collect()).unwrap_or_default();
        let active = self.doc.get("active").and_then(Value::as_u64).unwrap_or(0) as usize;
        (paths, active)
    }

    pub fn set_tabs(&mut self, paths: Vec<String>, active: usize) {
        if !self.shares_tabs {
            return;
        }
        let root = object(&mut self.doc);
        let tabs = Value::from(paths);
        let active = Value::from(active);
        if root.get("tabs") != Some(&tabs) || root.get("active") != Some(&active) {
            root.insert("tabs".into(), tabs);
            root.insert("active".into(), active);
            self.touch();
        }
    }

    fn touch(&mut self) {
        self.changed = Some(Instant::now());
        self.revision += 1;
    }

    /// The whole preferences object (for profiles and backups).
    pub fn settings_value(&self) -> Value {
        self.doc.get("settings").cloned().unwrap_or_else(|| Value::Object(Map::new()))
    }

    pub fn replace_settings(&mut self, value: Value) {
        object(&mut self.doc).insert("settings".into(), value);
        self.touch();
    }

    /// Once per frame: write a moment after the last change.
    pub fn save_when_due(&mut self, ctx: &egui::Context) -> Result<(), String> {
        let Some(changed) = self.changed else { return Ok(()) };
        if changed.elapsed() < SAVE_AFTER {
            ctx.request_repaint_after(SAVE_AFTER - changed.elapsed());
            return Ok(());
        }
        self.flush()
    }

    /// Write what has not been written yet.
    pub fn flush(&mut self) -> Result<(), String> {
        if self.changed.take().is_none() {
            return Ok(());
        }
        match &self.path {
            Some(path) => gitcraft_core::settings_file::save(path, &self.doc),
            None => Ok(()),
        }
    }
}

/// The object inside `value`, replacing anything that is not an object.
fn object(value: &mut Value) -> &mut Map<String, Value> {
    if !value.is_object() {
        *value = Value::Object(Map::new());
    }
    value.as_object_mut().expect("just made an object")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_keys_survive_a_change() {
        let mut store = Store::in_memory();
        store.doc = serde_json::json!({ "settings": { "zoom": 1.2, "externalEditor": "code" }, "drafts": { "/r": { "summary": "wip" } }, "profiles": [] });
        store.set("zoom", 1.5);
        store.set_tabs(vec!["/r".into()], 0);
        assert_eq!(store.doc["settings"]["externalEditor"], "code");
        assert_eq!(store.doc["drafts"]["/r"]["summary"], "wip");
        assert_eq!(store.zoom(), 1.5);
        assert_eq!(store.tabs(), (vec!["/r".to_owned()], 0));
    }

    #[test]
    fn missing_or_wrong_values_fall_back_to_the_defaults() {
        let mut store = Store::in_memory();
        assert_eq!(store.theme(), "system");
        assert_eq!(store.zoom(), 1.0);
        assert!(store.flag("showLeft", true));
        store.doc = serde_json::json!({ "settings": { "zoom": "large", "showLeft": 3 } });
        assert_eq!(store.zoom(), 1.0);
        assert!(store.flag("showLeft", true));
    }

    #[test]
    fn setting_the_same_value_is_not_a_change() {
        let mut store = Store::in_memory();
        store.set("theme", "dark");
        store.changed = None;
        store.set("theme", "dark");
        assert!(store.changed.is_none());
    }
}
