//! The catalogue: which Kynoko apps exist, where they live, and which files
//! each of their facades opens (docs/SPEC.md, section 4).
//!
//! The snapshot bundled at compile time is the floor; the platform's public
//! catalogue replaces it when online (at most every 12 hours, conditional
//! requests, see `refresh`), and the last good copy is kept on disk.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Catalogue {
    pub v: u32,
    #[serde(rename = "generatedAt")]
    pub generated_at: String,
    pub apps: Vec<App>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct App {
    pub code: String,
    pub url: String,
    #[serde(default)]
    pub names: HashMap<String, String>,
    pub status: String,
    #[serde(default)]
    pub facades: Vec<Facade>,
    /// How the app fares per browser engine, as it declares it (its
    /// manifest's `browsers`, relayed by the platform).
    #[serde(default)]
    pub browsers: Browsers,
}

/// An app's word on browsers: the engines it recommends, and what the user
/// will miss in the others. Engines: `chromium` (Chrome, Edge, Brave, and the
/// Kynoko window on Windows), `gecko` (Firefox), `webkit` (Safari, and the
/// Kynoko window on macOS and Linux). `recommended` may also name `kynoko`:
/// the Kynoko window itself, whatever its engine.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
pub struct Browsers {
    #[serde(default)]
    pub recommended: Vec<String>,
    #[serde(default)]
    pub limitations: Vec<Limitation>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct Limitation {
    pub engines: Vec<String>,
    /// One sentence, per language.
    #[serde(default)]
    pub texts: HashMap<String, String>,
    /// What only browsers miss: the Kynoko window of the same engine has it,
    /// through the launcher (the computer's fonts, for one).
    #[serde(default, rename = "browsersOnly")]
    pub browsers_only: bool,
}

impl Limitation {
    pub fn text(&self, lang: &str) -> Option<String> {
        self.texts.get(lang).or_else(|| self.texts.get("en")).or_else(|| self.texts.values().next()).cloned()
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct Facade {
    pub path: String,
    #[serde(default)]
    pub names: HashMap<String, String>,
    #[serde(default)]
    pub files: Vec<TileFile>,
    /// Published by the platform: only a listed facade gets a shortcut (a
    /// draft keeps its file types, which the app's /open route serves).
    #[serde(default = "listed_by_default")]
    pub listed: bool,
}

impl Facade {
    /// The facade's last path segment (`studio/express` -> `express`): its
    /// web manifest's name, and a file name part.
    pub fn slug(&self) -> &str {
        self.path.rsplit('/').next().unwrap_or(&self.path)
    }

    pub fn name(&self, lang: &str) -> String {
        self.names.get(lang).or_else(|| self.names.get("en")).cloned().unwrap_or_else(|| self.slug().to_string())
    }
}

fn listed_by_default() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
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

/* ------------------------------------------------------------- online copy */

/// The platform's public catalogue (overridable in settings.json, `catalogueUrl`).
pub const DEFAULT_URL: &str = "https://api.kynoko.com/api/public/launcher-catalogue/";
/// A successful check holds for this long.
const PERIOD_SECS: u64 = 12 * 3600;
/// After a failure: 5 min, 15 min, 45 min... capped by the period.
const RETRY_SECS: u64 = 300;

/// What is kept on disk between runs: the last good catalogue, its ETag, and
/// when it was checked.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Cache {
    pub etag: Option<String>,
    /// Last attempt (success or not), Unix seconds.
    pub checked_at: u64,
    /// Last success (200 or 304), Unix seconds.
    pub success_at: Option<u64>,
    /// Failures in a row since the last success.
    pub failures: u32,
    pub last_error: Option<String>,
    pub catalogue: Option<Catalogue>,
}

pub fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

impl Cache {
    fn path() -> std::path::PathBuf {
        crate::settings::dir().join("catalogue.json")
    }

    pub fn load() -> Cache {
        std::fs::read_to_string(Self::path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    pub fn save(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(crate::settings::dir())?;
        let tmp = Self::path().with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec(self).expect("serializable"))?;
        std::fs::rename(tmp, Self::path())
    }

    /// When the next check is due (Unix seconds).
    pub fn due_at(&self) -> u64 {
        let wait = if self.failures == 0 {
            PERIOD_SECS
        } else {
            (RETRY_SECS * 3u64.saturating_pow(self.failures - 1)).min(PERIOD_SECS)
        };
        self.checked_at + wait
    }

    /// The catalogue to use: the last good online copy, else the bundled one.
    pub fn catalogue(&self) -> Catalogue {
        self.catalogue.clone().filter(|c| c.v == 1).unwrap_or_else(Catalogue::bundled)
    }
}

pub enum Refresh {
    Updated(Catalogue),
    Unchanged,
    Failed(String),
}

/// One conditional request. The cache records the outcome either way.
pub fn refresh(url: &str, cache: &mut Cache) -> Refresh {
    let agent = ureq::AgentBuilder::new().timeout(std::time::Duration::from_secs(20)).build();
    let mut request = agent.get(url);
    if let (Some(etag), Some(_)) = (&cache.etag, &cache.catalogue) {
        request = request.set("If-None-Match", etag);
    }
    cache.checked_at = now();
    let outcome = match request.call() {
        Ok(response) if response.status() == 304 => Ok(None),
        Ok(response) => {
            let etag = response.header("ETag").map(str::to_string);
            match response.into_json::<Catalogue>() {
                Ok(catalogue) if catalogue.v == 1 => Ok(Some((catalogue, etag))),
                Ok(_) => Err("unknown catalogue version".to_string()),
                Err(e) => Err(format!("unreadable catalogue: {e}")),
            }
        }
        Err(e) => Err(e.to_string()),
    };
    match outcome {
        Ok(fresh) => {
            cache.failures = 0;
            cache.last_error = None;
            cache.success_at = Some(cache.checked_at);
            let result = match fresh {
                Some((catalogue, etag)) if Some(&catalogue) != cache.catalogue.as_ref() => {
                    cache.etag = etag;
                    cache.catalogue = Some(catalogue.clone());
                    Refresh::Updated(catalogue)
                }
                Some((_, etag)) => {
                    cache.etag = etag;
                    Refresh::Unchanged
                }
                None => Refresh::Unchanged,
            };
            let _ = cache.save();
            result
        }
        Err(e) => {
            cache.failures = cache.failures.saturating_add(1);
            cache.last_error = Some(e.clone());
            let _ = cache.save();
            Refresh::Failed(e)
        }
    }
}

impl PartialEq for Catalogue {
    /// Same content: the generation date alone is not a change.
    fn eq(&self, other: &Self) -> bool {
        self.v == other.v && self.apps == other.apps
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

    /// The facade a file of type `ext` opens in: the one marking it primary,
    /// else the first that lists it. Its name and icon are what the system
    /// shows for that type.
    pub fn facade_for(&self, ext: &str) -> Option<&Facade> {
        let has = |f: &&Facade| f.files.iter().any(|t| t.ext == ext);
        self.facades
            .iter()
            .filter(has)
            .find(|f| f.files.iter().any(|t| t.ext == ext && t.primary))
            .or_else(|| self.facades.iter().find(has))
    }

    /// The same app, its facades listing only the file types `keep` accepts.
    pub fn keeping(&self, keep: impl Fn(&str) -> bool) -> App {
        let mut app = self.clone();
        for f in &mut app.facades {
            f.files.retain(|t| keep(&t.ext));
        }
        app
    }

    pub fn name(&self, lang: &str) -> String {
        self.names
            .get(lang)
            .or_else(|| self.names.get("en"))
            .cloned()
            .unwrap_or_else(|| self.code.clone())
    }

    /// The app as the system sees it: its word on browsers is for the window
    /// alone, a change of it rewrites no association and no shortcut.
    pub fn for_system(&self) -> App {
        App { browsers: Browsers::default(), ..self.clone() }
    }

    /// The app's name where the system lists it among other programs ("Open
    /// with"): with the brand, once. "Media Studio" is "Kynoko Media
    /// Studio"; "Kynoko Office" stays as it is.
    pub fn display_name(&self, lang: &str) -> String {
        let name = self.name(lang);
        if name.to_lowercase().contains("kynoko") {
            name
        } else {
            format!("Kynoko {name}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_platform_answer() {
        // As the API sends it: an extra `success`, empty names, a draft facade.
        let json = r#"{"success":true,"v":1,"generatedAt":"2026-09-26T00:00:00Z","apps":[
            {"code":"Office","url":"https://office.example/","names":{},"status":"live","facades":[
              {"path":"document","names":{"en":"Document"},"files":[{"ext":"docx","mime":"x"}],"listed":false}]}]}"#;
        let c: Catalogue = serde_json::from_str(json).unwrap();
        assert_eq!(c.apps[0].name("fr"), "Office");
        assert!(!c.apps[0].facades[0].listed);
        assert_eq!(c.apps[0].extensions(), ["docx"]);
    }

    #[test]
    fn reads_what_an_app_says_of_browsers() {
        let json = r#"{"code":"MediaStudio","url":"https://m.example/","names":{},"status":"live","facades":[],
            "browsers":{"recommended":["chromium"],"limitations":[{"engines":["gecko","webkit"],"texts":{"fr":"Pas de HEVC.","en":"No HEVC."}}]}}"#;
        let app: App = serde_json::from_str(json).unwrap();
        assert_eq!(app.browsers.recommended, ["chromium"]);
        assert_eq!(app.browsers.limitations[0].text("fr").as_deref(), Some("Pas de HEVC."));
        assert_eq!(app.browsers.limitations[0].text("ja").as_deref(), Some("No HEVC."));
        assert!(!app.browsers.limitations[0].browsers_only);
        // The Kynoko window recommended, and what only browsers miss.
        let office: App = serde_json::from_str(
            r#"{"code":"Office","url":"u","status":"live","browsers":{"recommended":["kynoko"],
            "limitations":[{"engines":["gecko","webkit"],"browsersOnly":true,"texts":{"en":"No fonts."}}]}}"#,
        )
        .unwrap();
        assert_eq!(office.browsers.recommended, ["kynoko"]);
        assert!(office.browsers.limitations[0].browsers_only);
        // Absent (an older platform): nothing said, nothing shown.
        let plain: App = serde_json::from_str(r#"{"code":"X","url":"u","status":"live"}"#).unwrap();
        assert!(plain.browsers.recommended.is_empty() && plain.browsers.limitations.is_empty());
        // A change of it is not a change of what the system holds.
        assert!(app.for_system() == App { browsers: Browsers::default(), ..app.clone() });
    }

    #[test]
    fn the_brand_once() {
        let app = |en: &str, fr: &str| App {
            code: "X".into(),
            url: String::new(),
            names: [("en".to_string(), en.to_string()), ("fr".to_string(), fr.to_string())].into(),
            status: "live".into(),
            facades: vec![],
            browsers: Browsers::default(),
        };
        assert_eq!(app("Media Studio", "Media Studio").display_name("en"), "Kynoko Media Studio");
        assert_eq!(app("Kynoko Office", "Kynoko Office").display_name("fr"), "Kynoko Office");
        assert_eq!(app("Game Kit", "La Panoplie").display_name("fr"), "Kynoko La Panoplie");
        assert_eq!(app("KYNOKO Games", "KYNOKO Games").display_name("en"), "KYNOKO Games");
    }

    #[test]
    fn schedule() {
        let mut cache = Cache { checked_at: 1000, ..Default::default() };
        assert_eq!(cache.due_at(), 1000 + 12 * 3600);
        cache.failures = 1;
        assert_eq!(cache.due_at(), 1000 + 300);
        cache.failures = 3;
        assert_eq!(cache.due_at(), 1000 + 2700);
        cache.failures = 30;
        assert_eq!(cache.due_at(), 1000 + 12 * 3600);
    }

    #[test]
    fn same_content_is_no_change() {
        let a = Catalogue::bundled();
        let mut b = a.clone();
        b.generated_at = "another date".into();
        assert!(a == b);
    }
}
