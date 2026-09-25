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

use crate::catalogue::App;
use crate::settings::{self, Artefact, Inventory, Settings};

/// Where downloaded icons live (removed with the rest by the cleanup).
fn icons_dir() -> PathBuf {
    settings::dir().join("icons")
}

/// A file name Windows, macOS and Linux all accept.
fn file_name(name: &str) -> String {
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
fn fetch_icon(manifest_url: &str) -> Result<Vec<u8>, String> {
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

#[cfg(windows)]
fn start_menu() -> PathBuf {
    dirs::data_dir().unwrap_or_default().join(r"Microsoft\Windows\Start Menu\Programs")
}

#[cfg(windows)]
fn folder_of(app: &App, lang: &str) -> PathBuf {
    start_menu().join(file_name(&app.name(lang)))
}

/// Creates the app's folder in the Start menu: the app, then its facades.
#[cfg(windows)]
pub fn create(app: &App, settings: &Settings, lang: &str, inventory: &mut Inventory) -> std::io::Result<()> {
    let exe = std::env::current_exe()?;
    let base = settings.url_of(app);
    let base = base.trim_end_matches('/');
    let folder = folder_of(app, lang);
    inventory.record(Artefact::Dir { path: folder.to_string_lossy().into_owned() })?;
    std::fs::create_dir_all(&folder)?;

    let app_icon = icon_file(&format!("{base}/manifest.webmanifest"), &app.code, inventory);
    link(&exe, &folder.join(format!("{}.lnk", file_name(&app.name(lang)))), &app.code, app_icon.as_deref(), inventory)?;

    for facade in &app.facades {
        let name = facade.names.get(lang).or_else(|| facade.names.get("en")).cloned().unwrap_or_else(|| facade.path.clone());
        let slug = facade.path.rsplit('/').next().unwrap_or(&facade.path);
        let icon = icon_file(&format!("{base}/assets/manifests/{slug}.webmanifest"), &format!("{}-{slug}", app.code), inventory);
        let target = format!("{}/{}", app.code, facade.path);
        link(&exe, &folder.join(format!("{}.lnk", file_name(&name))), &target, icon.as_deref(), inventory)?;
    }
    Ok(())
}

#[cfg(windows)]
fn link(exe: &Path, at: &Path, target: &str, icon: Option<&Path>, inventory: &mut Inventory) -> std::io::Result<()> {
    inventory.record(Artefact::File { path: at.to_string_lossy().into_owned() })?;
    let mut sl = mslnk::ShellLink::new(exe).map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
    sl.set_arguments(Some(format!("launch {target}")));
    if let Some(icon) = icon {
        sl.set_icon_location(Some(icon.to_string_lossy().into_owned()));
    }
    sl.create_lnk(at).map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
}

/// Removes the app's shortcuts, its icons, and its folder once empty.
pub fn remove(app: &App, lang: &str, inventory: &mut Inventory) -> std::io::Result<()> {
    #[cfg(windows)]
    let folder = folder_of(app, lang).to_string_lossy().into_owned();
    #[cfg(not(windows))]
    let folder = { let _ = lang; String::from("\u{0}") };
    let icon_prefix = icons_dir().join(&app.code).to_string_lossy().into_owned();
    let mine: Vec<Artefact> = inventory
        .artefacts
        .iter()
        .filter(|a| match a {
            Artefact::File { path } => path.starts_with(&folder) || path.starts_with(&icon_prefix),
            Artefact::Dir { path } => *path == folder,
            _ => false,
        })
        .cloned()
        .collect();
    // Files first, then the folder (only removed when empty).
    for a in mine.iter().filter(|a| matches!(a, Artefact::File { .. })).chain(mine.iter().filter(|a| matches!(a, Artefact::Dir { .. }))) {
        match a {
            Artefact::File { path } => {
                let _ = std::fs::remove_file(path);
            }
            Artefact::Dir { path } => {
                let _ = std::fs::remove_dir(path);
            }
            _ => {}
        }
        inventory.forget(a)?;
    }
    Ok(())
}

#[cfg(not(windows))]
pub fn create(_app: &App, _settings: &Settings, _lang: &str, _inventory: &mut Inventory) -> std::io::Result<()> {
    Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "shortcuts: Windows only in this build"))
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
