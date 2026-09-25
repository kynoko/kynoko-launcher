//! The user's choices and the INVENTORY of everything written to the system.
//!
//! Both live in the user's config directory (`Kynoko Launcher/`). The
//! inventory is written BEFORE each change to the system, so cleanup removes
//! exactly what was created, and nothing else (docs/SPEC.md, section 11).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    /// Browser id (see browsers.rs) every app follows; None = the system's default.
    pub default_browser: Option<String>,
    /// Per-app override of the browser.
    pub app_browsers: HashMap<String, String>,
    /// Profile of the default browser (None = the browser's own default).
    pub default_profile: Option<String>,
    /// Profile of an app's own browser.
    pub app_profiles: HashMap<String, String>,
    /// Apps whose file types are associated with Kynoko Launcher.
    pub associated_apps: Vec<String>,
    /// Apps that have shortcuts (a Start menu folder with their facades).
    pub shortcut_apps: Vec<String>,
    /// The online catalogue's address, when not the platform's (testing
    /// against another environment).
    pub catalogue_url: Option<String>,
    /// The window's last language: shortcuts are named in it.
    pub ui_lang: Option<String>,
    /// Where each app is opened, when not at its catalogue address (testing
    /// against another environment). Never written by the launcher itself.
    pub app_urls: HashMap<String, String>,
}

/// One thing written to the system, in the order it was written.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Artefact {
    /// A registry key under HKEY_CURRENT_USER we created (removed with its tree).
    RegistryKey { path: String },
    /// A value we added to a key that is not ours (removed alone).
    RegistryValue { path: String, name: String },
    /// A file we created.
    File { path: String },
    /// A directory we created (removed only if empty).
    Dir { path: String },
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Inventory {
    pub artefacts: Vec<Artefact>,
}

pub fn dir() -> PathBuf {
    dirs::config_dir().unwrap_or_else(std::env::temp_dir).join("Kynoko Launcher")
}

fn read<T: for<'de> Deserialize<'de> + Default>(name: &str) -> T {
    fs::read_to_string(dir().join(name))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn write<T: Serialize>(name: &str, value: &T) -> std::io::Result<()> {
    fs::create_dir_all(dir())?;
    let tmp = dir().join(format!("{name}.tmp"));
    fs::write(&tmp, serde_json::to_vec_pretty(value).expect("serializable"))?;
    fs::rename(tmp, dir().join(name))
}

impl Settings {
    pub fn load() -> Settings {
        read("settings.json")
    }
    pub fn save(&self) -> std::io::Result<()> {
        write("settings.json", self)
    }
    /// The address `app` is opened at: its own, or the one settings point it to.
    pub fn url_of(&self, app: &crate::catalogue::App) -> String {
        self.app_urls.get(&app.code).cloned().unwrap_or_else(|| app.url.clone())
    }

    /// The browser for `app`: its own, else the default (None = system default).
    pub fn browser_for(&self, app: &str) -> Option<String> {
        self.app_browsers.get(app).cloned().or_else(|| self.default_browser.clone())
    }

    /// The profile for `app`, belonging to the browser `browser_for` gives.
    pub fn profile_for(&self, app: &str) -> Option<String> {
        if self.app_browsers.contains_key(app) {
            self.app_profiles.get(app).cloned()
        } else {
            self.default_profile.clone()
        }
    }
}

impl Inventory {
    pub fn load() -> Inventory {
        read("inventory.json")
    }
    /// Records `artefact` (once) and persists the inventory right away.
    pub fn record(&mut self, artefact: Artefact) -> std::io::Result<()> {
        if !self.artefacts.contains(&artefact) {
            self.artefacts.push(artefact);
        }
        write("inventory.json", self)
    }
    pub fn forget(&mut self, artefact: &Artefact) -> std::io::Result<()> {
        self.artefacts.retain(|a| a != artefact);
        write("inventory.json", self)
    }
}

/// Removes the launcher's own state (settings and inventory). Last step of
/// the cleanup, once every artefact is gone.
pub fn remove_own_state() {
    let _ = fs::remove_file(dir().join("settings.json"));
    let _ = fs::remove_file(dir().join("inventory.json"));
    let _ = fs::remove_file(dir().join("catalogue.json"));
    // Emptied by the cleanup (every icon is in the inventory).
    let _ = fs::remove_dir(dir().join("icons"));
    let _ = fs::remove_dir(dir());
}
