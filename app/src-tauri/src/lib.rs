//! Kynoko Launcher (docs/SPEC.md).
//!
//! One binary, several roles:
//! - no argument, or `kynoko-launcher://settings?app=<code>` (the apps'
//!   menu entry): the settings window, on that app;
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
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
mod settings;
mod shortcuts;
mod xdg;

use std::path::PathBuf;
use std::sync::{Mutex, RwLock};
use std::thread;
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use bridge::Bridge;
use catalogue::{Cache, Catalogue, Refresh};
use settings::{Inventory, Settings};

/// How long a freshly opened session waits for its page before the agent
/// may quit (the browser can take a while to start).
const FIRST_CONTACT: Duration = Duration::from_secs(120);

/// Added to every app address the launcher opens: the app then knows the
/// launcher is installed for this browser, and its menu entry opens it
/// directly instead of offering the download.
const MARKER: &str = "kynokoLauncher=1";

enum Command {
    /// The settings window, optionally on one app.
    Window(Option<String>),
    Open(Vec<PathBuf>),
    Launch(String),
    Cleanup,
}

fn parse(args: &[String]) -> Command {
    match args.first().map(String::as_str) {
        Some("open") => Command::Open(args[1..].iter().map(PathBuf::from).collect()),
        Some("launch") => Command::Launch(args.get(1).cloned().unwrap_or_default()),
        Some("cleanup") => Command::Cleanup,
        Some(url) if url.starts_with(&format!("{}:", assoc::SCHEME)) => Command::Window(app_param(url)),
        _ => Command::Window(None),
    }
}

/// `kynoko-launcher://settings?app=Office` -> Some("Office"). Anything that
/// is not a plain app code is ignored: a link must not steer the launcher.
fn app_param(url: &str) -> Option<String> {
    let query = url.split_once('?')?.1;
    query
        .split('&')
        .find_map(|pair| pair.strip_prefix("app="))
        .filter(|code| !code.is_empty() && code.len() <= 40 && code.chars().all(|c| c.is_ascii_alphanumeric()))
        .map(str::to_string)
}

struct Shared {
    /// The catalogue in use: the last good online copy, else the bundled one.
    catalogue: RwLock<Catalogue>,
    /// Last manual "check now" (checks are limited to one a minute).
    last_manual_check: Mutex<Option<Instant>>,
    bridge: Mutex<Option<Bridge>>,
    last_open: Mutex<Option<Instant>>,
    /// The app the window should put forward (from a `settings?app=` link).
    focus: Mutex<Option<String>>,
    /// macOS: a file or link arrived as an Apple Event (the window, asked for
    /// by the bare start that precedes it, is then not shown).
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    opened_by_event: std::sync::atomic::AtomicBool,
}

impl Shared {
    fn catalogue(&self) -> Catalogue {
        self.catalogue.read().expect("catalogue lock").clone()
    }

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
    settings.url_of(app)
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
        let catalogue = shared.catalogue();
        let every: Vec<String> = catalogue.apps.iter().map(|a| a.code.clone()).collect();
        let app = catalogue
            .app_for_ext(&ext, &settings.associated_apps)
            .or_else(|| catalogue.app_for_ext(&ext, &every))
            .ok_or_else(|| format!("no Kynoko app opens .{ext}"))?;
        let base = app_url(&settings, app);
        let token = bridge.open(path.clone(), origin_of(&base));
        let url = format!("{}/open#kynokoBridge=127.0.0.1:{}/{}&{MARKER}", base.trim_end_matches('/'), bridge.port, token);
        let profile = settings.profile_for(&app.code);
        launch::open(&url, browser_by_id(settings.browser_for(&app.code)).as_ref(), profile.as_deref())
            .map_err(|e| e.to_string())?;
    }
    *shared.last_open.lock().expect("lock") = Some(Instant::now());
    Ok(())
}

/// `launch <app>[/<facade>]`: the app (or one of its facades) in its browser.
fn launch_app(shared: &Shared, target: &str) -> Result<(), String> {
    let (code, facade) = target.split_once('/').unwrap_or((target, ""));
    let catalogue = shared.catalogue();
    let app = catalogue.app(code).ok_or_else(|| format!("unknown app {code}"))?;
    let settings = Settings::load();
    let url = format!("{}/{}#{MARKER}", app_url(&settings, app).trim_end_matches('/'), facade);
    let profile = settings.profile_for(code);
    launch::open(&url, browser_by_id(settings.browser_for(code)).as_ref(), profile.as_deref()).map_err(|e| e.to_string())
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
        Command::Window(focus) => {
            *shared.focus.lock().expect("lock") = focus.clone();
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.show();
                let _ = w.unminimize();
                let _ = w.set_focus();
            }
            // An open window re-reads its state and puts the app forward.
            let _ = app.emit("focus-app", focus);
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
    shortcuts: bool,
    browser: Option<String>,
    profile: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StateView {
    apps: Vec<AppView>,
    browsers: Vec<browsers::Browser>,
    default_browser: Option<String>,
    default_profile: Option<String>,
    catalogue_date: String,
    /// Last successful check of the online catalogue (Unix seconds), if any.
    catalogue_checked: Option<u64>,
    /// Why the last check failed, while it keeps failing.
    catalogue_error: Option<String>,
    windows: bool,
    /// The app to put forward, once.
    focus: Option<String>,
}

#[tauri::command]
fn get_state(shared: tauri::State<'_, Shared>, lang: String) -> StateView {
    let mut settings = Settings::load();
    // The window's language: what shortcuts are named in when the catalogue
    // later renames them in the background.
    if settings.ui_lang.as_deref() != Some(lang.as_str()) {
        settings.ui_lang = Some(lang.clone());
        let _ = settings.save();
    }
    let catalogue = shared.catalogue();
    let cache = Cache::load();
    StateView {
        apps: catalogue
            .apps
            .iter()
            .map(|a| AppView {
                code: a.code.clone(),
                name: a.name(&lang),
                extensions: a.extensions(),
                associated: settings.associated_apps.contains(&a.code),
                shortcuts: settings.shortcut_apps.contains(&a.code),
                browser: settings.app_browsers.get(&a.code).cloned(),
                profile: settings.app_profiles.get(&a.code).cloned(),
            })
            .collect(),
        browsers: browsers::installed(),
        default_browser: settings.default_browser.clone(),
        default_profile: settings.default_profile.clone(),
        catalogue_date: catalogue.generated_at.clone(),
        catalogue_checked: cache.success_at,
        catalogue_error: cache.last_error.clone(),
        windows: cfg!(windows),
        focus: shared.focus.lock().expect("lock").take(),
    }
}

#[tauri::command]
fn set_default_browser(id: Option<String>) -> Result<(), String> {
    let mut settings = Settings::load();
    // A profile belongs to one browser: another browser starts on its own default.
    settings.default_profile = None;
    settings.default_browser = id;
    settings.save().map_err(|e| e.to_string())
}

#[tauri::command]
fn set_default_profile(id: Option<String>) -> Result<(), String> {
    let mut settings = Settings::load();
    settings.default_profile = id;
    settings.save().map_err(|e| e.to_string())
}

#[tauri::command]
fn set_app_browser(code: String, id: Option<String>) -> Result<(), String> {
    let mut settings = Settings::load();
    settings.app_profiles.remove(&code);
    match id {
        Some(id) => settings.app_browsers.insert(code, id),
        None => settings.app_browsers.remove(&code),
    };
    settings.save().map_err(|e| e.to_string())
}

#[tauri::command]
fn set_app_profile(code: String, id: Option<String>) -> Result<(), String> {
    let mut settings = Settings::load();
    match id {
        Some(id) => settings.app_profiles.insert(code, id),
        None => settings.app_profiles.remove(&code),
    };
    settings.save().map_err(|e| e.to_string())
}

#[tauri::command]
fn set_associated(shared: tauri::State<'_, Shared>, code: String, on: bool) -> Result<(), String> {
    let catalogue = shared.catalogue();
    let app = catalogue.app(&code).ok_or("unknown app")?;
    let mut settings = Settings::load();
    let mut inventory = Inventory::load();
    if on {
        assoc::register(app, &settings, &mut inventory).map_err(|e| e.to_string())?;
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
fn set_shortcuts(shared: tauri::State<'_, Shared>, code: String, on: bool, lang: String) -> Result<(), String> {
    let catalogue = shared.catalogue();
    let app = catalogue.app(&code).ok_or("unknown app")?;
    let mut settings = Settings::load();
    let mut inventory = Inventory::load();
    // Rebuilt from scratch either way: names and icons follow the catalogue.
    shortcuts::remove(app, &lang, &mut inventory).map_err(|e| e.to_string())?;
    settings.shortcut_apps.retain(|c| c != &code);
    if on {
        shortcuts::create(app, &settings, &lang, &mut inventory).map_err(|e| e.to_string())?;
        settings.shortcut_apps.push(code);
    }
    settings.save().map_err(|e| e.to_string())
}

/// "Check now": at most once a minute, whatever the button is clicked.
#[tauri::command]
fn check_catalogue(app: AppHandle, shared: tauri::State<'_, Shared>) -> Result<(), String> {
    {
        let mut last = shared.last_manual_check.lock().expect("lock");
        if last.map(|t| t.elapsed() < Duration::from_secs(60)).unwrap_or(false) {
            return Ok(());
        }
        *last = Some(Instant::now());
    }
    check_online(&app);
    Ok(())
}

/// One conditional request for the online catalogue; a changed catalogue is
/// applied to what the user set up (see reconcile) before it replaces the
/// one in use.
fn check_online(app: &AppHandle) {
    let shared = app.state::<Shared>();
    let settings = Settings::load();
    let url = settings.catalogue_url.clone().unwrap_or_else(|| catalogue::DEFAULT_URL.to_string());
    let mut cache = Cache::load();
    match catalogue::refresh(&url, &mut cache) {
        Refresh::Updated(fresh) => {
            let old = shared.catalogue();
            reconcile(&old, &fresh);
            *shared.catalogue.write().expect("catalogue lock") = fresh;
            let _ = app.emit("catalogue-updated", ());
        }
        Refresh::Unchanged => {}
        // Kept in the cache and shown quietly in the window; never a notification.
        Refresh::Failed(e) => eprintln!("kynoko-launcher: catalogue not refreshed: {e}"),
    }
}

/// What a new catalogue changes in what the user set up (docs/SPEC.md,
/// section 4): an app that left loses its associations and shortcuts; a
/// changed app has them rebuilt from its new definition. Nothing is ever
/// added the user did not ask for: only apps already chosen are touched.
fn reconcile(old: &Catalogue, new: &Catalogue) {
    let mut settings = Settings::load();
    let mut inventory = Inventory::load();
    let lang = settings.ui_lang.clone().unwrap_or_else(|| "en".to_string());
    for code in settings.associated_apps.clone() {
        let (Some(before), after) = (old.app(&code), new.app(&code)) else { continue };
        if after.map(|a| a.extensions()) == Some(before.extensions()) {
            continue;
        }
        let _ = assoc::unregister(before, &mut inventory);
        match after {
            Some(a) => {
                let _ = assoc::register(a, &settings, &mut inventory);
            }
            None => settings.associated_apps.retain(|c| c != &code),
        }
    }
    for code in settings.shortcut_apps.clone() {
        let (Some(before), after) = (old.app(&code), new.app(&code)) else { continue };
        if after == Some(before) {
            continue;
        }
        let _ = shortcuts::remove(before, &lang, &mut inventory);
        match after {
            Some(a) => {
                let _ = shortcuts::create(a, &settings, &lang, &mut inventory);
            }
            None => settings.shortcut_apps.retain(|c| c != &code),
        }
    }
    let _ = settings.save();
}

/// The online catalogue, in the background: when due (12 h after the last
/// success, sooner after a failure), after a random delay so that machines
/// started together do not all ask together.
fn watch_catalogue(app: AppHandle) {
    thread::spawn(move || {
        let mut jitter = [0u8; 2];
        let _ = getrandom::getrandom(&mut jitter);
        // A first run has nothing but the bundled copy: ask soon.
        let first_delay = if Cache::load().success_at.is_none() { 5 } else { 30 + u16::from_le_bytes(jitter) as u64 % 570 };
        thread::sleep(Duration::from_secs(first_delay));
        loop {
            if catalogue::now() >= Cache::load().due_at() {
                check_online(&app);
            }
            thread::sleep(Duration::from_secs(600));
        }
    });
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
        return launch::open(&url, None, None).map_err(|e| e.to_string());
    }
    #[allow(unreachable_code)]
    Err("not available on this system".into())
}

/// macOS: documents and kynoko-launcher:// links, handed over as Apple Events.
#[cfg(target_os = "macos")]
fn opened(app: &AppHandle, urls: &[tauri::Url]) {
    let shared = app.state::<Shared>();
    shared.opened_by_event.store(true, std::sync::atomic::Ordering::SeqCst);
    let files: Vec<PathBuf> = urls.iter().filter(|u| u.scheme() == "file").filter_map(|u| u.to_file_path().ok()).collect();
    if !files.is_empty() {
        dispatch(app, Command::Open(files));
    }
    for link in urls.iter().filter(|u| u.scheme() == assoc::SCHEME) {
        dispatch(app, Command::Window(app_param(link.as_str())));
    }
    let shown = app.get_webview_window("main").and_then(|w| w.is_visible().ok()).unwrap_or(false);
    if !shown {
        watch_idle(app.clone());
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let first = parse(&args);
    let show_window = matches!(first, Command::Window(_));
    let cleaning = matches!(first, Command::Cleanup);

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            dispatch(app, parse(&argv[1..]));
        }))
        .manage(Shared {
            catalogue: RwLock::new(Cache::load().catalogue()),
            last_manual_check: Mutex::new(None),
            bridge: Mutex::new(None),
            last_open: Mutex::new(None),
            focus: Mutex::new(None),
            opened_by_event: std::sync::atomic::AtomicBool::new(false),
        })
        .invoke_handler(tauri::generate_handler![
            get_state,
            set_default_browser,
            set_default_profile,
            set_app_browser,
            set_app_profile,
            set_associated,
            set_shortcuts,
            check_catalogue,
            launch,
            remove_everything,
            open_default_apps
        ])
        .setup(move |app| {
            let handle = app.handle().clone();
            if !cleaning {
                if let Err(e) = assoc::register_scheme(&mut Inventory::load()) {
                    eprintln!("kynoko-launcher: cannot register {}://: {e}", assoc::SCHEME);
                }
            }
            if !cleaning {
                watch_catalogue(handle.clone());
            }
            // macOS starts the launcher WITHOUT arguments for a double-clicked
            // file, then hands the file over as an Apple Event: wait a moment
            // before showing the settings window, which that start did not mean.
            #[cfg(target_os = "macos")]
            if matches!(first, Command::Window(None)) {
                let later = handle.clone();
                thread::spawn(move || {
                    thread::sleep(Duration::from_millis(800));
                    let shared = later.state::<Shared>();
                    if !shared.opened_by_event.load(std::sync::atomic::Ordering::SeqCst) {
                        dispatch(&later, Command::Window(None));
                    }
                });
                return Ok(());
            }
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
        .build(tauri::generate_context!())
        .expect("error while building Kynoko Launcher")
        .run(|_handle, _event| {
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Opened { urls } = &_event {
                opened(_handle, urls);
            }
        });
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
    fn deep_links() {
        assert_eq!(app_param("kynoko-launcher://settings?app=Office"), Some("Office".into()));
        assert_eq!(app_param("kynoko-launcher://settings?x=1&app=PhotoStudio"), Some("PhotoStudio".into()));
        assert_eq!(app_param("kynoko-launcher://settings?app=../../evil"), None);
        assert_eq!(app_param("kynoko-launcher://settings"), None);
        assert!(matches!(parse(&["kynoko-launcher://settings?app=Office".into()]), Command::Window(Some(_))));
    }

    #[test]
    fn routing() {
        let c = Catalogue::bundled();
        assert!(c.apps.iter().all(|a| a.facades.iter().all(|f| f.listed)), "bundled facades default to listed");
        let all: Vec<String> = c.apps.iter().map(|a| a.code.clone()).collect();
        assert_eq!(c.app_for_ext("docx", &all).map(|a| a.code.as_str()), Some("Office"));
        assert_eq!(c.app_for_ext("JPG", &all).map(|a| a.code.as_str()), Some("PhotoStudio"));
        assert_eq!(c.app_for_ext("mkv", &all).map(|a| a.code.as_str()), Some("MediaStudio"));
        assert!(c.app_for_ext("exe", &all).is_none());
    }
}
