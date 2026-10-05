//! Shortcuts to the apps (docs/SPEC.md, section 6).
//!
//! Windows: a folder per app in the Start menu, holding the app and its
//! facades (the Start menu's own "submenu"). Every shortcut runs the launcher
//! (`launch <app>[/<facade>]`), never the browser: changing browsers never
//! rewrites a shortcut. Icons come from the apps' own web manifests at run
//! time (brand assets are not in this repository) and are turned into .ico
//! files. Everything written is recorded in the inventory first.

use std::io::Read;
use std::path::{Path, PathBuf};

use crate::catalogue::{App, Facade};
use crate::settings::{self, Artefact, Inventory, Settings};

/// Where downloaded icons live (removed with the rest by the cleanup).
fn icons_dir() -> PathBuf {
    settings::dir().join("icons")
}

/// A file name Windows, macOS and Linux all accept.
pub(crate) fn file_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| if matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || c.is_control() { ' ' } else { c })
        .collect();
    let trimmed = cleaned.trim().trim_end_matches('.').to_string();
    if trimmed.is_empty() { "Kynoko".to_string() } else { trimmed }
}

/// `src` of a manifest icon, made absolute against the manifest's address.
fn resolve(manifest_url: &str, src: &str) -> String {
    if src.starts_with("http://") || src.starts_with("https://") {
        return src.to_string();
    }
    let after_scheme = manifest_url.find("://").map(|i| i + 3).unwrap_or(0);
    let origin_end = manifest_url[after_scheme..].find('/').map(|i| i + after_scheme).unwrap_or(manifest_url.len());
    if src.starts_with('/') {
        format!("{}{}", &manifest_url[..origin_end], src)
    } else {
        let dir_end = manifest_url.rfind('/').map(|i| i + 1).unwrap_or(manifest_url.len());
        format!("{}{}", &manifest_url[..dir_end], src)
    }
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new().timeout(std::time::Duration::from_secs(10)).build()
}

fn body(response: ureq::Response) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    response.into_reader().read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    Ok(bytes)
}

/// The largest PNG "any" icon a web manifest declares, downloaded.
pub(crate) fn fetch_icon(manifest_url: &str) -> Result<Vec<u8>, String> {
    let agent = agent();
    let src = icon_src(&agent, manifest_url)?;
    body(agent.get(&src).call().map_err(|e| e.to_string())?)
}

/// The address of the largest PNG "any" icon a web manifest declares.
fn icon_src(agent: &ureq::Agent, manifest_url: &str) -> Result<String, String> {
    let manifest: serde_json::Value = agent
        .get(manifest_url)
        .call()
        .map_err(|e| e.to_string())?
        .into_json()
        .map_err(|e| e.to_string())?;
    let best = manifest["icons"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|i| i["purpose"].as_str().map(|p| p.split_whitespace().any(|x| x == "any")).unwrap_or(true))
        .filter(|i| i["type"].as_str().map(|t| t == "image/png").unwrap_or(true))
        .max_by_key(|i| {
            i["sizes"].as_str().and_then(|s| s.split('x').next()).and_then(|n| n.parse::<u32>().ok()).unwrap_or(0)
        })
        .and_then(|i| i["src"].as_str())
        .ok_or("no icon")?
        .to_string();
    Ok(resolve(manifest_url, &best))
}

/// What tells a kept icon's version (`<icon>.json` beside it): its address
/// and the validators its server gave.
#[derive(Default, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct IconVersion {
    src: String,
    etag: Option<String>,
    last_modified: Option<String>,
}

/// A manifest's icon kept at `path`, used as is when checked less than
/// `recheck` ago. Older, it is asked for again with what tells its version:
/// unchanged, the server says so (304) and nothing is downloaded; changed,
/// the new one replaces it (an app's new icon shows within `recheck`).
/// Offline, or given something that is not a PNG, the kept one serves.
pub(crate) fn kept_icon(manifest_url: &str, path: &Path, recheck: std::time::Duration) -> Option<Vec<u8>> {
    let kept = std::fs::read(path).ok();
    let checked = std::fs::metadata(path).and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok());
    if kept.is_some() && checked.is_some_and(|age| age < recheck) {
        return kept;
    }
    let version_path = path.with_extension("json");
    let agent = agent();
    let Ok(src) = icon_src(&agent, manifest_url) else { return kept };
    let mut request = agent.get(&src);
    let known = std::fs::read_to_string(&version_path).ok().and_then(|s| serde_json::from_str::<IconVersion>(&s).ok());
    if let (Some(_), Some(known)) = (&kept, known.filter(|k| k.src == src)) {
        if let Some(etag) = &known.etag {
            request = request.set("If-None-Match", etag);
        }
        if let Some(date) = &known.last_modified {
            request = request.set("If-Modified-Since", date);
        }
    }
    let Ok(response) = request.call() else { return kept };
    if response.status() == 304 {
        // Unchanged: checked now.
        let _ = std::fs::File::options().write(true).open(path).and_then(|f| f.set_modified(std::time::SystemTime::now()));
        return kept;
    }
    let version = IconVersion {
        src,
        etag: response.header("ETag").map(str::to_string),
        last_modified: response.header("Last-Modified").map(str::to_string),
    };
    let fresh = body(response).ok().filter(|b| b.starts_with(b"\x89PNG"));
    let Some(bytes) = fresh else { return kept };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, &bytes);
    let _ = std::fs::write(&version_path, serde_json::to_string(&version).unwrap_or_default());
    Some(bytes)
}

/// A PNG turned into an .ico holding 256, 48, 32 and 16 pixel frames (each
/// stored as PNG, which Windows reads since Vista).
pub fn png_to_ico(png: &[u8]) -> Result<Vec<u8>, String> {
    use image::ImageEncoder;
    let source = image::load_from_memory_with_format(png, image::ImageFormat::Png).map_err(|e| e.to_string())?;
    let sizes = [256u32, 48, 32, 16];
    let mut frames: Vec<Vec<u8>> = Vec::new();
    for size in sizes {
        let resized = source.resize_exact(size, size, image::imageops::FilterType::Lanczos3).to_rgba8();
        let mut out = Vec::new();
        image::codecs::png::PngEncoder::new(&mut out)
            .write_image(resized.as_raw(), size, size, image::ExtendedColorType::Rgba8)
            .map_err(|e| e.to_string())?;
        frames.push(out);
    }
    let mut ico = Vec::new();
    ico.extend_from_slice(&[0, 0, 1, 0]);
    ico.extend_from_slice(&(frames.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * frames.len() as u32;
    for (size, frame) in sizes.iter().zip(&frames) {
        let dim = if *size >= 256 { 0 } else { *size as u8 };
        ico.extend_from_slice(&[dim, dim, 0, 0]);
        ico.extend_from_slice(&1u16.to_le_bytes()); // colour planes
        ico.extend_from_slice(&32u16.to_le_bytes()); // bits per pixel
        ico.extend_from_slice(&(frame.len() as u32).to_le_bytes());
        ico.extend_from_slice(&offset.to_le_bytes());
        offset += frame.len() as u32;
    }
    for frame in frames {
        ico.extend_from_slice(&frame);
    }
    Ok(ico)
}

/// Where the mark on Kynoko's files comes from: the platform's own web
/// manifest, whose icon is the Kynoko tile (the mushroom). A brand picture,
/// fetched at run time like the apps' icons, never kept in this repository.
const MARK_MANIFEST: &str = "https://kynoko.com/manifest.webmanifest";
/// How long the kept mark serves before it is checked again.
const MARK_RECHECK: std::time::Duration = std::time::Duration::from_secs(7 * 24 * 3600);

/// The Kynoko mark, kept beside the icons; None when it was never had (offline).
fn mark(inventory: &mut Inventory) -> Option<Vec<u8>> {
    let path = icons_dir().join("kynoko-mark.png");
    for kept in [path.clone(), path.with_extension("json")] {
        inventory.record(Artefact::File { path: kept.to_string_lossy().into_owned() }).ok()?;
    }
    kept_icon(MARK_MANIFEST, &path, MARK_RECHECK)
}

/// Boréal's facet, kynoko-ui's corners (its 16px 4px radius tokens and their
/// kin), on a square of side `side`: the top-left and bottom-right corners
/// ample, a quarter of the side, the other two sharp, 6 % - as measured on
/// the Kynoko tile itself (128 and 30 px of 512).
fn in_facet(x: f32, y: f32, side: f32) -> bool {
    if x < 0.0 || y < 0.0 || x > side || y > side {
        return false;
    }
    let (left, top) = (x < side / 2.0, y < side / 2.0);
    let r = if left == top { side * 0.25 } else { side * 0.06 };
    let (cx, cy) = (if left { r } else { side - r }, if top { r } else { side - r });
    let in_corner = (if left { x < cx } else { x > cx }) && (if top { y < cy } else { y > cy });
    !in_corner || (x - cx).powi(2) + (y - cy).powi(2) <= r * r
}

/// How much of pixel (px, py) the facet of side `side` placed at (x0, y0)
/// covers, from 0 to 1 (4 × 4 samples: smooth edges at every size).
fn facet_cover(px: u32, py: u32, x0: f32, y0: f32, side: f32) -> f32 {
    let mut inside = 0;
    for sy in 0..4 {
        for sx in 0..4 {
            let x = px as f32 + (sx as f32 + 0.5) / 4.0 - x0;
            let y = py as f32 + (sy as f32 + 0.5) / 4.0 - y0;
            if in_facet(x, y, side) {
                inside += 1;
            }
        }
    }
    inside as f32 / 16.0
}

/// The icon a Kynoko app's files wear (a 256 px PNG): its facade's icon cut
/// to the facet, and the Kynoko mark in the corner the facet frees, bottom
/// right, parted from the icon by a transparent ring so that it reads in a
/// light folder and a dark one alike. Without the mark, the facet alone.
pub(crate) fn file_icon(icon: &[u8], mark: Option<&[u8]>) -> Result<Vec<u8>, String> {
    use image::ImageEncoder;
    const SIDE: u32 = 256;
    const MARK: u32 = SIDE * 9 / 25; // 36 % of the side: a small mark, the app's drawing first
    let load = |png: &[u8], size: u32| -> Result<image::RgbaImage, String> {
        Ok(image::load_from_memory_with_format(png, image::ImageFormat::Png)
            .map_err(|e| e.to_string())?
            .resize_exact(size, size, image::imageops::FilterType::Lanczos3)
            .to_rgba8())
    };
    let (side, mark_side, gap) = (SIDE as f32, MARK as f32, SIDE as f32 * 0.035);
    let corner = side - mark_side;
    let mut out = load(icon, SIDE)?;
    let mark = mark.and_then(|m| load(m, MARK).ok());
    for (x, y, pixel) in out.enumerate_pixels_mut() {
        let mut kept = facet_cover(x, y, 0.0, 0.0, side);
        if mark.is_some() {
            kept *= 1.0 - facet_cover(x, y, corner - gap, corner - gap, mark_side + 2.0 * gap);
        }
        pixel[3] = (pixel[3] as f32 * kept).round() as u8;
    }
    if let Some(mark) = mark {
        for (x, y, m) in mark.enumerate_pixels() {
            let a = facet_cover(x, y, 0.0, 0.0, mark_side) * m[3] as f32 / 255.0;
            if a <= 0.0 {
                continue;
            }
            // Over what is left beneath (nothing, the ring cleared it).
            let pixel = out.get_pixel_mut(SIDE - MARK + x, SIDE - MARK + y);
            let below = pixel[3] as f32 / 255.0;
            let total = a + below * (1.0 - a);
            for c in 0..3 {
                let v = (m[c] as f32 * a + pixel[c] as f32 * below * (1.0 - a)) / total;
                pixel[c] = v.round().clamp(0.0, 255.0) as u8;
            }
            pixel[3] = (total * 255.0).round() as u8;
        }
    }
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png)
        .write_image(out.as_raw(), SIDE, SIDE, image::ExtendedColorType::Rgba8)
        .map_err(|e| e.to_string())?;
    Ok(png)
}

/// Downloads an icon into `name`.ico; None when it cannot be had (the
/// shortcut then wears the launcher's own icon).
fn icon_file(manifest_url: &str, name: &str, inventory: &mut Inventory) -> Option<PathBuf> {
    let ico = fetch_icon(manifest_url).and_then(|png| png_to_ico(&png)).ok()?;
    let path = icons_dir().join(format!("{name}.ico"));
    inventory.record(Artefact::File { path: path.to_string_lossy().into_owned() }).ok()?;
    std::fs::create_dir_all(icons_dir()).ok()?;
    std::fs::write(&path, ico).ok()?;
    Some(path)
}

/// A facade's web manifest.
fn facade_manifest(base: &str, facade: &Facade) -> String {
    format!("{}/assets/manifests/{}.webmanifest", base.trim_end_matches('/'), facade.slug())
}

/// The icon files of `app`'s types wear: its facades', named apart from the
/// shortcuts' (`assoc-<app>-<facade>`) so that removing the shortcuts never
/// takes them away. Kept once downloaded: turning one type off and on again
/// does not fetch anything.
fn type_icon_path(app: &App, facade: &Facade, format: &str) -> PathBuf {
    icons_dir().join(format!("assoc-{}-{}.{format}", app.code, facade.slug()))
}

/// The icon (`ico` or `png`) of the files `facade` opens (see file_icon);
/// None when it cannot be had, and the type then wears the launcher's own.
#[cfg_attr(not(any(windows, target_os = "linux")), allow(dead_code))]
pub(crate) fn type_icon(app: &App, facade: &Facade, settings: &Settings, format: &str, inventory: &mut Inventory) -> Option<PathBuf> {
    let path = type_icon_path(app, facade, format);
    let recorded = Artefact::File { path: path.to_string_lossy().into_owned() };
    if path.is_file() && inventory.artefacts.contains(&recorded) {
        return Some(path);
    }
    let icon = fetch_icon(&facade_manifest(&settings.url_of(app), facade)).ok()?;
    let mark = mark(inventory);
    let png = file_icon(&icon, mark.as_deref()).unwrap_or(icon);
    let bytes = if format == "ico" { png_to_ico(&png).ok()? } else { png };
    inventory.record(recorded).ok()?;
    std::fs::create_dir_all(icons_dir()).ok()?;
    std::fs::write(&path, bytes).ok()?;
    Some(path)
}

/// Removes the icons of `app`'s file types (its associations are gone).
pub(crate) fn remove_type_icons(app: &App, inventory: &mut Inventory) -> std::io::Result<()> {
    let prefix = icons_dir().join(format!("assoc-{}-", app.code)).to_string_lossy().into_owned();
    for a in inventory.artefacts.clone() {
        if let Artefact::File { path } = &a {
            if path.starts_with(&prefix) {
                let _ = std::fs::remove_file(path);
                inventory.forget(&a)?;
            }
        }
    }
    Ok(())
}

/// One menu entry of an app: the app itself (`key` ""), or one of its listed
/// facades (`key` = the facade's path).
pub struct Item {
    pub key: String,
    /// In a shared menu: "Media Studio", "Media Studio - Convertir" (Windows
    /// sorts them together, the app first).
    pub name: String,
    /// Inside the app's own folder (macOS): "Media Studio", "Convertir".
    pub short: String,
    /// The `launch` argument: "MediaStudio" or "MediaStudio/studio/convert".
    pub target: String,
    pub manifest: String,
    /// Unique within the app, for file names: "" (the app) or the facade slug.
    pub slug: String,
}

/// Every entry `app` can have, the app first.
pub fn all_items(app: &App, settings: &Settings, lang: &str) -> Vec<Item> {
    let base = settings.url_of(app);
    let base = base.trim_end_matches('/');
    let name = app.name(lang);
    let mut out = vec![Item {
        key: String::new(),
        name: name.clone(),
        short: name.clone(),
        target: app.code.clone(),
        manifest: format!("{base}/manifest.webmanifest"),
        slug: String::new(),
    }];
    // Only published facades: a draft opens its files but gets no entry.
    for f in app.facades.iter().filter(|f| f.listed) {
        let short = f.name(lang);
        out.push(Item {
            key: f.path.clone(),
            name: format!("{name} - {short}"),
            short,
            target: format!("{}/{}", app.code, f.path),
            manifest: facade_manifest(base, f),
            slug: f.slug().to_string(),
        });
    }
    out
}

/// The entries the user kept.
fn chosen(app: &App, settings: &Settings, lang: &str) -> Vec<Item> {
    all_items(app, settings, lang).into_iter().filter(|i| settings.shortcut_chosen(&app.code, &i.key)).collect()
}

/// The name an entry's icon file is kept under.
#[cfg_attr(not(any(windows, target_os = "linux")), allow(dead_code))]
fn icon_name(app: &App, item: &Item) -> String {
    if item.slug.is_empty() { app.code.clone() } else { format!("{}-{}", app.code, item.slug) }
}

/// A recorded entry, file or bundle, gone.
pub fn remove_path(path: &str) {
    let p = Path::new(path);
    if p.is_dir() {
        let _ = std::fs::remove_dir_all(p);
    } else {
        let _ = std::fs::remove_file(p);
    }
}

#[cfg(windows)]
fn start_menu() -> PathBuf {
    dirs::data_dir().unwrap_or_default().join(r"Microsoft\Windows\Start Menu\Programs")
}

/// The launcher's one folder in the Start menu. Windows shows a single level
/// of folders there (deeper ones are flattened), so the apps and their
/// facades sit side by side: "Media Studio", "Media Studio - Convertir".
#[cfg(windows)]
fn kynoko_folder() -> PathBuf {
    start_menu().join("Kynoko")
}

#[cfg(windows)]
pub fn create(app: &App, settings: &Settings, lang: &str, inventory: &mut Inventory) -> std::io::Result<()> {
    let exe = std::env::current_exe()?;
    let folder = kynoko_folder();
    inventory.record(Artefact::Dir { path: folder.to_string_lossy().into_owned() })?;
    std::fs::create_dir_all(&folder)?;
    for item in chosen(app, settings, lang) {
        let icon = icon_file(&item.manifest, &icon_name(app, &item), inventory);
        link(&exe, &folder.join(format!("{}.lnk", file_name(&item.name))), &item.target, icon.as_deref(), &app.code, inventory)?;
    }
    Ok(())
}

#[cfg(windows)]
fn link(exe: &Path, at: &Path, target: &str, icon: Option<&Path>, app: &str, inventory: &mut Inventory) -> std::io::Result<()> {
    inventory.record(Artefact::Shortcut { path: at.to_string_lossy().into_owned(), app: app.to_string() })?;
    let mut sl = mslnk::ShellLink::new(exe).map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
    sl.set_arguments(Some(format!("launch {target}")));
    if let Some(icon) = icon {
        sl.set_icon_location(Some(icon.to_string_lossy().into_owned()));
    }
    sl.create_lnk(at).map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
    // The app's taskbar identity, the one its windows wear: a pinned entry
    // and the windows it opens make one button (taskbar.rs).
    if let Err(e) = crate::taskbar::stamp_shortcut(at, &crate::taskbar::app_id(app)) {
        eprintln!("kynoko-launcher: shortcut identity: {e}");
    }
    Ok(())
}

/// Where launcher 0.2.6 and before put an app's entries: its own folder
/// (Windows, macOS) or one entry with the facades as actions (Linux). Still
/// cleaned, so that an update moves them into the shared place.
fn legacy_place(app: &App, lang: &str) -> String {
    #[cfg(windows)]
    return start_menu().join(file_name(&app.name(lang))).to_string_lossy().into_owned();
    #[cfg(target_os = "macos")]
    return crate::macos::shortcut_folder(app, lang);
    #[cfg(target_os = "linux")]
    return { let _ = lang; crate::linux::main_entry(app) };
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    return { let _ = (app, lang); String::from("\u{0}") };
}

/// What several apps share (the Kynoko folder or menu): removed with the
/// last app's entries.
fn shared_places() -> Vec<String> {
    #[cfg(windows)]
    return vec![kynoko_folder().to_string_lossy().into_owned()];
    #[cfg(target_os = "macos")]
    return vec![crate::macos::kynoko_folder()];
    #[cfg(target_os = "linux")]
    return crate::linux::menu_files();
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    return Vec::new();
}

/// Removes the app's entries and their icons, the shared place once no app
/// has an entry left, and what an older launcher left for this app.
pub fn remove(app: &App, lang: &str, inventory: &mut Inventory) -> std::io::Result<()> {
    let legacy = legacy_place(app, lang);
    let icon_prefix = icons_dir().join(&app.code).to_string_lossy().into_owned();
    let mine: Vec<Artefact> = inventory
        .artefacts
        .iter()
        .filter(|a| match a {
            Artefact::Shortcut { app: owner, .. } => *owner == app.code,
            Artefact::File { path } => path.starts_with(&legacy) || path.starts_with(&icon_prefix),
            Artefact::Tree { path } => path.starts_with(&legacy),
            Artefact::Dir { path } => *path == legacy,
            _ => false,
        })
        .cloned()
        .collect();
    // Entries and files first, then a folder (only removed when empty).
    for a in mine
        .iter()
        .filter(|a| !matches!(a, Artefact::Dir { .. }))
        .chain(mine.iter().filter(|a| matches!(a, Artefact::Dir { .. })))
    {
        match a {
            Artefact::Shortcut { path, .. } => remove_path(path),
            Artefact::File { path } => {
                let _ = std::fs::remove_file(path);
            }
            Artefact::Tree { path } => {
                let _ = std::fs::remove_dir_all(path);
            }
            Artefact::Dir { path } => {
                let _ = std::fs::remove_dir(path);
            }
            _ => {}
        }
        inventory.forget(a)?;
    }
    if !inventory.artefacts.iter().any(|a| matches!(a, Artefact::Shortcut { .. })) {
        for place in shared_places() {
            for a in inventory.artefacts.clone() {
                match &a {
                    Artefact::Dir { path } if *path == place => {
                        let _ = std::fs::remove_dir(path);
                    }
                    Artefact::File { path } if *path == place => {
                        let _ = std::fs::remove_file(path);
                    }
                    _ => continue,
                }
                inventory.forget(&a)?;
            }
        }
    }
    Ok(())
}

/// Downloads an icon as PNG into `name`.png (Linux desktop entries take PNG).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn icon_png(manifest_url: &str, name: &str, inventory: &mut Inventory) -> Option<PathBuf> {
    let png = fetch_icon(manifest_url).ok()?;
    let path = icons_dir().join(format!("{name}.png"));
    inventory.record(Artefact::File { path: path.to_string_lossy().into_owned() }).ok()?;
    std::fs::create_dir_all(icons_dir()).ok()?;
    std::fs::write(&path, png).ok()?;
    Some(path)
}

/// Linux: a desktop entry per kept item, gathered under a "Kynoko" submenu
/// where the desktop reads merged menus (KDE, Xfce, MATE, Cinnamon).
#[cfg(target_os = "linux")]
pub fn create(app: &App, settings: &Settings, lang: &str, inventory: &mut Inventory) -> std::io::Result<()> {
    crate::linux::ensure_menu(inventory)?;
    for item in chosen(app, settings, lang) {
        let png = icon_png(&item.manifest, &icon_name(app, &item), inventory);
        crate::linux::write_shortcut(app, &item, png.as_deref(), inventory)?;
    }
    crate::linux::refresh_menus();
    Ok(())
}

/// macOS: Applications > Kynoko > <app> > the app and its facades.
#[cfg(target_os = "macos")]
pub fn create(app: &App, settings: &Settings, lang: &str, inventory: &mut Inventory) -> std::io::Result<()> {
    crate::macos::create_shortcuts(app, &chosen(app, settings, lang), lang, inventory, |manifest| fetch_icon(manifest).ok())
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
pub fn create(_app: &App, _settings: &Settings, _lang: &str, _inventory: &mut Inventory) -> std::io::Result<()> {
    Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "shortcuts: not on this system"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(file_name("Crop & frame"), "Crop & frame");
        assert_eq!(file_name("a/b:c?"), "a b c");
        assert_eq!(file_name("..."), "Kynoko");
    }

    #[test]
    fn urls() {
        let m = "https://app.example/assets/manifests/convert.webmanifest";
        assert_eq!(resolve(m, "/assets/images/x.png"), "https://app.example/assets/images/x.png");
        assert_eq!(resolve(m, "x.png"), "https://app.example/assets/manifests/x.png");
        assert_eq!(resolve(m, "https://cdn.example/x.png"), "https://cdn.example/x.png");
    }

    #[test]
    fn kept_icons_are_checked_again() {
        use std::sync::{Arc, Mutex};
        use std::time::Duration;
        // A web app: its manifest, and an icon whose content and ETag change.
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let port = server.server_addr().to_ip().unwrap().port();
        let icon = Arc::new(Mutex::new((b"\x89PNG first".to_vec(), "\"v1\"".to_string())));
        let asked = Arc::new(Mutex::new(Vec::<String>::new()));
        let (serving, log) = (icon.clone(), asked.clone());
        std::thread::spawn(move || {
            for request in server.incoming_requests() {
                let (bytes, etag) = serving.lock().unwrap().clone();
                let conditional = request.headers().iter().find(|h| h.field.equiv("If-None-Match")).map(|h| h.value.to_string());
                log.lock().unwrap().push(format!("{} {}", request.url(), conditional.clone().unwrap_or_default()));
                let manifest = request.url().ends_with(".webmanifest");
                let response = if manifest {
                    tiny_http::Response::from_data(br#"{"icons":[{"src":"/icon.png","sizes":"512x512","type":"image/png"}]}"#.to_vec())
                } else if conditional.as_deref() == Some(etag.as_str()) {
                    tiny_http::Response::from_data(Vec::new()).with_status_code(304)
                } else {
                    tiny_http::Response::from_data(bytes).with_header(tiny_http::Header::from_bytes("ETag", etag.as_bytes()).unwrap())
                };
                let _ = request.respond(response);
            }
        });
        let manifest = format!("http://127.0.0.1:{port}/manifest.webmanifest");
        let dir = std::env::temp_dir().join(format!("kynoko-icons-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("App.png");
        let hour = Duration::from_secs(3600);

        assert_eq!(kept_icon(&manifest, &path, hour).unwrap(), b"\x89PNG first");
        let requests = asked.lock().unwrap().len();
        // Checked less than an hour ago: nothing asked.
        assert_eq!(kept_icon(&manifest, &path, hour).unwrap(), b"\x89PNG first");
        assert_eq!(asked.lock().unwrap().len(), requests);
        // Due: asked with its ETag, unchanged (304), kept.
        assert_eq!(kept_icon(&manifest, &path, Duration::ZERO).unwrap(), b"\x89PNG first");
        assert_eq!(asked.lock().unwrap().last().unwrap(), "/icon.png \"v1\"");
        // The app changed its icon: the new one replaces it.
        *icon.lock().unwrap() = (b"\x89PNG second".to_vec(), "\"v2\"".to_string());
        assert_eq!(kept_icon(&manifest, &path, Duration::ZERO).unwrap(), b"\x89PNG second");
        assert_eq!(std::fs::read(&path).unwrap(), b"\x89PNG second");
        // Something else than a PNG (a web app's fallback page): kept.
        *icon.lock().unwrap() = (b"<!doctype html>".to_vec(), "\"v3\"".to_string());
        assert_eq!(kept_icon(&manifest, &path, Duration::ZERO).unwrap(), b"\x89PNG second");
        // Offline: kept.
        assert_eq!(kept_icon("http://127.0.0.1:9/manifest.webmanifest", &path, Duration::ZERO).unwrap(), b"\x89PNG second");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn solid(size: u32, rgba: [u8; 4]) -> Vec<u8> {
        use image::ImageEncoder;
        let img = image::RgbaImage::from_pixel(size, size, image::Rgba(rgba));
        let mut png = Vec::new();
        image::codecs::png::PngEncoder::new(&mut png)
            .write_image(img.as_raw(), size, size, image::ExtendedColorType::Rgba8)
            .unwrap();
        png
    }

    #[test]
    fn file_icons_wear_the_facet_and_the_mark() {
        let blue = [40, 110, 230, 255];
        let violet = [90, 40, 160, 255];
        let read = |png: &[u8]| image::load_from_memory(png).unwrap().to_rgba8();
        let near = |p: &image::Rgba<u8>, rgba: [u8; 4]| p.0.iter().zip(rgba).all(|(a, b)| a.abs_diff(b) <= 2);
        let alone = read(&file_icon(&solid(512, blue), None).unwrap());
        assert_eq!(alone.dimensions(), (256, 256));
        // The facet: top-left and bottom-right ample (64 px), the other two sharp (15 px).
        assert_eq!(alone.get_pixel(6, 6)[3], 0);
        assert_eq!(alone.get_pixel(250, 250)[3], 0);
        assert!(near(alone.get_pixel(30, 30), blue), "an ample corner's arc passes beyond (30, 30)");
        assert_eq!(alone.get_pixel(254, 1)[3], 0);
        assert_eq!(alone.get_pixel(1, 254)[3], 0);
        assert!(near(alone.get_pixel(250, 10), blue), "a sharp corner is small");
        assert!(near(alone.get_pixel(240, 16), blue));
        assert!(near(alone.get_pixel(128, 1), blue));
        assert!(near(alone.get_pixel(128, 128), blue));

        let marked = read(&file_icon(&solid(512, blue), Some(&solid(64, violet))).unwrap());
        // The mark in the bottom-right corner (92 px from 164), the icon elsewhere...
        assert!(near(marked.get_pixel(220, 220), violet));
        assert!(near(marked.get_pixel(100, 100), blue));
        // ...parted by a transparent ring (9 px), left of and above the mark.
        assert_eq!(marked.get_pixel(160, 215)[3], 0);
        assert_eq!(marked.get_pixel(215, 160)[3], 0);
        assert!(near(marked.get_pixel(150, 215), blue));
    }

    /// A look at a real one: KYNOKO_FILE_ICON_SAMPLE="<icon.png>;<mark.png>;<out.png>".
    #[test]
    #[ignore]
    fn file_icon_sample() {
        let spec = std::env::var("KYNOKO_FILE_ICON_SAMPLE").unwrap();
        let parts: Vec<&str> = spec.split(';').collect();
        let mark = std::fs::read(parts[1]).ok();
        let png = file_icon(&std::fs::read(parts[0]).unwrap(), mark.as_deref()).unwrap();
        std::fs::write(parts[2], &png).unwrap();
        // And as a folder shows it: 96, 48, 32 and 16 px, on light and on dark.
        let icon = image::load_from_memory(&png).unwrap();
        let mut sheet = image::RgbaImage::new(250, 220);
        for (row, bg) in [[243u8, 243, 243, 255], [32, 32, 32, 255]].iter().enumerate() {
            for y in 0..110 {
                for x in 0..250 {
                    sheet.put_pixel(x, row as u32 * 110 + y, image::Rgba(*bg));
                }
            }
            let mut x = 5;
            for size in [96u32, 48, 32, 16] {
                let small = icon.resize_exact(size, size, image::imageops::FilterType::Lanczos3).to_rgba8();
                image::imageops::overlay(&mut sheet, &small, x, row as i64 * 110 + 7);
                x += size as i64 + 12;
            }
        }
        sheet.save(parts[2].replace(".png", "-sheet.png")).unwrap();
    }

    #[test]
    fn ico() {
        let mut png = Vec::new();
        {
            use image::ImageEncoder;
            let img = image::RgbaImage::from_pixel(64, 64, image::Rgba([99, 102, 241, 255]));
            image::codecs::png::PngEncoder::new(&mut png)
                .write_image(img.as_raw(), 64, 64, image::ExtendedColorType::Rgba8)
                .unwrap();
        }
        let ico = png_to_ico(&png).unwrap();
        assert_eq!(&ico[0..6], &[0, 0, 1, 0, 4, 0]);
        // First frame: 256 px (written as 0), PNG data at the announced offset.
        assert_eq!(ico[6], 0);
        let offset = u32::from_le_bytes([ico[18], ico[19], ico[20], ico[21]]) as usize;
        assert_eq!(&ico[offset..offset + 4], &[0x89, b'P', b'N', b'G']);
    }
}
