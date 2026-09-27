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

/// The largest PNG "any" icon a web manifest declares, downloaded.
pub(crate) fn fetch_icon(manifest_url: &str) -> Result<Vec<u8>, String> {
    let agent = ureq::AgentBuilder::new().timeout(std::time::Duration::from_secs(10)).build();
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
    let mut bytes = Vec::new();
    agent
        .get(&resolve(manifest_url, &best))
        .call()
        .map_err(|e| e.to_string())?
        .into_reader()
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    Ok(bytes)
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

/// The icon (`ico` or `png`) of the files `facade` opens; None when it
/// cannot be had, and the type then wears the launcher's own.
#[cfg_attr(not(any(windows, target_os = "linux")), allow(dead_code))]
pub(crate) fn type_icon(app: &App, facade: &Facade, settings: &Settings, format: &str, inventory: &mut Inventory) -> Option<PathBuf> {
    let path = type_icon_path(app, facade, format);
    let recorded = Artefact::File { path: path.to_string_lossy().into_owned() };
    if path.is_file() && inventory.artefacts.contains(&recorded) {
        return Some(path);
    }
    let png = fetch_icon(&facade_manifest(&settings.url_of(app), facade)).ok()?;
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
    sl.create_lnk(at).map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
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
