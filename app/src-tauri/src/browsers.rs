//! The browsers installed on the computer, as the system declares them
//! (docs/SPEC.md, section 5): never a hard-coded list, so every channel and
//! fork (Firefox Nightly, Chrome Canary, LibreWolf...) shows up by itself.

use serde::Serialize;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Engine {
    Chromium,
    Gecko,
    Webkit,
    Unknown,
    /// Not a browser: the launcher's own window (see lib.rs, open_embedded).
    Embedded,
}

#[derive(Debug, Clone, Serialize)]
pub struct Browser {
    /// Stable id: the system's own key for it (registry key name on Windows).
    pub id: String,
    pub name: String,
    pub exe: String,
    pub engine: Engine,
    /// The engine it renders pages with, which is what apps speak of (their
    /// recommendations and limitations, see catalogue::Browsers). Opera is
    /// Chromium there, though `engine` keeps it unknown for opening windows;
    /// the Kynoko window is WebView2 on Windows, WebKit elsewhere.
    #[serde(rename = "webEngine")]
    pub web_engine: Engine,
    /// The browser's profiles. The Kynoko session and the Local Network
    /// Access permission belong to a profile, so the profile is part of the
    /// choice (docs/SPEC.md, section 5).
    pub profiles: Vec<Profile>,
    /// How to start it: the program and its fixed arguments (`flatpak run
    /// org.mozilla.firefox` on Linux), before the launcher's own.
    #[serde(skip)]
    pub command: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Profile {
    /// What the browser is launched with: the profile directory (Chromium)
    /// or the profile name (Gecko).
    pub id: String,
    pub name: String,
    /// The one the browser opens by itself.
    pub default: bool,
}

/// The engine `exe` (a program, a bundle, a command line) renders pages with,
/// knowing `engine`, the one it is opened as (see Browser::web_engine).
pub fn web_engine_of(engine: &Engine, exe: &str) -> Engine {
    match engine {
        Engine::Unknown if exe.to_ascii_lowercase().contains("opera") => Engine::Chromium,
        Engine::Embedded if cfg!(windows) => Engine::Chromium,
        Engine::Embedded => Engine::Webkit,
        other => other.clone(),
    }
}

/// The engine, from the executable's name: it decides how a window is opened.
pub fn engine_of(exe: &str) -> Engine {
    let file = exe.rsplit(['\\', '/']).next().unwrap_or(exe).to_ascii_lowercase();
    let mut stem = file.trim_end_matches(".exe");
    // Linux packages name channels as suffixes (google-chrome-stable, firefox-esr).
    for suffix in ["-stable", "-beta", "-dev", "-unstable", "-nightly", "-esr", "-canary", "-bin"] {
        stem = stem.strip_suffix(suffix).unwrap_or(stem);
    }
    match stem {
        "chrome" | "google-chrome" | "msedge" | "microsoft-edge" | "edge" | "brave" | "brave-browser" | "vivaldi" | "chromium"
        | "chromium-browser" | "thorium" | "yandex" | "yandex-browser" => Engine::Chromium,
        // Opera is Chromium, but whether it honours --app is unverified: an
        // app switch it ignored would open a blank window. A plain window,
        // which opens the page, until someone has checked.
        "opera" | "launcher" => Engine::Unknown,
        "firefox" | "librewolf" | "waterfox" | "zen" | "zen-browser" | "floorp" | "mullvadbrowser" | "icecat" => Engine::Gecko,
        "safari" | "epiphany" => Engine::Webkit,
        _ => Engine::Unknown,
    }
}

#[cfg(windows)]
pub fn installed() -> Vec<Browser> {
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
    use winreg::RegKey;

    let mut out: Vec<Browser> = Vec::new();
    let roots = [
        (HKEY_CURRENT_USER, r"SOFTWARE\Clients\StartMenuInternet"),
        (HKEY_LOCAL_MACHINE, r"SOFTWARE\Clients\StartMenuInternet"),
        (HKEY_LOCAL_MACHINE, r"SOFTWARE\WOW6432Node\Clients\StartMenuInternet"),
    ];
    for (hive, path) in roots {
        let Ok(clients) = RegKey::predef(hive).open_subkey(path) else { continue };
        for id in clients.enum_keys().flatten() {
            if out.iter().any(|b| b.id == id) {
                continue;
            }
            let Ok(key) = clients.open_subkey(&id) else { continue };
            let name: String = key
                .open_subkey("Capabilities")
                .and_then(|c| c.get_value("ApplicationName"))
                .or_else(|_| key.get_value(""))
                .unwrap_or_else(|_| id.clone());
            let Ok(command) = key.open_subkey(r"shell\open\command").and_then(|c| c.get_value::<String, _>("")) else {
                continue;
            };
            let exe = executable_of(&command);
            let engine = engine_of(&exe);
            let profiles = match engine {
                Engine::Chromium => chromium_profiles(&exe),
                Engine::Gecko => gecko_profiles(&id, &exe),
                _ => Vec::new(),
            };
            let web_engine = web_engine_of(&engine, &exe);
            out.push(Browser { engine, web_engine, id, name: resolve_indirect(name), command: vec![exe.clone()], exe, profiles });
        }
    }
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out
}

/// The launcher's own window, offered with the browsers: an app window on
/// every system, whatever browser is installed.
pub const EMBEDDED: &str = "kynoko-window";

pub fn embedded() -> Browser {
    Browser {
        id: EMBEDDED.to_string(),
        name: "Kynoko Launcher".to_string(),
        exe: String::new(),
        engine: Engine::Embedded,
        web_engine: web_engine_of(&Engine::Embedded, ""),
        profiles: Vec::new(),
        command: Vec::new(),
    }
}

/// The browser Windows opens web links with, when it is one of `installed`:
/// "the system's browser" then gets the same treatment as a chosen one (an
/// app window for Chromium, a Taskbar Tab for Firefox).
#[cfg(windows)]
pub fn system_default() -> Option<Browser> {
    use winreg::enums::{HKEY_CLASSES_ROOT, HKEY_CURRENT_USER};
    use winreg::RegKey;
    let choice = r"Software\Microsoft\Windows\Shell\Associations\UrlAssociations\https";
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let prog_id: String = ["UserChoiceLatest", "UserChoice"]
        .iter()
        .find_map(|k| hkcu.open_subkey(format!(r"{choice}\{k}")).and_then(|key| key.get_value("ProgId")).ok())?;
    let command: String = RegKey::predef(HKEY_CLASSES_ROOT)
        .open_subkey(format!(r"{prog_id}\shell\open\command"))
        .and_then(|k| k.get_value(""))
        .ok()?;
    let exe = executable_of(&command);
    installed().into_iter().find(|b| b.exe.eq_ignore_ascii_case(&exe))
}

/// Linux: the desktop entry the desktop opens web links with
/// (`xdg-settings`, else `xdg-mime`), when it is one of `installed`.
#[cfg(target_os = "linux")]
pub fn system_default() -> Option<Browser> {
    let ask = |program: &str, args: &[&str]| {
        std::process::Command::new(program)
            .args(args)
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|id| id.ends_with(".desktop"))
    };
    let id = ask("xdg-settings", &["get", "default-web-browser"]).or_else(|| ask("xdg-mime", &["query", "default", "x-scheme-handler/https"]))?;
    installed().into_iter().find(|b| b.id == id)
}

#[cfg(not(any(windows, target_os = "linux")))]
pub fn system_default() -> Option<Browser> {
    None
}

/// The engine names apps speak of (catalogue::Browsers).
fn engine_name(engine: &Engine) -> Option<&'static str> {
    match engine {
        Engine::Chromium => Some("chromium"),
        Engine::Gecko => Some("gecko"),
        Engine::Webkit => Some("webkit"),
        _ => None,
    }
}

/// Whether an app recommending `recommended` recommends `b`: by the engine
/// it renders with, or, for the Kynoko window, by name (`kynoko`). The
/// window's own rule (main.ts, recommends).
pub fn recommends(recommended: &[String], b: &Browser) -> bool {
    if b.id == EMBEDDED && recommended.iter().any(|r| r == "kynoko") {
        return true;
    }
    engine_name(&b.web_engine).is_some_and(|e| recommended.iter().any(|r| r == e))
}

/// The browser an app is given when the launcher first meets it (docs/SPEC.md,
/// section 5): None when the one it would follow (`current`: the default, or
/// the system's) is one it recommends, or when none it recommends is
/// installed; else the first it recommends, in its order, the usual stable
/// browser of that engine first (one opening app windows before Opera).
pub fn recommended_for(recommended: &[String], current: Option<&Browser>, installed: &[Browser]) -> Option<String> {
    if recommended.is_empty() || current.is_some_and(|b| recommends(recommended, b)) {
        return None;
    }
    for r in recommended {
        if r == "kynoko" {
            return Some(EMBEDDED.to_string());
        }
        let mut fit: Vec<&Browser> =
            installed.iter().filter(|b| b.id != EMBEDDED && engine_name(&b.web_engine) == Some(r.as_str())).collect();
        fit.sort_by_key(|b| preference(b));
        if let Some(b) = fit.first() {
            return Some(b.id.clone());
        }
    }
    None
}

/// Lower first: the browser people usually mean by an engine.
fn preference(b: &Browser) -> u8 {
    let n = b.name.to_ascii_lowercase();
    let usual = ["chrome", "firefox", "edge", "brave", "chromium", "vivaldi", "safari"]
        .iter()
        .position(|w| n.contains(w) && !(*w == "chrome" && n.contains("chromium")))
        .unwrap_or(7) as u8;
    let channel = ["canary", "beta", "dev", "nightly", "unstable", "developer"].iter().any(|c| n.contains(c));
    usual + if channel { 10 } else { 0 } + if b.engine == Engine::Unknown { 20 } else { 0 }
}

#[cfg(target_os = "linux")]
pub fn installed() -> Vec<Browser> {
    crate::linux::browsers()
}

#[cfg(target_os = "macos")]
pub fn installed() -> Vec<Browser> {
    crate::macos::browsers()
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
pub fn installed() -> Vec<Browser> {
    // macOS (LaunchServices) and Linux (.desktop files): next milestone.
    Vec::new()
}

/// Chromium keeps its profiles in `<LocalAppData>\<vendor>\<product>\User Data`,
/// the same vendor\product the program is installed under (per machine or per
/// user): `...\Google\Chrome SxS\Application\chrome.exe` -> `Google\Chrome SxS`.
#[cfg_attr(not(windows), allow(dead_code))]
fn chromium_user_data(exe: &str, local_app_data: &std::path::Path) -> Option<std::path::PathBuf> {
    let parts: Vec<&str> = exe.split(['\\', '/']).collect();
    let app = parts.iter().position(|p| p.eq_ignore_ascii_case("Application"))?;
    if app < 2 {
        return None;
    }
    Some(local_app_data.join(parts[app - 2]).join(parts[app - 1]).join("User Data"))
}

#[cfg(windows)]
fn chromium_profiles(exe: &str) -> Vec<Profile> {
    let Some(local) = dirs::data_local_dir() else { return Vec::new() };
    let Some(dir) = chromium_user_data(exe, &local) else { return Vec::new() };
    let Ok(text) = std::fs::read_to_string(dir.join("Local State")) else { return Vec::new() };
    let Ok(state) = serde_json::from_str::<serde_json::Value>(&text) else { return Vec::new() };
    parse_local_state(&state)
}

#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn parse_local_state(state: &serde_json::Value) -> Vec<Profile> {
    let last = state["profile"]["last_used"].as_str().unwrap_or("Default");
    let mut out: Vec<Profile> = state["profile"]["info_cache"]
        .as_object()
        .map(|cache| {
            cache
                .iter()
                .map(|(dir, info)| Profile {
                    id: dir.clone(),
                    name: info["name"].as_str().unwrap_or(dir).to_string(),
                    default: dir == last,
                })
                .collect()
        })
        .unwrap_or_default();
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out
}

/// Where each Gecko browser keeps profiles.ini, under the roaming AppData.
#[cfg_attr(not(windows), allow(dead_code))]
fn gecko_home(exe: &str) -> Option<&'static str> {
    let file = exe.rsplit(['\\', '/']).next().unwrap_or(exe).to_ascii_lowercase();
    match file.trim_end_matches(".exe") {
        "firefox" => Some(r"Mozilla\Firefox"),
        "librewolf" => Some("librewolf"),
        "waterfox" => Some("Waterfox"),
        "floorp" => Some("Floorp"),
        "zen" => Some("zen"),
        _ => None,
    }
}

#[cfg(windows)]
fn gecko_profiles(id: &str, exe: &str) -> Vec<Profile> {
    let Some(home) = gecko_home(exe).and_then(|h| dirs::config_dir().map(|c| c.join(h))) else { return Vec::new() };
    let profiles = std::fs::read_to_string(home.join("profiles.ini")).unwrap_or_default();
    let installs = std::fs::read_to_string(home.join("installs.ini")).unwrap_or_default();
    parse_profiles_ini(&profiles, &installs, id)
}

/// Profiles from profiles.ini; the default is the one installs.ini gives THIS
/// install (Firefox and Firefox Nightly share the file, not the default). The
/// install hash is the tail of the registry id (`Firefox-308046B0AF4A39CB`).
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn parse_profiles_ini(profiles: &str, installs: &str, browser_id: &str) -> Vec<Profile> {
    type Section = (String, std::collections::HashMap<String, String>);
    let sections = |text: &str| -> Vec<Section> {
        let mut out: Vec<Section> = Vec::new();
        for line in text.lines().map(str::trim) {
            if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                out.push((name.to_string(), Default::default()));
            } else if let (Some((k, v)), Some(last)) = (line.split_once('='), out.last_mut()) {
                last.1.insert(k.to_string(), v.to_string());
            }
        }
        out
    };
    let hash = browser_id.rsplit('-').next().unwrap_or(browser_id);
    let install_default = sections(installs)
        .into_iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(hash))
        .and_then(|(_, kv)| kv.get("Default").cloned());
    let mut out: Vec<Profile> = sections(profiles)
        .into_iter()
        .filter(|(name, kv)| name.starts_with("Profile") && kv.contains_key("Name"))
        .map(|(_, kv)| {
            let path = kv.get("Path").cloned().unwrap_or_default();
            let default = match &install_default {
                Some(d) => *d == path,
                None => kv.get("Default").map(|v| v == "1").unwrap_or(false),
            };
            Profile { id: kv["Name"].clone(), name: kv["Name"].clone(), default }
        })
        .collect();
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out
}

/// `"C:\Program Files\X\x.exe" --arg` -> `C:\Program Files\X\x.exe`.
#[cfg_attr(not(windows), allow(dead_code))]
fn executable_of(command: &str) -> String {
    let c = command.trim();
    if let Some(rest) = c.strip_prefix('"') {
        rest.split('"').next().unwrap_or(rest).to_string()
    } else {
        c.split_whitespace().next().unwrap_or(c).to_string()
    }
}

/// Some browsers register their name as a resource reference (`@path,-123`);
/// shown as is, it would read as garbage. Fall back to the file name.
#[cfg_attr(not(windows), allow(dead_code))]
fn resolve_indirect(name: String) -> String {
    if let Some(reference) = name.strip_prefix('@') {
        let file = reference.split(',').next().unwrap_or(reference);
        let stem = file.rsplit(['\\', '/']).next().unwrap_or(file).trim_end_matches(".exe");
        return stem.to_string();
    }
    name
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engines() {
        assert_eq!(engine_of(r"C:\Program Files\Mozilla Firefox\firefox.exe"), Engine::Gecko);
        assert_eq!(engine_of(r"C:\Program Files\Firefox Nightly\firefox.exe"), Engine::Gecko);
        assert_eq!(engine_of(r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe"), Engine::Chromium);
        assert_eq!(engine_of("/Applications/Safari.app/Contents/MacOS/Safari"), Engine::Webkit);
        assert_eq!(engine_of("unknown.exe"), Engine::Unknown);
        // What pages are rendered with, which apps speak of.
        let opera = r"C:\Users\u\AppData\Local\Programs\Opera\launcher.exe";
        assert_eq!(engine_of(opera), Engine::Unknown);
        assert_eq!(web_engine_of(&engine_of(opera), opera), Engine::Chromium);
        assert_eq!(web_engine_of(&Engine::Gecko, "firefox.exe"), Engine::Gecko);
        let window = if cfg!(windows) { Engine::Chromium } else { Engine::Webkit };
        assert_eq!(web_engine_of(&Engine::Embedded, ""), window);
        assert_eq!(engine_of("/usr/bin/google-chrome-stable"), Engine::Chromium);
        assert_eq!(engine_of("/usr/bin/firefox-esr"), Engine::Gecko);
        assert_eq!(engine_of("microsoft-edge-beta"), Engine::Chromium);
        assert_eq!(engine_of("epiphany"), Engine::Webkit);
    }

    #[test]
    fn chromium_data_dirs() {
        let local = std::path::Path::new("L");
        let dir = |exe: &str| chromium_user_data(exe, local).map(|p| p.to_string_lossy().replace('\\', "/"));
        assert_eq!(dir(r"C:\Program Files\Google\Chrome\Application\chrome.exe").as_deref(), Some("L/Google/Chrome/User Data"));
        assert_eq!(
            dir(r"C:\Users\u\AppData\Local\Google\Chrome SxS\Application\chrome.exe").as_deref(),
            Some("L/Google/Chrome SxS/User Data")
        );
        assert_eq!(dir(r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe").as_deref(), Some("L/Microsoft/Edge/User Data"));
        assert_eq!(dir(r"C:\opera\launcher.exe"), None);
    }

    #[test]
    fn local_state() {
        let state: serde_json::Value = serde_json::from_str(
            r#"{"profile":{"last_used":"Profile 7","info_cache":{"Profile 6":{"name":"Profil 1"},"Profile 7":{"name":"Profil 2"}}}}"#,
        )
        .unwrap();
        let p = parse_local_state(&state);
        assert_eq!(p.len(), 2);
        assert!(p.iter().find(|x| x.id == "Profile 7").unwrap().default);
    }

    #[test]
    fn firefox_profiles() {
        let profiles = "[Profile1]\nName=default-nightly\nIsRelative=1\nPath=Profiles/j3.default-nightly\n[Profile0]\nName=default\nPath=Profiles/15.default\nDefault=1\n";
        let installs = "[308046B0AF4A39CB]\nDefault=Profiles/15.default\n[6F193CCC56814779]\nDefault=Profiles/j3.default-nightly\n";
        let release = parse_profiles_ini(profiles, installs, "Firefox-308046B0AF4A39CB");
        let nightly = parse_profiles_ini(profiles, installs, "Firefox-6F193CCC56814779");
        assert_eq!(release.iter().find(|p| p.default).map(|p| p.name.as_str()), Some("default"));
        assert_eq!(nightly.iter().find(|p| p.default).map(|p| p.name.as_str()), Some("default-nightly"));
        assert_eq!(release.len(), 2);
    }

    #[test]
    fn executables() {
        assert_eq!(executable_of(r#""C:\Program Files\X\x.exe" --flag"#), r"C:\Program Files\X\x.exe");
        assert_eq!(executable_of(r"C:\x.exe --flag"), r"C:\x.exe");
        assert_eq!(resolve_indirect(r"@C:\Program Files\X\x.exe,-100".into()), "x");
    }

    #[test]
    fn recommended_at_first_meeting() {
        let b = |id: &str, name: &str, engine: Engine, web: Engine| Browser {
            id: id.into(),
            name: name.into(),
            exe: String::new(),
            engine,
            web_engine: web,
            profiles: Vec::new(),
            command: Vec::new(),
        };
        let firefox = b("firefox.desktop", "Firefox", Engine::Gecko, Engine::Gecko);
        let nightly = b("nightly", "Firefox Nightly", Engine::Gecko, Engine::Gecko);
        let opera = b("opera", "Opera", Engine::Unknown, Engine::Chromium);
        let brave = b("brave", "Brave", Engine::Chromium, Engine::Chromium);
        let chrome = b("chrome", "Google Chrome", Engine::Chromium, Engine::Chromium);
        let installed = vec![opera.clone(), brave.clone(), chrome.clone(), nightly.clone(), firefox.clone()];
        let rec = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        // Following a browser it recommends: left as it is.
        assert_eq!(recommended_for(&rec(&["chromium", "gecko"]), Some(&firefox), &installed), None);
        // Chromium recommended, Firefox followed: the usual Chromium, not Opera nor Brave.
        assert_eq!(recommended_for(&rec(&["chromium"]), Some(&firefox), &installed).as_deref(), Some("chrome"));
        assert_eq!(recommended_for(&rec(&["chromium"]), None, &[opera.clone(), brave.clone()]).as_deref(), Some("brave"));
        // Gecko: the stable Firefox before Nightly.
        assert_eq!(recommended_for(&rec(&["gecko"]), Some(&chrome), &installed).as_deref(), Some("firefox.desktop"));
        // The Kynoko window, by name; nothing recommended installed: left alone.
        assert_eq!(recommended_for(&rec(&["kynoko"]), Some(&chrome), &installed).as_deref(), Some(EMBEDDED));
        assert_eq!(recommended_for(&rec(&["chromium"]), Some(&firefox), &[firefox.clone()]), None);
        assert_eq!(recommended_for(&[], Some(&firefox), &installed), None);
    }
}
