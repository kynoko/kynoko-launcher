//! Linux integration (docs/SPEC.md, sections 5 to 7): browsers from their
//! desktop entries, associations through a desktop entry per app, a
//! shared-mime-info package and `mimeapps.list`, shortcuts as desktop entries
//! whose actions are the app's facades. Everything user-level (~/.local,
//! ~/.config), everything recorded in the inventory before it is written.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::browsers::{engine_of, Browser, Engine, Profile};
use crate::catalogue::App;
use crate::settings::{Artefact, Inventory, Settings};
use crate::xdg;

fn home() -> PathBuf {
    dirs::home_dir().unwrap_or_default()
}

fn data_home() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".local/share"))
}

fn applications() -> PathBuf {
    data_home().join("applications")
}

fn mimeapps() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".config")).join("mimeapps.list")
}

/// The program to register: the AppImage file itself when run from one (the
/// running binary lives in a mount that disappears when it exits).
fn exe() -> String {
    std::env::var("APPIMAGE")
        .ok()
        .filter(|p| !p.is_empty())
        .unwrap_or_else(|| std::env::current_exe().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default())
}

/* ---------------------------------------------------------------- browsers */

fn app_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![applications()];
    let system = std::env::var("XDG_DATA_DIRS").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    dirs.extend(system.split(':').map(|d| Path::new(d).join("applications")));
    dirs.push(home().join(".local/share/flatpak/exports/share/applications"));
    dirs.push(PathBuf::from("/var/lib/flatpak/exports/share/applications"));
    dirs.push(PathBuf::from("/var/lib/snapd/desktop/applications"));
    dirs
}

pub fn browsers() -> Vec<Browser> {
    let mut out: Vec<Browser> = Vec::new();
    for dir in app_dirs() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(id) = path.file_name().map(|n| n.to_string_lossy().into_owned()) else { continue };
            // A desktop id found first (user dirs come first) hides the same id later.
            if !id.ends_with(".desktop") || id.starts_with("kynoko-") || out.iter().any(|b| b.id == id) {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else { continue };
            let e = xdg::desktop_entry(&text);
            let is_browser = e.get("MimeType").map(|m| m.split(';').any(|t| t == "x-scheme-handler/https")).unwrap_or(false);
            let hidden = ["NoDisplay", "Hidden"].iter().any(|k| e.get(*k).map(|v| v == "true").unwrap_or(false));
            if !is_browser || hidden || e.get("Type").map(|t| t != "Application").unwrap_or(false) {
                continue;
            }
            let Some(exec) = e.get("Exec") else { continue };
            let command = xdg::exec_argv(exec);
            if command.is_empty() {
                continue;
            }
            let program = xdg::program_of(&command);
            let engine = engine_of(&program);
            let flatpak = command.first().map(|c| c.ends_with("flatpak")).unwrap_or(false);
            let flatpak_id = if flatpak { command.iter().skip_while(|a| *a != "run").skip(1).find(|a| !a.starts_with('-')).cloned() } else { None };
            let profiles = profiles(&engine, &program, flatpak_id.as_deref());
            out.push(Browser {
                id,
                name: e.get("Name").cloned().unwrap_or(program.clone()),
                exe: command[0].clone(),
                engine,
                profiles,
                command,
            });
        }
    }
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out
}

/// Where a browser keeps its profiles on Linux (a Flatpak keeps them in its sandbox).
fn profiles(engine: &Engine, program: &str, flatpak_id: Option<&str>) -> Vec<Profile> {
    let base = match flatpak_id {
        Some(id) => home().join(".var/app").join(id),
        None => home(),
    };
    let config = if flatpak_id.is_some() { base.join("config") } else { base.join(".config") };
    let stem = program.trim_end_matches("-stable").trim_end_matches("-beta").trim_end_matches("-unstable");
    match engine {
        Engine::Chromium => {
            let dir = match stem {
                "google-chrome" | "chrome" => "google-chrome",
                "chromium" | "chromium-browser" => "chromium",
                "microsoft-edge" | "edge" => "microsoft-edge",
                "brave-browser" | "brave" => "BraveSoftware/Brave-Browser",
                "vivaldi" => "vivaldi",
                _ => return Vec::new(),
            };
            std::fs::read_to_string(config.join(dir).join("Local State"))
                .ok()
                .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
                .map(|state| crate::browsers::parse_local_state(&state))
                .unwrap_or_default()
        }
        Engine::Gecko => {
            let dir = match stem {
                "firefox" | "firefox-esr" => base.join(".mozilla/firefox"),
                "librewolf" => base.join(".librewolf"),
                "waterfox" => base.join(".waterfox"),
                _ => return Vec::new(),
            };
            let profiles = std::fs::read_to_string(dir.join("profiles.ini")).unwrap_or_default();
            crate::browsers::parse_profiles_ini(&profiles, "", "")
        }
        _ => Vec::new(),
    }
}

/* ------------------------------------------------------------ associations */

fn open_entry(app: &App) -> String {
    format!("kynoko-launcher-open-{}.desktop", app.code)
}

const URL_ENTRY: &str = "kynoko-launcher-url.desktop";

/// `(mime, ext)` for every file type of `app`; a type without a MIME name gets
/// one of ours (`application/x-kynoko-<ext>`).
fn types(app: &App) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for f in app.facades.iter().flat_map(|f| f.files.iter()) {
        let mime = f.mime.clone().unwrap_or_else(|| format!("application/x-kynoko-{}", f.ext));
        if !out.iter().any(|(_, e)| *e == f.ext) {
            out.push((mime, f.ext.clone()));
        }
    }
    out
}

fn write_recorded(path: &Path, text: &str, inventory: &mut Inventory) -> std::io::Result<()> {
    inventory.record(Artefact::File { path: path.to_string_lossy().into_owned() })?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, text)
}

fn refresh_databases() {
    let _ = Command::new("update-mime-database").arg(data_home().join("mime")).status();
    let _ = Command::new("update-desktop-database").arg(applications()).status();
}

pub fn register(app: &App, settings: &Settings, inventory: &mut Inventory) -> std::io::Result<()> {
    let pairs = types(app);
    let package = data_home().join("mime/packages").join(format!("kynoko-launcher-{}.xml", app.code));
    write_recorded(&package, &xdg::mime_package(&pairs), inventory)?;

    let mimes: Vec<&str> = pairs.iter().map(|(m, _)| m.as_str()).collect();
    let entry = open_entry(app);
    let name = format!("{} (Kynoko Launcher)", app.name(settings.ui_lang.as_deref().unwrap_or("en")));
    let text = xdg::render_entry(
        &[
            ("Type", "Application".into()),
            ("Name", name),
            ("Exec", format!("{} open %F", xdg::exec_quote(&exe()))),
            ("MimeType", format!("{};", mimes.join(";"))),
            ("NoDisplay", "true".into()),
        ],
        &[],
    );
    write_recorded(&applications().join(&entry), &text, inventory)?;

    // Default handler of each type, the previous one kept to be restored.
    let mut list = std::fs::read_to_string(mimeapps()).unwrap_or_default();
    for mime in mimes {
        let previous = xdg::mimeapps_get(&list, xdg::defaults_group(), mime);
        if previous.as_deref() != Some(entry.as_str()) {
            inventory.record(Artefact::MimeDefault { mime: mime.to_string(), desktop: entry.clone(), previous })?;
        }
        list = xdg::mimeapps_set(&list, xdg::defaults_group(), mime, Some(&entry));
        list = xdg::mimeapps_added(&list, mime, &entry, true);
    }
    write_mimeapps(&list)?;
    refresh_databases();
    Ok(())
}

fn write_mimeapps(list: &str) -> std::io::Result<()> {
    let path = mimeapps();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, list)
}

/// Puts back what was the default before us, unless the user changed it since.
pub fn restore_default(mime: &str, desktop: &str, previous: Option<&str>) {
    let Ok(list) = std::fs::read_to_string(mimeapps()) else { return };
    let mut next = xdg::mimeapps_added(&list, mime, desktop, false);
    if xdg::mimeapps_get(&next, xdg::defaults_group(), mime).as_deref() == Some(desktop) {
        next = xdg::mimeapps_set(&next, xdg::defaults_group(), mime, previous);
    }
    if next != list {
        let _ = write_mimeapps(&next);
    }
}

/// Removes what `register` wrote for `app`: its entry, its package, its defaults.
pub fn unregister(app: &App, inventory: &mut Inventory) -> std::io::Result<()> {
    let entry = open_entry(app);
    for a in inventory.artefacts.clone() {
        match &a {
            Artefact::MimeDefault { mime, desktop, previous } if *desktop == entry => {
                restore_default(mime, desktop, previous.as_deref());
                inventory.forget(&a)?;
            }
            Artefact::File { path } if path.ends_with(&entry) || path.ends_with(&format!("kynoko-launcher-{}.xml", app.code)) => {
                let _ = std::fs::remove_file(path);
                inventory.forget(&a)?;
            }
            _ => {}
        }
    }
    refresh_databases();
    Ok(())
}

pub fn register_scheme(inventory: &mut Inventory) -> std::io::Result<()> {
    let scheme = format!("x-scheme-handler/{}", crate::assoc::SCHEME);
    let text = xdg::render_entry(
        &[
            ("Type", "Application".into()),
            ("Name", "Kynoko Launcher".into()),
            ("Exec", format!("{} %u", xdg::exec_quote(&exe()))),
            ("MimeType", format!("{scheme};")),
            ("NoDisplay", "true".into()),
        ],
        &[],
    );
    write_recorded(&applications().join(URL_ENTRY), &text, inventory)?;
    let list = std::fs::read_to_string(mimeapps()).unwrap_or_default();
    let previous = xdg::mimeapps_get(&list, xdg::defaults_group(), &scheme);
    if previous.as_deref() != Some(URL_ENTRY) {
        inventory.record(Artefact::MimeDefault { mime: scheme.clone(), desktop: URL_ENTRY.into(), previous })?;
        write_mimeapps(&xdg::mimeapps_set(&list, xdg::defaults_group(), &scheme, Some(URL_ENTRY)))?;
    }
    Ok(())
}

pub fn after_cleanup() {
    refresh_databases();
}

/* --------------------------------------------------------------- shortcuts */

/// One desktop entry per app, its listed facades as actions (the right-click
/// submenu of GNOME and KDE). Icons: the manifests' PNGs, as is.
pub fn create_shortcut(app: &App, settings: &Settings, lang: &str, inventory: &mut Inventory, icon: impl Fn(&str, &str) -> Option<PathBuf>) -> std::io::Result<()> {
    let base = settings.url_of(app);
    let base = base.trim_end_matches('/');
    let exe = xdg::exec_quote(&exe());
    let actions: Vec<(String, String, String)> = app
        .facades
        .iter()
        .filter(|f| f.listed)
        .map(|f| {
            let id: String = f.path.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
            let name = f.names.get(lang).or_else(|| f.names.get("en")).cloned().unwrap_or_else(|| f.path.clone());
            (id, name, format!("{exe} launch {}/{}", app.code, f.path))
        })
        .collect();
    let mut fields = vec![
        ("Type", "Application".to_string()),
        ("Name", app.name(lang)),
        ("Exec", format!("{exe} launch {}", app.code)),
        ("Categories", "Office;".to_string()),
    ];
    if let Some(png) = icon(&format!("{base}/manifest.webmanifest"), &app.code) {
        fields.push(("Icon", png.to_string_lossy().into_owned()));
    }
    let path = applications().join(format!("kynoko-{}.desktop", app.code));
    write_recorded(&path, &xdg::render_entry(&fields, &actions), inventory)?;
    let _ = Command::new("update-desktop-database").arg(applications()).status();
    Ok(())
}

pub fn shortcut_paths(app: &App) -> Vec<String> {
    vec![applications().join(format!("kynoko-{}.desktop", app.code)).to_string_lossy().into_owned()]
}
