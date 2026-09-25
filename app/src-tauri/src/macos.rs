//! macOS integration (docs/SPEC.md, sections 5 to 7).
//!
//! - Browsers: the applications in /Applications and ~/Applications whose
//!   Info.plist declares the https scheme; started through `open`.
//! - Associations: macOS reads document types from the launcher's own
//!   Info.plist, which declares every Kynoko type as an "Alternate" handler
//!   (generated from the bundled catalogue, tools/mac-info-plist.mjs).
//!   Turning an app on makes the launcher the DEFAULT handler of its types
//!   through LaunchServices, the previous handler recorded and put back on
//!   removal.
//! - Files and kynoko-launcher:// links arrive as Apple Events (lib.rs,
//!   RunEvent::Opened), not as arguments.
//! - Shortcuts: tiny application bundles in ~/Applications/Kynoko/<App>/,
//!   created locally (no quarantine), each running the launcher.

use std::path::{Path, PathBuf};

use crate::browsers::{engine_of, Browser, Engine, Profile};
use crate::catalogue::App;
use crate::settings::{Artefact, Inventory, Settings};

/// The launcher's bundle identifier (tauri.conf.json `identifier`).
pub const BUNDLE_ID: &str = "com.kynoko.launcher";

fn home() -> PathBuf {
    dirs::home_dir().unwrap_or_default()
}

/* ---------------------------------------------------------------- browsers */

fn app_bundles() -> Vec<PathBuf> {
    let mut out = Vec::new();
    for root in [PathBuf::from("/Applications"), home().join("Applications")] {
        let Ok(entries) = std::fs::read_dir(&root) else { continue };
        for e in entries.flatten() {
            let p = e.path();
            if p.extension().map(|x| x == "app").unwrap_or(false) {
                out.push(p);
            } else if p.is_dir() {
                // One level of folders (/Applications/Utilities, vendor folders).
                if let Ok(inner) = std::fs::read_dir(&p) {
                    out.extend(inner.flatten().map(|i| i.path()).filter(|i| i.extension().map(|x| x == "app").unwrap_or(false)));
                }
            }
        }
    }
    out
}

/// Engine from the bundle identifier (a Mac executable name says little).
fn engine_of_bundle(id: &str) -> Engine {
    let id = id.to_ascii_lowercase();
    if id.starts_with("com.apple.safari") {
        Engine::Webkit
    } else if id.starts_with("org.mozilla.") || id.contains("librewolf") || id.contains("waterfox") || id.contains("zen") || id.contains("floorp") {
        Engine::Gecko
    } else if ["com.google.chrome", "com.microsoft.edgemac", "com.brave.browser", "org.chromium.chromium", "com.operasoftware.", "com.vivaldi.", "company.thebrowser."]
        .iter()
        .any(|p| id.starts_with(p))
    {
        Engine::Chromium
    } else {
        engine_of(&id)
    }
}

pub fn browsers() -> Vec<Browser> {
    let mut out: Vec<Browser> = Vec::new();
    for bundle in app_bundles() {
        let Ok(value) = plist::Value::from_file(bundle.join("Contents/Info.plist")) else { continue };
        let Some(dict) = value.as_dictionary() else { continue };
        let schemes = dict
            .get("CFBundleURLTypes")
            .and_then(|v| v.as_array())
            .map(|types| {
                types.iter().filter_map(|t| t.as_dictionary()).any(|t| {
                    t.get("CFBundleURLSchemes")
                        .and_then(|s| s.as_array())
                        .map(|s| s.iter().any(|x| x.as_string() == Some("https")))
                        .unwrap_or(false)
                })
            })
            .unwrap_or(false);
        let id = dict.get("CFBundleIdentifier").and_then(|v| v.as_string()).unwrap_or_default().to_string();
        if !schemes || id.is_empty() || id == BUNDLE_ID || out.iter().any(|b| b.id == id) {
            continue;
        }
        let name = dict
            .get("CFBundleDisplayName")
            .or_else(|| dict.get("CFBundleName"))
            .and_then(|v| v.as_string())
            .map(str::to_string)
            .unwrap_or_else(|| bundle.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default());
        let engine = engine_of_bundle(&id);
        let app = bundle.to_string_lossy().into_owned();
        // `open -n` starts the app with our arguments even when it runs; a
        // WebKit browser only needs the URL handed to it.
        let command = match engine {
            Engine::Webkit => vec!["open".to_string(), "-a".to_string(), app.clone()],
            _ => vec!["open".to_string(), "-na".to_string(), app.clone(), "--args".to_string()],
        };
        let profiles = profiles(&engine, &id);
        out.push(Browser { id, name, exe: app, engine, profiles, command });
    }
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out
}

fn profiles(engine: &Engine, id: &str) -> Vec<Profile> {
    let support = home().join("Library/Application Support");
    let id = id.to_ascii_lowercase();
    match engine {
        Engine::Chromium => {
            let dir = if id.starts_with("com.google.chrome.canary") {
                "Google/Chrome Canary"
            } else if id.starts_with("com.google.chrome.beta") {
                "Google/Chrome Beta"
            } else if id.starts_with("com.google.chrome") {
                "Google/Chrome"
            } else if id.starts_with("com.microsoft.edgemac") {
                "Microsoft Edge"
            } else if id.starts_with("com.brave.browser") {
                "BraveSoftware/Brave-Browser"
            } else if id.starts_with("org.chromium.chromium") {
                "Chromium"
            } else if id.starts_with("com.vivaldi.") {
                "Vivaldi"
            } else {
                return Vec::new();
            };
            std::fs::read_to_string(support.join(dir).join("Local State"))
                .ok()
                .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
                .map(|s| crate::browsers::parse_local_state(&s))
                .unwrap_or_default()
        }
        Engine::Gecko if id.starts_with("org.mozilla.") => {
            let profiles = std::fs::read_to_string(support.join("Firefox/profiles.ini")).unwrap_or_default();
            crate::browsers::parse_profiles_ini(&profiles, "", "")
        }
        _ => Vec::new(),
    }
}

/* ------------------------------------------------------------ LaunchServices */

mod ls {
    use core_foundation::base::TCFType;
    use core_foundation::string::{CFString, CFStringRef};

    #[link(name = "CoreServices", kind = "framework")]
    extern "C" {
        static kUTTagClassFilenameExtension: CFStringRef;
        fn UTTypeCreatePreferredIdentifierForTag(tag_class: CFStringRef, tag: CFStringRef, conforming_to: CFStringRef) -> CFStringRef;
        fn LSCopyDefaultRoleHandlerForContentType(content_type: CFStringRef, role: u32) -> CFStringRef;
        fn LSSetDefaultRoleHandlerForContentType(content_type: CFStringRef, role: u32, handler: CFStringRef) -> i32;
    }

    const ROLES_ALL: u32 = 0xFFFF_FFFF;

    /// The uniform type identifier of a file extension (`docx` ->
    /// `org.openxmlformats.wordprocessingml.document`, or a dynamic one).
    pub fn uti_for(ext: &str) -> Option<String> {
        let tag = CFString::new(ext);
        // SAFETY: documented CoreServices call; the result follows the Create rule.
        let raw = unsafe { UTTypeCreatePreferredIdentifierForTag(kUTTagClassFilenameExtension, tag.as_concrete_TypeRef(), std::ptr::null()) };
        (!raw.is_null()).then(|| unsafe { CFString::wrap_under_create_rule(raw) }.to_string())
    }

    pub fn default_handler(uti: &str) -> Option<String> {
        let t = CFString::new(uti);
        // SAFETY: as above (Copy rule).
        let raw = unsafe { LSCopyDefaultRoleHandlerForContentType(t.as_concrete_TypeRef(), ROLES_ALL) };
        (!raw.is_null()).then(|| unsafe { CFString::wrap_under_create_rule(raw) }.to_string())
    }

    pub fn set_default_handler(uti: &str, bundle: &str) -> bool {
        let (t, b) = (CFString::new(uti), CFString::new(bundle));
        // SAFETY: both strings outlive the call.
        unsafe { LSSetDefaultRoleHandlerForContentType(t.as_concrete_TypeRef(), ROLES_ALL, b.as_concrete_TypeRef()) == 0 }
    }
}

/// Marks which app a recorded default belongs to (`com.kynoko.launcher|Office`).
fn owner(app: &App) -> String {
    format!("{BUNDLE_ID}|{}", app.code)
}

pub fn register(app: &App, inventory: &mut Inventory) -> std::io::Result<()> {
    for ext in app.extensions() {
        let Some(uti) = ls::uti_for(&ext) else { continue };
        let previous = ls::default_handler(&uti);
        if previous.as_deref() != Some(BUNDLE_ID) {
            inventory.record(Artefact::MimeDefault { mime: uti.clone(), desktop: owner(app), previous })?;
        }
        ls::set_default_handler(&uti, BUNDLE_ID);
    }
    Ok(())
}

/// Puts back what was the default before us, unless the user changed it since.
pub fn restore_default(uti: &str, previous: Option<&str>) {
    if ls::default_handler(uti).as_deref() == Some(BUNDLE_ID) {
        if let Some(p) = previous {
            ls::set_default_handler(uti, p);
        }
    }
}

pub fn unregister(app: &App, inventory: &mut Inventory) -> std::io::Result<()> {
    let mine = owner(app);
    for a in inventory.artefacts.clone() {
        if let Artefact::MimeDefault { mime, desktop, previous } = &a {
            if *desktop == mine {
                restore_default(mime, previous.as_deref());
                inventory.forget(&a)?;
            }
        }
    }
    Ok(())
}

/* --------------------------------------------------------------- shortcuts */

fn folder_of(app: &App, lang: &str) -> PathBuf {
    home().join("Applications/Kynoko").join(crate::shortcuts::file_name(&app.name(lang)))
}

pub fn shortcut_folder(app: &App, lang: &str) -> String {
    folder_of(app, lang).to_string_lossy().into_owned()
}

/// An .icns holding one PNG frame (macOS reads PNG inside icns since 10.7).
pub fn png_to_icns(png: &[u8]) -> Vec<u8> {
    let size = image::load_from_memory_with_format(png, image::ImageFormat::Png).map(|i| i.width()).unwrap_or(512);
    let kind: &[u8; 4] = match size {
        s if s >= 1024 => b"ic10",
        s if s >= 512 => b"ic09",
        s if s >= 256 => b"ic08",
        _ => b"ic07",
    };
    let entry_len = 8 + png.len() as u32;
    let mut out = Vec::with_capacity(16 + png.len());
    out.extend_from_slice(b"icns");
    out.extend_from_slice(&(8 + entry_len).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(&entry_len.to_be_bytes());
    out.extend_from_slice(png);
    out
}

/// One shortcut: `<name>.app` running `kynoko-launcher launch <target>`.
fn bundle(at: &Path, name: &str, target: &str, id_suffix: &str, icon: Option<&[u8]>, inventory: &mut Inventory) -> std::io::Result<()> {
    inventory.record(Artefact::Tree { path: at.to_string_lossy().into_owned() })?;
    let contents = at.join("Contents");
    std::fs::create_dir_all(contents.join("MacOS"))?;
    std::fs::create_dir_all(contents.join("Resources"))?;
    let exe = std::env::current_exe()?.to_string_lossy().into_owned();
    let script = format!("#!/bin/sh\nexec '{}' launch '{}'\n", exe.replace('\'', "'\\''"), target.replace('\'', "'\\''"));
    let run = contents.join("MacOS/launch");
    std::fs::write(&run, script)?;
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&run, std::fs::Permissions::from_mode(0o755))?;
    }
    let mut info = plist::Dictionary::new();
    info.insert("CFBundleName".into(), name.into());
    info.insert("CFBundleDisplayName".into(), name.into());
    info.insert("CFBundleExecutable".into(), "launch".into());
    info.insert("CFBundleIdentifier".into(), format!("{BUNDLE_ID}.shortcut.{id_suffix}").into());
    info.insert("CFBundlePackageType".into(), "APPL".into());
    info.insert("LSUIElement".into(), true.into());
    if let Some(png) = icon {
        std::fs::write(contents.join("Resources/icon.icns"), png_to_icns(png))?;
        info.insert("CFBundleIconFile".into(), "icon".into());
    }
    plist::Value::Dictionary(info)
        .to_file_xml(contents.join("Info.plist"))
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
}

pub fn create_shortcuts(app: &App, settings: &Settings, lang: &str, inventory: &mut Inventory, icon: impl Fn(&str) -> Option<Vec<u8>>) -> std::io::Result<()> {
    let base = settings.url_of(app);
    let base = base.trim_end_matches('/');
    let folder = folder_of(app, lang);
    inventory.record(Artefact::Dir { path: folder.to_string_lossy().into_owned() })?;
    std::fs::create_dir_all(&folder)?;
    let name = app.name(lang);
    let app_icon = icon(&format!("{base}/manifest.webmanifest"));
    bundle(&folder.join(format!("{}.app", crate::shortcuts::file_name(&name))), &name, &app.code, &app.code.to_lowercase(), app_icon.as_deref(), inventory)?;
    for f in app.facades.iter().filter(|f| f.listed) {
        let fname = f.names.get(lang).or_else(|| f.names.get("en")).cloned().unwrap_or_else(|| f.path.clone());
        let slug = f.path.rsplit('/').next().unwrap_or(&f.path);
        let fi = icon(&format!("{base}/assets/manifests/{slug}.webmanifest"));
        let suffix = format!("{}.{}", app.code.to_lowercase(), slug.replace(|c: char| !c.is_ascii_alphanumeric(), "-"));
        bundle(
            &folder.join(format!("{}.app", crate::shortcuts::file_name(&fname))),
            &fname,
            &format!("{}/{}", app.code, f.path),
            &suffix,
            fi.as_deref(),
            inventory,
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icns_frame() {
        let icns = png_to_icns(&[0x89, b'P', b'N', b'G', 0, 0]);
        assert_eq!(&icns[0..4], b"icns");
        assert_eq!(u32::from_be_bytes([icns[4], icns[5], icns[6], icns[7]]) as usize, icns.len());
        assert_eq!(&icns[16..20], &[0x89, b'P', b'N', b'G']);
    }

    #[test]
    fn engines() {
        assert_eq!(engine_of_bundle("com.apple.Safari"), Engine::Webkit);
        assert_eq!(engine_of_bundle("com.apple.SafariTechnologyPreview"), Engine::Webkit);
        assert_eq!(engine_of_bundle("org.mozilla.nightly"), Engine::Gecko);
        assert_eq!(engine_of_bundle("com.google.Chrome.canary"), Engine::Chromium);
        assert_eq!(engine_of_bundle("com.microsoft.edgemac.Beta"), Engine::Chromium);
    }
}
