//! The catalogue: which Kynoko apps exist, where they live, and which files
//! each of their facades opens (docs/SPEC.md, section 4).
//!
//! This build reads the snapshot bundled at compile time; fetching the
//! platform's public catalogue (12 h, conditional requests) comes next.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Catalogue {
    pub v: u32,
    #[serde(rename = "generatedAt")]
    pub generated_at: String,
    pub apps: Vec<App>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct App {
    pub code: String,
    pub url: String,
    pub names: HashMap<String, String>,
    pub status: String,
    pub facades: Vec<Facade>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Facade {
    pub path: String,
    pub names: HashMap<String, String>,
    pub files: Vec<TileFile>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TileFile {
    pub ext: String,
    #[serde(default)]
    pub mime: Option<String>,
    #[serde(default)]
    pub primary: bool,
}

const BUNDLED: &str = include_str!("../catalogue.json");

impl Catalogue {
    pub fn bundled() -> Catalogue {
        serde_json::from_str(BUNDLED).expect("bundled catalogue is valid JSON")
    }

    pub fn app(&self, code: &str) -> Option<&App> {
        self.apps.iter().find(|a| a.code == code)
    }

    /// The app that opens `ext` among `candidates` (the apps the user
    /// associated): the one whose facade marks it primary, else the first.
    pub fn app_for_ext<'a>(&'a self, ext: &str, candidates: &[String]) -> Option<&'a App> {
        let ext = ext.to_ascii_lowercase();
        let opens = |app: &&App| app.facades.iter().any(|f| f.files.iter().any(|t| t.ext == ext));
        let pool: Vec<&App> = self
            .apps
            .iter()
            .filter(|a| a.status == "live" && candidates.iter().any(|c| c == &a.code))
            .filter(opens)
            .collect();
        pool.iter()
            .find(|a| a.facades.iter().any(|f| f.files.iter().any(|t| t.ext == ext && t.primary)))
            .or_else(|| pool.first())
            .copied()
    }
}

impl App {
    /// Every extension the app's facades open, once, in declaration order.
    pub fn extensions(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for f in &self.facades {
            for t in &f.files {
                if !out.contains(&t.ext) {
                    out.push(t.ext.clone());
                }
            }
        }
        out
    }

    pub fn name(&self, lang: &str) -> String {
        self.names
            .get(lang)
            .or_else(|| self.names.get("en"))
            .cloned()
            .unwrap_or_else(|| self.code.clone())
    }
}
