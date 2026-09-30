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
mod fonts;
mod handoff;
mod launch;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod programs;
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
use handoff::Handoff;
use catalogue::{App, Cache, Catalogue, Refresh};
use settings::{Inventory, Settings};

/// How long a freshly opened session waits for its page before the agent
/// may quit (the browser can take a while to start).
const FIRST_CONTACT: Duration = Duration::from_secs(120);
/// After the last Kynoko window closes, the launcher stays this long: the web
/// engine writes cookies and storage to disk with a delay (about 30 s for
/// cookies), and quitting sooner loses a sign-in made just before.
const FLUSH_GRACE: Duration = Duration::from_secs(40);
/// A Kynoko window starts without the system's title bar: its page's own bar
/// takes its place (the skeleton asks own_window_is_maximized as it starts,
/// which is how it says so). A page that has not done it this long after
/// loading gets the system's title bar back: an older version of an app, the
/// sign-in page, a payment page, an error page; a window must always be
/// movable and closable.
const TITLE_BAR_GRACE: Duration = Duration::from_millis(2500);
/// How long the window uses an app's kept icon before asking whether it
/// changed (a conditional request: nothing is downloaded when it did not).
const ICON_RECHECK: Duration = Duration::from_secs(3600);

/// Added to every app address the launcher opens: the app then knows the
/// launcher is installed for this browser, and its menu entry opens it
/// directly instead of offering the download.
const MARKER: &str = "kynokoLauncher=1";

enum Command {
    /// The settings window, optionally on one app.
    Window(Option<String>),
    /// Files to open, and the app the system entry named (`open --app <code>`),
    /// when it named one.
    Open(Option<String>, Vec<PathBuf>),
    Launch(String),
    /// Takes away everything written to the system; `--keep-preferences`
    /// leaves the user's choices, which a reinstall then applies again.
    Cleanup { keep_preferences: bool },
}

fn parse(args: &[String]) -> Command {
    match args.first().map(String::as_str) {
        Some("open") => match args.get(1).map(String::as_str) {
            Some("--app") => Command::Open(args.get(2).filter(|c| plain_code(c)).cloned(), args.iter().skip(3).map(PathBuf::from).collect()),
            _ => Command::Open(None, args[1..].iter().map(PathBuf::from).collect()),
        },
        Some("launch") => Command::Launch(args.get(1).cloned().unwrap_or_default()),
        Some("cleanup") => Command::Cleanup { keep_preferences: args.get(1).is_some_and(|a| a == "--keep-preferences") },
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
        .filter(|code| plain_code(code))
        .map(str::to_string)
}

/// An app code as the catalogue writes them: a link or a command line must
/// not smuggle anything else in.
fn plain_code(code: &str) -> bool {
    !code.is_empty() && code.len() <= 40 && code.chars().all(|c| c.is_ascii_alphanumeric())
}

struct Shared {
    /// The catalogue in use: the last good online copy, else the bundled one.
    catalogue: RwLock<Catalogue>,
    /// Last manual "check now" (checks are limited to one a minute).
    last_manual_check: Mutex<Option<Instant>>,
    bridge: Mutex<Option<Bridge>>,
    handoff: Mutex<Option<Handoff>>,
    /// When a Kynoko window was last seen open or closed (see FLUSH_GRACE).
    last_app_window: Mutex<Option<Instant>>,
    /// Per Kynoko window: when its page last started loading, and when a
    /// page last took over the title bar (see TITLE_BAR_GRACE).
    page_loads: Mutex<std::collections::HashMap<String, Instant>>,
    title_bar_claims: Mutex<std::collections::HashMap<String, Instant>>,
    /// Set at start: what the launcher's own app windows are opened with.
    handle: std::sync::OnceLock<AppHandle>,
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
            *slot = Some(Bridge::start(remember_reached).map_err(|e| e.to_string())?);
        }
        Ok(slot.clone().expect("started"))
    }

    /// The hand-off listener for the gateway at `origin` (started once).
    fn handoff(&self, origin: &str) -> Result<Handoff, String> {
        let mut slot = self.handoff.lock().expect("hand-off lock");
        if slot.as_ref().is_some_and(|h| h.origin() != origin) {
            *slot = None;
        }
        if slot.is_none() {
            *slot = Some(Handoff::start(origin.to_string(), remember_reached).map_err(|e| e.to_string())?);
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
    if id == browsers::EMBEDDED {
        return Some(browsers::embedded());
    }
    browsers::installed().into_iter().find(|b| b.id == id)
}

/// `open [--app <code>] <file>`: one bridge session and one launch per file.
/// The app the system entry named wins when it opens that type; else the
/// associated app the user kept that type for; else any app that opens it.
fn open_files(shared: &Shared, named: Option<&str>, files: &[PathBuf]) -> Result<(), String> {
    let settings = Settings::load();
    let bridge = shared.bridge()?;
    for path in files {
        let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
        let catalogue = shared.catalogue();
        let every: Vec<String> = catalogue.apps.iter().map(|a| a.code.clone()).collect();
        let kept: Vec<String> = settings.associated_apps.iter().filter(|c| settings.ext_chosen(c, &ext)).cloned().collect();
        let named: Vec<String> = named.map(|c| vec![c.to_string()]).unwrap_or_default();
        let app = catalogue
            .app_for_ext(&ext, &named)
            .or_else(|| catalogue.app_for_ext(&ext, &kept))
            .or_else(|| catalogue.app_for_ext(&ext, &every))
            .ok_or_else(|| format!("no Kynoko app opens .{ext}"))?;
        let base = app_url(&settings, app);
        let origin = origin_of(&base);
        let (browser, profile) = browser_for(&settings, &app.code);
        let key = browser.as_ref().map(|b| reach_key(b, profile.as_deref(), &origin));
        let token = bridge.open(path.clone(), origin, key);
        let url = format!("{}/open#kynokoBridge=127.0.0.1:{}/{}&{MARKER}", base.trim_end_matches('/'), bridge.port, token);
        open_page(shared, &settings, browser, profile, &url, true)?;
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
    let (browser, profile) = browser_for(&settings, code);
    open_page(shared, &settings, browser, profile, &url, false)
}

/// The browser and profile `code` opens in: the one chosen for it, else the
/// default one, else the system's.
fn browser_for(settings: &Settings, code: &str) -> (Option<browsers::Browser>, Option<String>) {
    let browser = browser_by_id(settings.browser_for(code)).or_else(browsers::system_default);
    (browser, settings.profile_for(code))
}

/// See Settings::loopback_ok.
fn reach_key(b: &browsers::Browser, profile: Option<&str>, origin: &str) -> String {
    format!("{}|{}|{origin}", b.id, profile.unwrap_or("-"))
}

/// Proof arrived (from the bridge or the hand-off) that a browser profile
/// lets an origin reach the launcher: kept, so that Firefox's app window can
/// be used from now on.
fn remember_reached(key: String) {
    let mut settings = Settings::load();
    if !settings.loopback_ok.contains(&key) {
        settings.loopback_ok.push(key);
        let _ = settings.save();
    }
}

/// Opens an app page. Chromium: an app window (`--app`). Firefox on
/// Windows: a Taskbar Tab window through the gateway (handoff.rs), which is
/// how a facade and a file survive Firefox starting that window at the
/// site's root. But a Taskbar Tab has no address bar for Firefox's
/// permission prompt to hang from, so as long as this browser profile has
/// not been seen reaching the launcher (the gateway, and for a file the app
/// too), the gateway opens in an ordinary window, where the prompt is sure
/// to show; the next time is an app window.
fn open_page(
    shared: &Shared,
    settings: &Settings,
    browser: Option<browsers::Browser>,
    profile: Option<String>,
    url: &str,
    with_file: bool,
) -> Result<(), String> {
    if browser.as_ref().is_some_and(|b| b.engine == browsers::Engine::Embedded) {
        return open_embedded(shared, url);
    }
    let b = match browser {
        Some(b) if cfg!(windows) && b.engine == browsers::Engine::Gecko => b,
        other => return launch::open(url, other.as_ref(), profile.as_deref()).map_err(|e| e.to_string()),
    };
    let gateway = settings.gateway();
    let gateway_key = reach_key(&b, profile.as_deref(), &origin_of(&gateway));
    let app_key = reach_key(&b, profile.as_deref(), &origin_of(url));
    let handoff = shared.handoff(&origin_of(&gateway))?;
    handoff.push(url.to_string(), gateway_key.clone());
    // The launcher stays up until the gateway has asked.
    *shared.last_open.lock().expect("lock") = Some(Instant::now());
    let proven = settings.loopback_ok.contains(&gateway_key) && (!with_file || settings.loopback_ok.contains(&app_key));
    if proven {
        launch::open_taskbar_tab(&gateway, &b, profile.as_deref())
    } else {
        launch::open(&gateway, Some(&b), profile.as_deref())
    }
    .map_err(|e| e.to_string())
}

fn cleanup(keep_preferences: bool) -> Result<(), String> {
    let mut inventory = Inventory::load();
    assoc::remove_all(&mut inventory).map_err(|e| e.to_string())?;
    if keep_preferences {
        settings::remove_state_keeping_preferences();
    } else {
        settings::remove_own_state();
    }
    Ok(())
}

/* Kynoko windows' own title bar (the skeleton's KynokoNativeWindowService).
   Each command acts on the window the calling page is in, and can reach no
   other: the page names nothing. Only Kynoko windows are granted them. */

/// The page holds the title bar: noted, and the system's taken away if it
/// had been given back (see TITLE_BAR_GRACE).
fn claim_title_bar(window: &tauri::WebviewWindow) {
    window
        .state::<Shared>()
        .title_bar_claims
        .lock()
        .expect("lock")
        .insert(window.label().to_string(), Instant::now());
    #[cfg(not(target_os = "macos"))]
    if window.is_decorated().unwrap_or(false) {
        let _ = window.set_decorations(false);
    }
}

#[tauri::command]
fn own_window_drag(window: tauri::WebviewWindow) -> Result<(), String> {
    claim_title_bar(&window);
    window.start_dragging().map_err(|e| e.to_string())
}

#[tauri::command]
fn own_window_minimize(window: tauri::WebviewWindow) -> Result<(), String> {
    claim_title_bar(&window);
    window.minimize().map_err(|e| e.to_string())
}

#[tauri::command]
fn own_window_toggle_maximize(window: tauri::WebviewWindow) -> Result<(), String> {
    claim_title_bar(&window);
    if window.is_maximized().map_err(|e| e.to_string())? {
        window.unmaximize().map_err(|e| e.to_string())
    } else {
        window.maximize().map_err(|e| e.to_string())
    }
}

#[tauri::command]
fn own_window_is_maximized(window: tauri::WebviewWindow) -> Result<bool, String> {
    claim_title_bar(&window);
    window.is_maximized().map_err(|e| e.to_string())
}

#[tauri::command]
fn own_window_close(window: tauri::WebviewWindow) -> Result<(), String> {
    window.close().map_err(|e| e.to_string())
}

/// The font families of this computer, for a Kynoko window's page (an app's
/// font menus; see fonts.rs). Names only, never a font file. Read off the
/// window's thread, once per run.
#[tauri::command]
async fn system_fonts() -> Result<Vec<fonts::Family>, String> {
    tauri::async_runtime::spawn_blocking(|| fonts::families().to_vec())
        .await
        .map_err(|e| e.to_string())
}

/// Page loads of a Kynoko window: a page that has not taken the title bar
/// TITLE_BAR_GRACE after loading gets the system's back.
fn watch_title_bar(window: tauri::WebviewWindow, event: tauri::webview::PageLoadEvent) {
    let label = window.label().to_string();
    match event {
        tauri::webview::PageLoadEvent::Started => {
            window.state::<Shared>().page_loads.lock().expect("lock").insert(label, Instant::now());
        }
        tauri::webview::PageLoadEvent::Finished => {
            thread::spawn(move || {
                thread::sleep(TITLE_BAR_GRACE);
                let shared = window.state::<Shared>();
                let started = shared.page_loads.lock().expect("lock").get(&label).copied();
                let claimed = shared.title_bar_claims.lock().expect("lock").get(&label).copied();
                let held = match (started, claimed) {
                    (Some(s), Some(c)) => c >= s,
                    (None, Some(_)) => true,
                    _ => false,
                };
                if !held {
                    let _ = window.set_decorations(true);
                }
            });
        }
    }
}

/// An app page in the launcher's own window: an app window on every system
/// (the system's web engine: WebView2, WKWebView, WebKitGTK), whatever
/// browser is installed. The page is a remote site: these windows are given
/// no capability (capabilities/default.json names "main" only), so it can
/// never call the launcher. Its session is its own, kept in the launcher's
/// data folder (signed in once).
fn open_embedded(shared: &Shared, url: &str) -> Result<(), String> {
    use std::sync::atomic::{AtomicU32, Ordering};
    static NEXT: AtomicU32 = AtomicU32::new(1);
    let handle = shared.handle.get().ok_or("not started")?.clone();
    let target = url.parse::<tauri::Url>().map_err(|e| e.to_string())?;
    let label = format!("app-{}", NEXT.fetch_add(1, Ordering::SeqCst));
    // A window without an address bar stays on Kynoko (and its payment
    // pages, whose return must land in this same session): any other
    // address goes to the system's browser.
    thread::spawn(move || {
        let builder = tauri::WebviewWindowBuilder::new(&handle, &label, tauri::WebviewUrl::External(target));
        #[cfg(target_os = "macos")]
        let builder = builder.title_bar_style(tauri::TitleBarStyle::Overlay).hidden_title(true);
        let built = builder
            .title("Kynoko")
            .inner_size(1280.0, 840.0)
            // No system title bar: the app's own bar is the title bar (the
            // skeleton's KynokoNativeWindowService, allowed by the capability
            // "kynoko-window"). macOS keeps its window buttons, drawn over
            // the start of the app's bar.
            .decorations(cfg!(target_os = "macos"))
            // Files dropped from the system go to the PAGE (the apps take them
            // with the web's drag and drop), not to a launcher handler that
            // would swallow them before the page sees anything.
            .disable_drag_drop_handler()
            .on_page_load(|window, payload| watch_title_bar(window, payload.event()))
            .focused(true)
            // The window is named after its page ("Comptes | Kynoko Office"):
            // that is what the taskbar and Alt+Tab show.
            .on_document_title_changed(|window, title| {
                if !title.trim().is_empty() {
                    let _ = window.set_title(&title);
                }
            })
            .on_navigation(|u| {
                let within = |d: &str, h: &str| h == d || h.ends_with(&format!(".{d}"));
                let kynoko = u.scheme() == "https"
                    && u.host_str().is_some_and(|h| within("kynoko.com", h) || within("stripe.com", h));
                if !kynoko && u.scheme() != "about" {
                    let _ = launch::open(u.as_str(), None, None);
                }
                kynoko || u.scheme() == "about"
            })
            .build();
        match built {
            // In front, with the focus: it was opened for the user, now.
            Ok(window) => {
                let _ = window.unminimize();
                let _ = window.set_focus();
            }
            Err(e) => eprintln!("kynoko-launcher: cannot open an app window: {e}"),
        }
    });
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
        Command::Open(named, files) => open_files(&shared, named.as_deref(), &files),
        Command::Launch(target) => launch_app(&shared, &target),
        Command::Cleanup { keep_preferences } => {
            let r = cleanup(keep_preferences);
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
        let app_windows = app.webview_windows().keys().any(|l| l.starts_with("app-"));
        if app_windows {
            *shared.last_app_window.lock().expect("lock") = Some(Instant::now());
        }
        let flushing = shared.last_app_window.lock().expect("lock").is_some_and(|t| t.elapsed() < FLUSH_GRACE);
        let window_shown = app.get_webview_window("main").and_then(|w| w.is_visible().ok()).unwrap_or(false)
            || app_windows
            || flushing;
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
    /// The app's file types, by the facade each one opens in.
    types: Vec<TypeGroup>,
    associated: bool,
    shortcuts: bool,
    /// The app's menu entries (the app, then its listed facades), kept or not.
    shortcut_items: Vec<ShortcutView>,
    browser: Option<String>,
    profile: Option<String>,
    /// The engines the app recommends, and `kynoko` for the Kynoko window
    /// (see catalogue::Browsers).
    recommended: Vec<String>,
    /// What the user will miss per engine, in the window's language.
    limitations: Vec<LimitView>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LimitView {
    engines: Vec<String>,
    text: String,
    /// Not said of the Kynoko window (catalogue::Limitation::browsers_only).
    browsers_only: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StateView {
    apps: Vec<AppView>,
    browsers: Vec<browsers::Browser>,
    /// The browser "the system's" stands for (Windows: the https handler).
    system_browser: Option<String>,
    default_browser: Option<String>,
    default_profile: Option<String>,
    /// The launcher's own version.
    version: String,
    catalogue_date: String,
    /// Last successful check of the online catalogue (Unix seconds), if any.
    catalogue_checked: Option<u64>,
    /// Why the last check failed, while it keeps failing.
    catalogue_error: Option<String>,
    windows: bool,
    os: String,
    /// The app to put forward, once.
    focus: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ShortcutView {
    key: String,
    name: String,
    on: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TypeGroup {
    facade: String,
    exts: Vec<ExtView>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ExtView {
    ext: String,
    /// Kept by the user (acted on while the app is associated).
    on: bool,
}

/// `app`'s types grouped by the facade that opens them, in catalogue order.
fn type_groups(app: &catalogue::App, settings: &Settings, lang: &str) -> Vec<TypeGroup> {
    let mut groups: Vec<(String, TypeGroup)> = Vec::new();
    for ext in app.extensions() {
        let Some(facade) = app.facade_for(&ext) else { continue };
        let view = ExtView { on: settings.ext_chosen(&app.code, &ext), ext };
        match groups.iter_mut().find(|(path, _)| *path == facade.path) {
            Some((_, g)) => g.exts.push(view),
            None => groups.push((facade.path.clone(), TypeGroup { facade: facade.name(lang), exts: vec![view] })),
        }
    }
    groups.into_iter().map(|(_, g)| g).collect()
}

#[tauri::command]
fn get_state(app: AppHandle, shared: tauri::State<'_, Shared>, lang: String) -> StateView {
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
                types: type_groups(a, &settings, &lang),
                associated: settings.associated_apps.contains(&a.code),
                shortcuts: settings.shortcut_apps.contains(&a.code),
                shortcut_items: shortcuts::all_items(a, &settings, &lang)
                    .into_iter()
                    .map(|i| ShortcutView {
                        on: settings.shortcut_apps.contains(&a.code) && settings.shortcut_chosen(&a.code, &i.key),
                        key: i.key,
                        name: if cfg!(target_os = "macos") { i.short } else { i.name },
                    })
                    .collect(),
                browser: settings.app_browsers.get(&a.code).cloned(),
                profile: settings.app_profiles.get(&a.code).cloned(),
                recommended: a.browsers.recommended.clone(),
                limitations: a
                    .browsers
                    .limitations
                    .iter()
                    .filter_map(|l| {
                        l.text(&lang).map(|text| LimitView { engines: l.engines.clone(), text, browsers_only: l.browsers_only })
                    })
                    .collect(),
            })
            .collect(),
        browsers: std::iter::once(browsers::embedded()).chain(browsers::installed()).collect(),
        system_browser: browsers::system_default().map(|b| b.id),
        default_browser: settings.default_browser.clone(),
        default_profile: settings.default_profile.clone(),
        version: app.package_info().version.to_string(),
        catalogue_date: catalogue.generated_at.clone(),
        catalogue_checked: cache.success_at,
        catalogue_error: cache.last_error.clone(),
        windows: cfg!(windows),
        os: std::env::consts::OS.to_string(),
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
        assoc::register(&settings.chosen(app), &settings, &mut inventory).map_err(|e| e.to_string())?;
        if !settings.associated_apps.contains(&code) {
            settings.associated_apps.push(code);
        }
    } else {
        assoc::unregister(app, &mut inventory).map_err(|e| e.to_string())?;
        settings.associated_apps.retain(|c| c != &code);
    }
    settings.save().map_err(|e| e.to_string())
}

/// Keeps or takes out file types of an app: one (`ext`), or all of them
/// (`ext` = None, "select all" / "deselect all"). An associated app is
/// registered again at once, with its new set of types.
#[tauri::command]
fn set_extension(shared: tauri::State<'_, Shared>, code: String, ext: Option<String>, on: bool) -> Result<(), String> {
    let catalogue = shared.catalogue();
    let app = catalogue.app(&code).ok_or("unknown app")?;
    let exts = match ext {
        Some(ext) if app.extensions().contains(&ext) => vec![ext],
        Some(_) => return Err("unknown file type".into()),
        None => app.extensions(),
    };
    let mut settings = Settings::load();
    let excluded = settings.excluded_exts.entry(code.clone()).or_default();
    excluded.retain(|e| !exts.contains(e));
    if !on {
        excluded.extend(exts);
    }
    if settings.excluded_exts.get(&code).is_some_and(|x| x.is_empty()) {
        settings.excluded_exts.remove(&code);
    }
    settings.save().map_err(|e| e.to_string())?;
    if settings.associated_apps.contains(&code) {
        let mut inventory = Inventory::load();
        assoc::register_again(app, &settings.chosen(app), &settings, &mut inventory).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Ticks or unticks one shortcut of an app ("" = the app itself, else a
/// facade path), or all of them (`key` None). There is no separate switch:
/// an app has shortcuts as long as one is ticked, so ticking the first one
/// of an app without any starts from none, and unticking the last one
/// removes the app's shortcuts. They are rebuilt at once.
#[tauri::command]
fn set_shortcut_item(shared: tauri::State<'_, Shared>, code: String, key: Option<String>, on: bool, lang: String) -> Result<(), String> {
    let catalogue = shared.catalogue();
    let app = catalogue.app(&code).ok_or("unknown app")?;
    let mut settings = Settings::load();
    let keys: Vec<String> = shortcuts::all_items(app, &settings, &lang).into_iter().map(|i| i.key).collect();
    let targets = match key {
        Some(k) if keys.contains(&k) => vec![k],
        Some(_) => return Err("unknown shortcut".into()),
        None => keys.clone(),
    };
    let had = settings.shortcut_apps.contains(&code);
    let mut excluded: Vec<String> =
        if had { settings.excluded_shortcuts.get(&code).cloned().unwrap_or_default() } else { keys.clone() };
    excluded.retain(|k| !targets.contains(k));
    if !on {
        excluded.extend(targets);
    }
    let any = keys.iter().any(|k| !excluded.contains(k));
    settings.excluded_shortcuts.remove(&code);
    settings.shortcut_apps.retain(|c| c != &code);
    if any {
        if !excluded.is_empty() {
            settings.excluded_shortcuts.insert(code.clone(), excluded);
        }
        settings.shortcut_apps.push(code.clone());
    }
    settings.save().map_err(|e| e.to_string())?;
    let mut inventory = Inventory::load();
    shortcuts::remove(app, &lang, &mut inventory).map_err(|e| e.to_string())?;
    if any {
        shortcuts::create(app, &settings, &lang, &mut inventory).map_err(|e| e.to_string())?;
    }
    Ok(())
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
/// What "Check now" found: the catalogue, and the launcher's own version.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CheckReport {
    /// "updated", "unchanged", "failed" or "recent" (checked less than a
    /// minute ago: not asked again).
    catalogue: String,
    catalogue_error: Option<String>,
    /// This launcher's version, and the latest published one (None when
    /// GitHub could not be asked).
    current: String,
    latest: Option<String>,
    newer: bool,
}

/// Where the launcher's releases are published (its public repository).
const RELEASES_API: &str = "https://api.github.com/repos/kynoko/kynoko-launcher/releases/latest";
pub(crate) const RELEASES_PAGE: &str = "https://github.com/kynoko/kynoko-launcher/releases/latest";

/// The latest published version ("0.2.11"), from GitHub's public API.
fn latest_release() -> Result<String, String> {
    let agent = ureq::AgentBuilder::new().timeout(Duration::from_secs(10)).build();
    let release: serde_json::Value = agent
        .get(RELEASES_API)
        .set("User-Agent", "kynoko-launcher")
        .set("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| e.to_string())?
        .into_json()
        .map_err(|e| e.to_string())?;
    release["tag_name"].as_str().map(|t| t.trim_start_matches('v').to_string()).ok_or_else(|| "no tag".to_string())
}

/// Whether `latest` is a newer version than `current` ("0.2.11" > "0.2.9").
fn is_newer(latest: &str, current: &str) -> bool {
    let parse = |v: &str| v.split('.').map(|n| n.parse::<u64>().unwrap_or(0)).collect::<Vec<_>>();
    parse(latest) > parse(current)
}

/// "Check now": the catalogue (at most once a minute) and the launcher's
/// version, both answered, off the window's thread.
#[tauri::command]
async fn check_catalogue(app: AppHandle) -> Result<CheckReport, String> {
    let current = app.package_info().version.to_string();
    tauri::async_runtime::spawn_blocking(move || {
        let recent = {
            let shared = app.state::<Shared>();
            let mut last = shared.last_manual_check.lock().expect("lock");
            let recent = last.is_some_and(|t| t.elapsed() < Duration::from_secs(60));
            if !recent {
                *last = Some(Instant::now());
            }
            recent
        };
        let (catalogue, catalogue_error) = if recent {
            ("recent".to_string(), None)
        } else {
            match check_online(&app) {
                Ok(true) => ("updated".to_string(), None),
                Ok(false) => ("unchanged".to_string(), None),
                Err(e) => ("failed".to_string(), Some(e)),
            }
        };
        let latest = latest_release().ok();
        let newer = latest.as_deref().is_some_and(|l| is_newer(l, &current));
        CheckReport { catalogue, catalogue_error, current, latest, newer }
    })
    .await
    .map_err(|e| e.to_string())
}

/// Opens the page of the latest release in the system's browser.
#[tauri::command]
fn open_download() -> Result<(), String> {
    launch::open(RELEASES_PAGE, None, None).map_err(|e| e.to_string())
}

/// One check of the online catalogue: Ok(true) when it changed (and was
/// applied), Ok(false) when it did not.
fn check_online(app: &AppHandle) -> Result<bool, String> {
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
            Ok(true)
        }
        Refresh::Unchanged => Ok(false),
        // Kept in the cache and shown quietly in the window; never a notification.
        Refresh::Failed(e) => {
            eprintln!("kynoko-launcher: catalogue not refreshed: {e}");
            Err(e.to_string())
        }
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
        // Types, facade names and routes: anything the system shows.
        if after.map(|a| settings.chosen(&a.for_system())) == Some(settings.chosen(&before.for_system())) {
            continue;
        }
        match after {
            Some(a) => {
                let _ = assoc::register_again(before, &settings.chosen(a), &settings, &mut inventory);
            }
            None => {
                let _ = assoc::unregister(before, &mut inventory);
                settings.associated_apps.retain(|c| c != &code);
            }
        }
    }
    for code in settings.shortcut_apps.clone() {
        let (Some(before), after) = (old.app(&code), new.app(&code)) else { continue };
        if after.map(App::for_system) == Some(before.for_system()) {
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
/// After an update, the associations and shortcuts are written again by the
/// new version: an older one may have written other names, icons or
/// commands. After a reinstall that kept the preferences, this is what puts
/// them back. Once per version, in the background.
fn refresh_registrations(app: &AppHandle) {
    let version = app.package_info().version.to_string();
    if Settings::load().registered_by.as_deref() == Some(version.as_str()) {
        return;
    }
    let catalogue = app.state::<Shared>().catalogue();
    thread::spawn(move || {
        let settings = Settings::load();
        let mut inventory = Inventory::load();
        for code in settings.associated_apps.clone() {
            if let Some(app) = catalogue.app(&code) {
                let _ = assoc::register_again(app, &settings.chosen(app), &settings, &mut inventory);
            }
        }
        let lang = settings.ui_lang.clone().unwrap_or_else(|| "en".to_string());
        for code in settings.shortcut_apps.clone() {
            if let Some(app) = catalogue.app(&code) {
                let _ = shortcuts::remove(app, &lang, &mut inventory);
                let _ = shortcuts::create(app, &settings, &lang, &mut inventory);
            }
        }
        // Read again: the window may have changed a setting meanwhile.
        let mut latest = Settings::load();
        latest.registered_by = Some(version);
        let _ = latest.save();
    });
}

fn watch_catalogue(app: AppHandle) {
    thread::spawn(move || {
        let mut jitter = [0u8; 2];
        let _ = getrandom::getrandom(&mut jitter);
        // A first run has nothing but the bundled copy: ask soon.
        let first_delay = if Cache::load().success_at.is_none() { 5 } else { 30 + u16::from_le_bytes(jitter) as u64 % 570 };
        thread::sleep(Duration::from_secs(first_delay));
        loop {
            if catalogue::now() >= Cache::load().due_at() {
                let _ = check_online(&app);
            }
            thread::sleep(Duration::from_secs(600));
        }
    });
}

/// An app's icon for the window, as a data: URL (the window's CSP loads
/// nothing remote). From the app's web manifest, kept on disk and checked
/// again after ICON_RECHECK (see shortcuts::kept_icon).
#[tauri::command]
async fn app_icon(shared: tauri::State<'_, Shared>, code: String) -> Result<Option<String>, String> {
    if !plain_code(&code) {
        return Ok(None);
    }
    let catalogue = shared.catalogue();
    let Some(app) = catalogue.app(&code) else { return Ok(None) };
    let manifest = format!("{}/manifest.webmanifest", Settings::load().url_of(app).trim_end_matches('/'));
    let path = settings::dir().join(settings::UI_ICONS).join(format!("{code}.png"));
    let png = tauri::async_runtime::spawn_blocking(move || shortcuts::kept_icon(&manifest, &path, ICON_RECHECK))
        .await
        .map_err(|e| e.to_string())?;
    Ok(png.filter(|b| b.starts_with(b"\x89PNG")).map(|b| format!("data:image/png;base64,{}", base64(&b))))
}

/// Standard base64, for a data: URL (one small image, no crate needed).
fn base64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (u32::from(chunk[0]) << 16) | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8) | u32::from(*chunk.get(2).unwrap_or(&0));
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

#[tauri::command]
fn launch(shared: tauri::State<'_, Shared>, target: String) -> Result<(), String> {
    launch_app(&shared, &target)
}

#[tauri::command]
fn remove_everything() -> Result<(), String> {
    cleanup(false)
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
        dispatch(app, Command::Open(None, files));
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
    // Windows only lets the program the user just started bring a window to
    // the front. A double-clicked file starts THIS instance, which hands the
    // file to the launcher already running: lend it that right first, or the
    // window opened for the file would stay behind the others.
    #[cfg(windows)]
    // SAFETY: plain Win32 call, no pointer involved.
    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow(
            windows_sys::Win32::UI::WindowsAndMessaging::ASFW_ANY,
        );
    }
    // Started as an app's own program ("Open with"): the launcher takes over.
    #[cfg(windows)]
    if programs::hand_over() {
        return;
    }

    let args: Vec<String> = std::env::args().skip(1).collect();
    let first = parse(&args);
    let show_window = matches!(first, Command::Window(_));
    let cleaning = matches!(first, Command::Cleanup { .. });

    let mut builder = tauri::Builder::default();
    // One launcher at a time: a second start hands its arguments over. An
    // ISOLATED run (end-to-end tests) stays out of it, so that a test never
    // lands in the installed launcher of the machine it runs on.
    if !settings::isolated() {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            dispatch(app, parse(&argv[1..]));
        }));
    }
    builder
        .manage(Shared {
            catalogue: RwLock::new(Cache::load().catalogue()),
            last_manual_check: Mutex::new(None),
            bridge: Mutex::new(None),
            handoff: Mutex::new(None),
            last_app_window: Mutex::new(None),
            page_loads: Mutex::new(std::collections::HashMap::new()),
            title_bar_claims: Mutex::new(std::collections::HashMap::new()),
            handle: std::sync::OnceLock::new(),
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
            set_extension,
            set_shortcut_item,
            own_window_drag,
            own_window_minimize,
            own_window_toggle_maximize,
            own_window_is_maximized,
            own_window_close,
            system_fonts,
            app_icon,
            set_shortcuts,
            check_catalogue,
            launch,
            remove_everything,
            open_default_apps,
            open_download
        ])
        .setup(move |app| {
            let handle = app.handle().clone();
            let _ = handle.state::<Shared>().handle.set(handle.clone());
            if !cleaning && !settings::isolated() {
                if let Err(e) = assoc::register_scheme(&mut Inventory::load()) {
                    eprintln!("kynoko-launcher: cannot register {}://: {e}", assoc::SCHEME);
                }
            }
            if !cleaning {
                refresh_registrations(&handle);
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
            // Closing the settings window hides it: an open file may still
            // need the bridge. The idle watch quits once nothing does. A
            // Kynoko window (an app page) really closes: hidden, it would
            // keep the launcher and its page alive for nothing.
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" {
                    api.prevent_close();
                    let _ = window.hide();
                } else {
                    *window.state::<Shared>().last_app_window.lock().expect("lock") = Some(Instant::now());
                }
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
    fn base64_encoding() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64(&[0xff, 0xfe]), "//4=");
    }

    #[test]
    fn versions() {
        assert!(is_newer("0.2.11", "0.2.9"));
        assert!(is_newer("0.3.0", "0.2.11"));
        assert!(!is_newer("0.2.11", "0.2.11"));
        assert!(!is_newer("0.2.9", "0.2.11"));
    }

    #[test]
    fn cleanup_arguments() {
        let args = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(matches!(parse(&args(&["cleanup"])), Command::Cleanup { keep_preferences: false }));
        assert!(matches!(parse(&args(&["cleanup", "--keep-preferences"])), Command::Cleanup { keep_preferences: true }));
    }

    #[test]
    fn open_arguments() {
        let args = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        match parse(&args(&["open", "--app", "MediaStudio", "C:\\a b.png"])) {
            Command::Open(Some(app), files) => {
                assert_eq!(app, "MediaStudio");
                assert_eq!(files, [PathBuf::from("C:\\a b.png")]);
            }
            _ => panic!("open --app"),
        }
        assert!(matches!(parse(&args(&["open", "x.docx"])), Command::Open(None, f) if f.len() == 1));
        assert!(matches!(parse(&args(&["open", "--app", "../evil", "x"])), Command::Open(None, _)));
    }

    #[test]
    fn chosen_types() {
        let c = Catalogue::bundled();
        let office = c.app("Office").unwrap();
        assert_eq!(office.facade_for("csv").map(|f| f.slug()), Some("spreadsheet"));
        let photo = c.app("PhotoStudio").unwrap();
        // Listed by five facades: the one marking it primary opens it.
        assert_eq!(photo.facade_for("png").map(|f| f.slug()), Some("express"));
        assert_eq!(photo.facade_for("psd").map(|f| f.slug()), Some("edit"));
        let mut s = Settings::default();
        s.excluded_exts.insert("Office".into(), vec!["csv".into(), "txt".into()]);
        let kept = s.chosen(office).extensions();
        assert!(!kept.contains(&"csv".to_string()) && !kept.contains(&"txt".to_string()));
        assert!(kept.contains(&"tsv".to_string()));
        assert_eq!(kept.len(), office.extensions().len() - 2);
        let groups = type_groups(office, &s, "fr");
        assert_eq!(groups.iter().map(|g| g.facade.as_str()).collect::<Vec<_>>(), ["Document", "Tableur", "Présentation"]);
        assert!(groups[1].exts.iter().any(|e| e.ext == "csv" && !e.on));
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
