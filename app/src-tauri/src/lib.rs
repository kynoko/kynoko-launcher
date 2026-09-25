//! Kynoko Launcher (docs/SPEC.md).
//!
//! One binary, several roles:
//! - no argument: the settings window;
//! - `open <file>...`: what a double-click runs; opens each file in the app
//!   that reads it, through the loopback bridge;
//! - `launch <app>[/<facade>]`: what a shortcut runs;
//! - `cleanup`: removes everything the launcher wrote (uninstaller, and the
//!   window's "Remove everything").
//!
//! A second invocation hands its arguments to the running one (single
//! instance), so one AGENT owns the bridge. The agent lives while files are
//! open and quits by itself once none is and no window is shown.

mod assoc;
mod bridge;
mod browsers;
mod catalogue;
mod launch;
mod settings;

use std::path::PathBuf;
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Manager};

use bridge::Bridge;
use catalogue::Catalogue;
use settings::{Inventory, Settings};

/// How long a freshly opened session waits for its page before the agent
/// may quit (the browser can take a while to start).
const FIRST_CONTACT: Duration = Duration::from_secs(120);

enum Command {
    Window,
    Open(Vec<PathBuf>),
    Launch(String),
    Cleanup,
}

fn parse(args: &[String]) -> Command {
    match args.first().map(String::as_str) {
        Some("open") => Command::Open(args[1..].iter().map(PathBuf::from).collect()),
        Some("launch") => Command::Launch(args.get(1).cloned().unwrap_or_default()),
        Some("cleanup") => Command::Cleanup,
        _ => Command::Window,
    }
}

struct Shared {
    catalogue: Catalogue,
    bridge: Mutex<Option<Bridge>>,
    last_open: Mutex<Option<Instant>>,
}

impl Shared {
    fn bridge(&self) -> Result<Bridge, String> {
        let mut slot = self.bridge.lock().expect("bridge lock");
        if slot.is_none() {
            *slot = Some(Bridge::start().map_err(|e| e.to_string())?);
        }
        Ok(slot.clone().expect("started"))
    }
}

/// The address an app is opened at: its own, or the one settings point it to.
fn app_url(settings: &Settings, app: &catalogue::App) -> String {
    settings.app_urls.get(&app.code).cloned().unwrap_or_else(|| app.url.clone())
}

fn origin_of(url: &str) -> String {
    // scheme://host[:port], without path.
    let after_scheme = url.find("://").map(|i| i + 3).unwrap_or(0);
    let end = url[after_scheme..].find('/').map(|i| i + after_scheme).unwrap_or(url.len());
    url[..end].to_string()
}

fn browser_by_id(id: Option<String>) -> Option<browsers::Browser> {
    let id = id?;
    browsers::installed().into_iter().find(|b| b.id == id)
}

/// `open <file>`: one bridge session per file, one launch per app.
fn open_files(shared: &Shared, files: &[PathBuf]) -> Result<(), String> {
    let settings = Settings::load();
    let bridge = shared.bridge()?;
    for path in files {
        let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
        let app = shared
            .catalogue
            .app_for_ext(&ext, &settings.associated_apps)
            .or_else(|| shared.catalogue.app_for_ext(&ext, &shared.catalogue.apps.iter().map(|a| a.code.clone()).collect::<Vec<_>>()))
            .ok_or_else(|| format!("no Kynoko app opens .{ext}"))?;
        let base = app_url(&settings, app);
        let token = bridge.open(path.clone(), origin_of(&base));
        let url = format!("{}/open#kynokoBridge=127.0.0.1:{}/{}", base.trim_end_matches('/'), bridge.port, token);
        launch::open(&url, browser_by_id(settings.browser_for(&app.code)).as_ref()).map_err(|e| e.to_string())?;
    }
    *shared.last_open.lock().expect("lock") = Some(Instant::now());
    Ok(())
}

/// `launch <app>[/<facade>]`: the app (or one of its facades) in its browser.
fn launch_app(shared: &Shared, target: &str) -> Result<(), String> {
    let (code, facade) = target.split_once('/').unwrap_or((target, ""));
    let app = shared.catalogue.app(code).ok_or_else(|| format!("unknown app {code}"))?;
    let settings = Settings::load();
    let url = format!("{}/{}", app_url(&settings, app).trim_end_matches('/'), facade);
    launch::open(&url, browser_by_id(settings.browser_for(code)).as_ref()).map_err(|e| e.to_string())
}

fn cleanup() -> Result<(), String> {
    let mut inventory = Inventory::load();
    assoc::remove_all(&mut inventory).map_err(|e| e.to_string())?;
    settings::remove_own_state();
    Ok(())
}

fn dispatch(app: &AppHandle, command: Command) {
    let shared = app.state::<Shared>();
    let result = match command {
        Command::Window => {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.show();
                let _ = w.set_focus();
            }
            Ok(())
        }
        Command::Open(files) => open_files(&shared, &files),
        Command::Launch(target) => launch_app(&shared, &target),
        Command::Cleanup => {
            let r = cleanup();
            app.exit(if r.is_ok() { 0 } else { 1 });
            r
        }
    };
    if let Err(e) = result {
        eprintln!("kynoko-launcher: {e}");
    }
}

/// Quits the agent once nothing needs it: no visible window, no live session,
/// and no session still waiting for its page to show up.
fn watch_idle(app: AppHandle) {
    thread::spawn(move || loop {
        thread::sleep(Duration::from_secs(5));
        let shared = app.state::<Shared>();
        let window_shown = app.get_webview_window("main").and_then(|w| w.is_visible().ok()).unwrap_or(false);
        let live = shared.bridge.lock().expect("bridge lock").as_ref().map(|b| b.live()).unwrap_or(0);
        let waiting = shared.last_open.lock().expect("lock").map(|t| t.elapsed() < FIRST_CONTACT).unwrap_or(false);
        if !window_shown && live == 0 && !waiting {
            app.exit(0);
            return;
        }
    });
}

/* ------------------------------------------------------------ window API */

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AppView {
    code: String,
    name: String,
    extensions: Vec<String>,
    associated: bool,
    browser: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StateView {
    apps: Vec<AppView>,
    browsers: Vec<browsers::Browser>,
    default_browser: Option<String>,
    catalogue_date: String,
    windows: bool,
}

#[tauri::command]
fn get_state(shared: tauri::State<'_, Shared>, lang: String) -> StateView {
    let settings = Settings::load();
    StateView {
        apps: shared
            .catalogue
            .apps
            .iter()
            .map(|a| AppView {
                code: a.code.clone(),
                name: a.name(&lang),
                extensions: a.extensions(),
                associated: settings.associated_apps.contains(&a.code),
                browser: settings.app_browsers.get(&a.code).cloned(),
            })
            .collect(),
        browsers: browsers::installed(),
        default_browser: settings.default_browser.clone(),
        catalogue_date: shared.catalogue.generated_at.clone(),
        windows: cfg!(windows),
    }
}

#[tauri::command]
fn set_default_browser(id: Option<String>) -> Result<(), String> {
    let mut settings = Settings::load();
    settings.default_browser = id;
    settings.save().map_err(|e| e.to_string())
}

#[tauri::command]
fn set_app_browser(code: String, id: Option<String>) -> Result<(), String> {
    let mut settings = Settings::load();
    match id {
        Some(id) => settings.app_browsers.insert(code, id),
        None => settings.app_browsers.remove(&code),
    };
    settings.save().map_err(|e| e.to_string())
}

#[tauri::command]
fn set_associated(shared: tauri::State<'_, Shared>, code: String, on: bool) -> Result<(), String> {
    let app = shared.catalogue.app(&code).ok_or("unknown app")?;
    let mut settings = Settings::load();
    let mut inventory = Inventory::load();
    if on {
        assoc::register(app, &mut inventory).map_err(|e| e.to_string())?;
        if !settings.associated_apps.contains(&code) {
            settings.associated_apps.push(code);
        }
    } else {
        assoc::unregister(app, &mut inventory).map_err(|e| e.to_string())?;
        settings.associated_apps.retain(|c| c != &code);
    }
    settings.save().map_err(|e| e.to_string())
}

#[tauri::command]
fn launch(shared: tauri::State<'_, Shared>, target: String) -> Result<(), String> {
    launch_app(&shared, &target)
}

#[tauri::command]
fn remove_everything() -> Result<(), String> {
    cleanup()
}

/// Windows: the Default apps page, on the launcher's own entry.
#[tauri::command]
fn open_default_apps() -> Result<(), String> {
    #[cfg(windows)]
    {
        let url = format!("ms-settings:defaultapps?registeredAppUser={}", assoc::APPLICATION_NAME.replace(' ', "%20"));
        return launch::open(&url, None).map_err(|e| e.to_string());
    }
    #[allow(unreachable_code)]
    Err("not available on this system".into())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let first = parse(&args);
    let show_window = matches!(first, Command::Window);

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            dispatch(app, parse(&argv[1..]));
        }))
        .manage(Shared {
            catalogue: Catalogue::bundled(),
            bridge: Mutex::new(None),
            last_open: Mutex::new(None),
        })
        .invoke_handler(tauri::generate_handler![
            get_state,
            set_default_browser,
            set_app_browser,
            set_associated,
            launch,
            remove_everything,
            open_default_apps
        ])
        .setup(move |app| {
            let handle = app.handle().clone();
            dispatch(&handle, first);
            if !show_window {
                watch_idle(handle);
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            // Closing the window hides it: an open file may still need the
            // bridge. The idle watch quits once nothing does.
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
                watch_idle(window.app_handle().clone());
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running Kynoko Launcher");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origins() {
        assert_eq!(origin_of("https://office.kynoko.com/"), "https://office.kynoko.com");
        assert_eq!(origin_of("https://a.example:8443/x/y"), "https://a.example:8443");
        assert_eq!(origin_of("https://a.example"), "https://a.example");
    }

    #[test]
    fn routing() {
        let c = Catalogue::bundled();
        let all: Vec<String> = c.apps.iter().map(|a| a.code.clone()).collect();
        assert_eq!(c.app_for_ext("docx", &all).map(|a| a.code.as_str()), Some("Office"));
        assert_eq!(c.app_for_ext("JPG", &all).map(|a| a.code.as_str()), Some("PhotoStudio"));
        assert_eq!(c.app_for_ext("mkv", &all).map(|a| a.code.as_str()), Some("MediaStudio"));
        assert!(c.app_for_ext("exe", &all).is_none());
    }
}
