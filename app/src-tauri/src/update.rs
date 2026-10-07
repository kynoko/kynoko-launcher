//! "Download and install": the newer Kynoko Launcher the window announces, downloaded
//! from its GitHub release and installed, when the user asks for it. Never
//! automatic (docs/SPEC.md, section 13).
//!
//! The trust is the release's (section 12): the file comes from this
//! repository's releases only, and its SHA-256 must be the one GitHub
//! computed when it was uploaded (the API's `digest`; the release's
//! SHA256SUMS.txt when the API gives none). The window only ever asks for
//! "the latest": the file's address and path never come from a page.

use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use serde::Serialize;
use sha2::{Digest, Sha256};

const RELEASES_API: &str = "https://api.github.com/repos/kynoko/kynoko-launcher/releases/latest";
/// The page of the latest release ("Download": the user takes it from there).
pub const RELEASES_PAGE: &str = "https://github.com/kynoko/kynoko-launcher/releases/latest";
/// Where every file of a release is served from; nothing else is fetched.
const DOWNLOADS: &str = "https://github.com/kynoko/kynoko-launcher/releases/download/";
const SUMS: &str = "SHA256SUMS.txt";
/// The AppImage is about 80 MB: anything far bigger is not a release file.
const MAX_SIZE: u64 = 512 * 1024 * 1024;

/// How this system takes a new version.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Mode {
    /// Installed in place: the launcher closes first, and the new one starts
    /// once installed (Windows: the installer in update mode; the AppImage).
    Restart,
    /// Handed to the system, which finishes with the user: the disk image
    /// opens in the Finder (macOS), the package in the software centre (a
    /// .deb or .rpm install).
    Handoff,
}

/// The release file this system installs, and how. None when there is none
/// (an architecture without a build): the window then offers "Download" only.
#[cfg(windows)]
pub fn wanted() -> Option<(&'static str, Mode)> {
    // Windows on ARM runs the x64 build.
    Some(("kynoko-launcher-windows-x64-setup.exe", Mode::Restart))
}

#[cfg(target_os = "macos")]
pub fn wanted() -> Option<(&'static str, Mode)> {
    Some(("kynoko-launcher-macos-universal.dmg", Mode::Handoff))
}

#[cfg(all(unix, not(target_os = "macos")))]
pub fn wanted() -> Option<(&'static str, Mode)> {
    if !cfg!(target_arch = "x86_64") {
        return None;
    }
    if std::env::var_os("APPIMAGE").is_some() {
        return Some(("kynoko-launcher-linux-x86_64.AppImage", Mode::Restart));
    }
    // Installed from a package: the same kind of package, for the system's
    // own installer.
    let packaged = std::env::current_exe().map(|p| p.starts_with("/usr")).unwrap_or(false);
    if packaged && Path::new("/var/lib/dpkg/info/kynoko-launcher.list").exists() {
        return Some(("kynoko-launcher-linux-amd64.deb", Mode::Handoff));
    }
    if packaged && Path::new("/usr/bin/rpm").exists() {
        return Some(("kynoko-launcher-linux-x86_64.rpm", Mode::Handoff));
    }
    None
}

/// One file of the release.
pub struct Asset {
    name: String,
    url: String,
    size: u64,
    /// Lowercase hex, when GitHub gives it.
    sha256: Option<String>,
}

pub struct Release {
    /// "0.2.25"
    pub version: String,
    /// The file this system installs, when the release has it.
    pub asset: Option<Asset>,
    sums: Option<String>,
}

fn agent() -> ureq::Agent {
    // No overall limit: a slow connection still finishes. Each read waits a
    // while, a connection does not.
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(15))
        .timeout_read(Duration::from_secs(30))
        .build()
}

/// The latest published release, from GitHub's public API.
pub fn latest() -> Result<Release, String> {
    let release: serde_json::Value = agent()
        .get(RELEASES_API)
        .set("User-Agent", "kynoko-launcher")
        .set("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| e.to_string())?
        .into_json()
        .map_err(|e| e.to_string())?;
    let version = release["tag_name"].as_str().map(|t| t.trim_start_matches('v').to_string()).ok_or("no tag")?;
    let assets = release["assets"].as_array().cloned().unwrap_or_default();
    let named = |name: &str| assets.iter().find(|a| a["name"].as_str() == Some(name)).and_then(asset_of);
    Ok(Release {
        version,
        asset: wanted().and_then(|(name, _)| named(name)),
        sums: named(SUMS).map(|a| a.url),
    })
}

/// A release file as the API lists it; None when it is not served from this
/// repository's releases.
fn asset_of(a: &serde_json::Value) -> Option<Asset> {
    let url = a["browser_download_url"].as_str()?;
    if !url.starts_with(DOWNLOADS) {
        return None;
    }
    Some(Asset {
        name: a["name"].as_str()?.to_string(),
        url: url.to_string(),
        size: a["size"].as_u64()?,
        sha256: a["digest"].as_str().and_then(|d| d.strip_prefix("sha256:")).and_then(hex64),
    })
}

/// 64 hex digits, lowercased.
fn hex64(s: &str) -> Option<String> {
    let s = s.trim().to_ascii_lowercase();
    (s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())).then_some(s)
}

/// The SHA-256 SHA256SUMS.txt gives `name` (`<hex>  <name>` lines).
fn sum_in(sums: &str, name: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let (hash, file) = line.split_once(char::is_whitespace)?;
        (file.trim().trim_start_matches('*') == name).then(|| hex64(hash)).flatten()
    })
}

/// Where the installer is put: the user's cache, never a roaming profile.
/// An isolated run (end-to-end tests) keeps it in its own folder.
pub fn dir() -> PathBuf {
    if crate::settings::isolated() {
        crate::settings::dir().join("update")
    } else {
        dirs::cache_dir().unwrap_or_else(std::env::temp_dir).join("Kynoko Launcher").join("update")
    }
}

/// Removes what an earlier install left behind (at start). A file still in
/// use (the installer that started this launcher) stays until next time.
pub fn clean() {
    let _ = fs::remove_dir_all(dir());
}

/// Downloads the release's file for this system into `dir`, checked against
/// the SHA-256 GitHub gives for it. `progress(received, total)` is called as
/// it arrives. The checked file's path.
pub fn download(release: &Release, dir: &Path, mut progress: impl FnMut(u64, u64)) -> Result<PathBuf, String> {
    let asset = release.asset.as_ref().ok_or("no installer for this system in the release")?;
    if asset.size == 0 || asset.size > MAX_SIZE {
        return Err(format!("unexpected size: {} bytes", asset.size));
    }
    let expected = match &asset.sha256 {
        Some(hash) => hash.clone(),
        None => {
            let url = release.sums.as_deref().ok_or("no checksum for the installer")?;
            let sums = agent().get(url).set("User-Agent", "kynoko-launcher").call().map_err(|e| e.to_string())?.into_string().map_err(|e| e.to_string())?;
            sum_in(&sums, &asset.name).ok_or("no checksum for the installer")?
        }
    };
    let _ = fs::remove_dir_all(dir);
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let part = dir.join(format!("{}.part", asset.name));
    let response = agent().get(&asset.url).set("User-Agent", "kynoko-launcher").call().map_err(|e| e.to_string())?;
    let mut body = response.into_reader().take(asset.size + 1);
    let mut out = fs::File::create(&part).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    let mut received = 0u64;
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = body.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        received += n as u64;
        if received > asset.size {
            return Err("the file is bigger than announced".into());
        }
        hasher.update(&buf[..n]);
        out.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        progress(received, asset.size);
    }
    out.sync_all().map_err(|e| e.to_string())?;
    drop(out);
    if received != asset.size {
        return Err(format!("incomplete download: {received} of {} bytes", asset.size));
    }
    let got = format!("{:x}", hasher.finalize());
    if got != expected {
        let _ = fs::remove_file(&part);
        return Err("the file does not match the release's checksum".into());
    }
    let path = dir.join(&asset.name);
    fs::rename(&part, &path).map_err(|e| e.to_string())?;
    Ok(path)
}

/// The installer's arguments on Windows: passive (a progress bar, no
/// question), update (the version in place is not uninstalled first, so the
/// user's settings stay), and the new launcher started at the end (/R). It
/// stops a launcher still running.
pub const WINDOWS_ARGS: [&str; 3] = ["/P", "/UPDATE", "/R"];

/// Starts the install of `path` (see Mode). For Mode::Restart, the caller
/// quits right after.
#[cfg(windows)]
pub fn install(path: &Path) -> io::Result<()> {
    Command::new(path).args(WINDOWS_ARGS).spawn().map(|_| ())
}

#[cfg(target_os = "macos")]
pub fn install(path: &Path) -> io::Result<()> {
    Command::new("open").arg(path).spawn().map(|_| ())
}

#[cfg(all(unix, not(target_os = "macos")))]
pub fn install(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let Some(target) = std::env::var_os("APPIMAGE").map(PathBuf::from) else {
        return Command::new("xdg-open").arg(path).spawn().map(|_| ());
    };
    // The AppImage replaced in place by a rename (the running one keeps the
    // file it has open), then started once this launcher has quit: a second
    // launcher would only hand its start over to this one.
    let name = target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let next = target.with_file_name(format!(".{name}.new"));
    fs::copy(path, &next)?;
    fs::set_permissions(&next, fs::Permissions::from_mode(0o755))?;
    fs::rename(&next, &target)?;
    Command::new("sh")
        .args(["-c", "while kill -0 \"$1\" 2>/dev/null; do sleep 0.2; done; exec \"$0\""])
        .arg(&target)
        .arg(std::process::id().to_string())
        .spawn()
        .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksums() {
        let h = "19f70ee85d393df4d4dd7ae058e77f71094128d980c0b8d712c86991d3ceaf6a";
        assert_eq!(hex64(&h.to_uppercase()), Some(h.to_string()));
        assert_eq!(hex64("abc"), None);
        assert_eq!(hex64(&"z".repeat(64)), None);
        let sums = format!("{h}  kynoko-launcher-windows-x64-setup.exe\n{}  kynoko-launcher-macos-universal.dmg\n", "0".repeat(64));
        assert_eq!(sum_in(&sums, "kynoko-launcher-windows-x64-setup.exe"), Some(h.to_string()));
        assert_eq!(sum_in(&sums, "kynoko-launcher-macos-universal.dmg"), Some("0".repeat(64)));
        assert_eq!(sum_in(&sums, "other.exe"), None);
        assert_eq!(sum_in(&format!("{h} *kynoko-launcher-windows-x64-setup.exe"), "kynoko-launcher-windows-x64-setup.exe"), Some(h.to_string()));
    }

    #[test]
    fn only_this_repository() {
        let ok = serde_json::json!({ "name": "a.exe", "size": 3, "digest": "sha256:".to_string() + &"a".repeat(64),
            "browser_download_url": "https://github.com/kynoko/kynoko-launcher/releases/download/v0.2.25/a.exe" });
        let asset = asset_of(&ok).expect("accepted");
        assert_eq!(asset.sha256.as_deref(), Some("a".repeat(64).as_str()));
        for url in [
            "https://github.com/someone/kynoko-launcher/releases/download/v0.2.25/a.exe",
            "http://github.com/kynoko/kynoko-launcher/releases/download/v0.2.25/a.exe",
            "https://github.com.evil.example/kynoko/kynoko-launcher/releases/download/a.exe",
        ] {
            let mut bad = ok.clone();
            bad["browser_download_url"] = url.into();
            assert!(asset_of(&bad).is_none(), "{url}");
        }
        let mut no_digest = ok.clone();
        no_digest["digest"] = serde_json::Value::Null;
        assert!(asset_of(&no_digest).expect("accepted").sha256.is_none());
    }

    #[test]
    fn this_system_has_a_file() {
        if cfg!(windows) || cfg!(target_os = "macos") {
            assert!(wanted().is_some());
        }
    }
}
