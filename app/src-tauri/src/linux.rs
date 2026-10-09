//! Linux integration (docs/SPEC.md, sections 5 to 7): browsers from their
//! desktop entries, associations through a desktop entry per app, a
//! shared-mime-info package and `mimeapps.list`, shortcuts as desktop entries
//! whose actions are the app's facades. Everything user-level (~/.local,
//! ~/.config), everything recorded in the inventory before it is written.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::browsers::{engine_of, web_engine_of, Browser, Engine, Profile};
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
/// running binary lives in a mount that disappears when it exits), its
/// installed copy once there is one (see install_self).
fn exe() -> String {
    match appimage() {
        Some(running) => appimage_target(&running).to_string_lossy().into_owned(),
        None => std::env::current_exe().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default(),
    }
}

/* ---------------------------------------------------------------- AppImage */

/// The AppImage this launcher runs from, if it does (the .deb and the .rpm don't).
fn appimage() -> Option<PathBuf> {
    std::env::var_os("APPIMAGE").filter(|p| !p.is_empty()).map(PathBuf::from)
}

/// Where the AppImage installs itself: a place of its own, so that the
/// shortcuts and file types it writes never point into a Downloads folder
/// someone may tidy, and its menu entry stays valid.
fn installed_appimage() -> PathBuf {
    data_home().join("kynoko-launcher/kynoko-launcher.AppImage")
}

const SELF_ENTRY: &str = "kynoko-launcher.desktop";

/// The launcher's icon, for its own menu entry (the release's, see the CI).
const ICON: &[u8] = include_bytes!("../icons/128x128.png");

/// The AppImage to register, run and update: the installed copy once it
/// exists, else the one running.
pub fn appimage_target(running: &Path) -> PathBuf {
    let installed = installed_appimage();
    if installed.is_file() {
        installed
    } else {
        running.to_path_buf()
    }
}

/// The AppImage installs itself (docs/SPEC.md, section 12): a copy in
/// ~/.local/share/kynoko-launcher, executable, and its own "Kynoko Launcher"
/// menu entry. Started from elsewhere, a newer version replaces the copy
/// (an older one leaves it). Not in the inventory: resetting the launcher's
/// choices leaves it installed; `cleanup`, what one runs before deleting
/// the AppImage, removes it (remove_self).
pub fn install_self(version: &str) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let Some(running) = appimage() else { return Ok(()) };
    let installed = installed_appimage();
    let dir = installed.parent().expect("a folder").to_path_buf();
    let stamp = dir.join("version");
    let same = std::fs::canonicalize(&running).ok().is_some_and(|r| std::fs::canonicalize(&installed).ok() == Some(r));
    let kept = std::fs::read_to_string(&stamp).unwrap_or_default();
    if !same && (!installed.is_file() || crate::is_newer(version, kept.trim())) {
        std::fs::create_dir_all(&dir)?;
        let next = dir.join(".kynoko-launcher.AppImage.new");
        std::fs::copy(&running, &next)?;
        std::fs::set_permissions(&next, std::fs::Permissions::from_mode(0o755))?;
        std::fs::rename(&next, &installed)?;
        std::fs::write(&stamp, version)?;
    } else if same && kept.trim() != version {
        // Updated in place (update.rs): the copy is this version now.
        std::fs::write(&stamp, version)?;
    }
    let icon = dir.join("kynoko-launcher.png");
    if std::fs::read(&icon).ok().as_deref() != Some(ICON) {
        std::fs::create_dir_all(&dir)?;
        std::fs::write(&icon, ICON)?;
    }
    let entry = applications().join(SELF_ENTRY);
    let text = xdg::render_entry(
        &[
            ("Type", "Application".into()),
            ("Name", "Kynoko Launcher".into()),
            ("Exec", xdg::exec_quote(&installed.to_string_lossy())),
            ("Icon", icon.to_string_lossy().into_owned()),
            ("Categories", "Utility;".into()),
            ("Terminal", "false".into()),
        ],
        &[],
    );
    if std::fs::read_to_string(&entry).ok().as_deref() != Some(text.as_str()) {
        std::fs::create_dir_all(applications())?;
        std::fs::write(&entry, text)?;
        refresh_menus();
    }
    Ok(())
}

/// Undoes install_self.
pub fn remove_self() {
    let _ = std::fs::remove_file(applications().join(SELF_ENTRY));
    if let Some(dir) = installed_appimage().parent() {
        let _ = std::fs::remove_dir_all(dir);
    }
    refresh_menus();
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
            let web_engine = web_engine_of(&engine, &command.join(" "));
            out.push(Browser {
                id,
                name: e.get("Name").cloned().unwrap_or(program.clone()),
                exe: command[0].clone(),
                engine,
                web_engine,
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

/// The prefix of `app`'s open entries: one per facade its types open in
/// (`kynoko-launcher-open-<app>-<facade>.desktop`), so "Open with" shows the
/// app's name ("Kynoko Office") with the facade's icon. Launcher 0.1 wrote a
/// single `...-<app>.desktop`, still recognised on removal.
fn open_prefix(app: &App) -> String {
    format!("kynoko-launcher-open-{}-", app.code)
}

fn is_open_entry(app: &App, name: &str) -> bool {
    let legacy = format!("kynoko-launcher-open-{}.desktop", app.code);
    name.ends_with(&legacy) || name.rsplit('/').next().is_some_and(|n| n.starts_with(&open_prefix(app)) && n.ends_with(".desktop"))
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

    let lang = settings.ui_lang.as_deref().unwrap_or("en");
    // Each type goes to the facade it opens in: one entry per such facade.
    let mut by_facade: Vec<(&crate::catalogue::Facade, Vec<&str>)> = Vec::new();
    for (mime, ext) in &pairs {
        let Some(facade) = app.facade_for(ext) else { continue };
        match by_facade.iter_mut().find(|(f, _)| f.path == facade.path) {
            Some((_, mimes)) => mimes.push(mime),
            None => by_facade.push((facade, vec![mime])),
        }
    }
    let mut list = std::fs::read_to_string(mimeapps()).unwrap_or_default();
    for (facade, mimes) in by_facade {
        let entry = format!("{}{}.desktop", open_prefix(app), facade.slug());
        let mut fields = vec![
            ("Type", "Application".to_string()),
            ("Name", app.display_name(lang)),
            ("Comment", facade.name(lang)),
            ("Exec", format!("{} open --app {} %F", xdg::exec_quote(&exe()), app.code)),
            ("MimeType", format!("{};", mimes.join(";"))),
            ("NoDisplay", "true".into()),
        ];
        if let Some(png) = crate::shortcuts::type_icon(app, facade, settings, "png", inventory) {
            fields.push(("Icon", png.to_string_lossy().into_owned()));
        }
        write_recorded(&applications().join(&entry), &xdg::render_entry(&fields, &[]), inventory)?;

        // Default handler of each type, the previous one kept to be restored.
        for mime in mimes {
            let previous = xdg::mimeapps_get(&list, xdg::defaults_group(), mime);
            if previous.as_deref() != Some(entry.as_str()) {
                inventory.record(Artefact::MimeDefault { mime: mime.to_string(), desktop: entry.clone(), previous })?;
            }
            list = xdg::mimeapps_set(&list, xdg::defaults_group(), mime, Some(&entry));
            list = xdg::mimeapps_added(&list, mime, &entry, true);
        }
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
    for a in inventory.artefacts.clone() {
        match &a {
            Artefact::MimeDefault { mime, desktop, previous } if is_open_entry(app, desktop) => {
                restore_default(mime, desktop, previous.as_deref());
                inventory.forget(&a)?;
            }
            Artefact::File { path } if is_open_entry(app, path) || path.ends_with(&format!("kynoko-launcher-{}.xml", app.code)) => {
                let _ = std::fs::remove_file(path);
                inventory.forget(&a)?;
            }
            _ => {}
        }
    }
    crate::shortcuts::remove_type_icons(app, inventory)?;
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

/// The app's own entry: also where launcher 0.2.6 and before put the app
/// with its facades as actions (see shortcuts::remove).
pub fn main_entry(app: &App) -> String {
    applications().join(format!("kynoko-{}.desktop", app.code)).to_string_lossy().into_owned()
}

/// One desktop entry per kept item, in the "Kynoko" submenu (category
/// X-Kynoko, see ensure_menu).
pub fn write_shortcut(app: &App, item: &crate::shortcuts::Item, icon: Option<&Path>, inventory: &mut Inventory) -> std::io::Result<()> {
    let path = if item.slug.is_empty() {
        PathBuf::from(main_entry(app))
    } else {
        let slug: String = item.slug.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
        applications().join(format!("kynoko-{}--{slug}.desktop", app.code))
    };
    let mut fields = vec![
        ("Type", "Application".to_string()),
        ("Name", item.name.clone()),
        ("Exec", format!("{} launch {}", xdg::exec_quote(&exe()), item.target)),
        ("Categories", "X-Kynoko;".to_string()),
    ];
    if let Some(png) = icon {
        fields.push(("Icon", png.to_string_lossy().into_owned()));
    }
    inventory.record(Artefact::Shortcut { path: path.to_string_lossy().into_owned(), app: app.code.clone() })?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&path, xdg::render_entry(&fields, &[]))
}

fn config_home() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".config"))
}

/// The "Kynoko" submenu: a merged menu file and its directory entry, shared
/// by every app (removed with the last app's entries).
pub fn menu_files() -> Vec<String> {
    vec![
        config_home().join("menus/applications-merged/kynoko-launcher.menu").to_string_lossy().into_owned(),
        data_home().join("desktop-directories/kynoko-launcher.directory").to_string_lossy().into_owned(),
    ]
}

pub fn ensure_menu(inventory: &mut Inventory) -> std::io::Result<()> {
    let files = menu_files();
    let menu = "<!DOCTYPE Menu PUBLIC \"-//freedesktop//DTD Menu 1.0//EN\"\n \"http://www.freedesktop.org/standards/menu-spec/1.0/menu.dtd\">\n\
<Menu>\n  <Name>Applications</Name>\n  <Menu>\n    <Name>Kynoko</Name>\n    <Directory>kynoko-launcher.directory</Directory>\n\
    <Include><Category>X-Kynoko</Category></Include>\n  </Menu>\n</Menu>\n";
    write_recorded(Path::new(&files[0]), menu, inventory)?;
    write_recorded(Path::new(&files[1]), "[Desktop Entry]\nType=Directory\nName=Kynoko\n", inventory)
}

pub fn refresh_menus() {
    let _ = Command::new("update-desktop-database").arg(applications()).status();
}
