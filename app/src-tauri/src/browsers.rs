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
}

#[derive(Debug, Clone, Serialize)]
pub struct Browser {
    /// Stable id: the system's own key for it (registry key name on Windows).
    pub id: String,
    pub name: String,
    pub exe: String,
    pub engine: Engine,
}

/// The engine, from the executable's name: it decides how a window is opened.
pub fn engine_of(exe: &str) -> Engine {
    let file = exe.rsplit(['\\', '/']).next().unwrap_or(exe).to_ascii_lowercase();
    let stem = file.trim_end_matches(".exe");
    match stem {
        "chrome" | "msedge" | "brave" | "opera" | "launcher" | "vivaldi" | "chromium" | "thorium" | "yandex" => Engine::Chromium,
        "firefox" | "librewolf" | "waterfox" | "zen" | "floorp" | "mullvadbrowser" => Engine::Gecko,
        "safari" => Engine::Webkit,
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
            out.push(Browser { engine: engine_of(&exe), id, name: resolve_indirect(name), exe });
        }
    }
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out
}

#[cfg(not(windows))]
pub fn installed() -> Vec<Browser> {
    // macOS (LaunchServices) and Linux (.desktop files): next milestone.
    Vec::new()
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
    }

    #[test]
    fn executables() {
        assert_eq!(executable_of(r#""C:\Program Files\X\x.exe" --flag"#), r"C:\Program Files\X\x.exe");
        assert_eq!(executable_of(r"C:\x.exe --flag"), r"C:\x.exe");
        assert_eq!(resolve_indirect(r"@C:\Program Files\X\x.exe,-100".into()), "x");
    }
}
