//! Opening an app's page in the chosen browser (docs/SPEC.md, section 5).

use std::io;
use std::process::Command;

use crate::browsers::{Browser, Engine};

/// Opens `url`: in `browser` (and its `profile`, when one is chosen), else in
/// the system's default browser.
pub fn open(url: &str, browser: Option<&Browser>, profile: Option<&str>) -> io::Result<()> {
    match browser {
        Some(b) => {
            // Only a profile the browser actually has: a stale choice falls
            // back to the browser's own default rather than creating one.
            let profile = profile.filter(|p| b.profiles.iter().any(|x| x.id == *p));
            Command::new(&b.exe).args(args_for(&b.engine, url, profile)).spawn().map(|_| ())
        }
        None => system_default(url),
    }
}

/// The browser's arguments for opening `url` in `profile`.
fn args_for(engine: &Engine, url: &str, profile: Option<&str>) -> Vec<String> {
    let mut args = Vec::new();
    match engine {
        // An app window: no tabs, no address bar, the profile's storage.
        Engine::Chromium => {
            if let Some(p) = profile {
                args.push(format!("--profile-directory={p}"));
            }
            args.push(format!("--app={url}"));
        }
        Engine::Gecko => {
            if let Some(p) = profile {
                args.extend(["-P".to_string(), p.to_string()]);
            }
            args.extend(["-new-window".to_string(), url.to_string()]);
        }
        Engine::Webkit | Engine::Unknown => args.push(url.to_string()),
    }
    args
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments() {
        let u = "https://a.example/open#x=1";
        assert_eq!(args_for(&Engine::Chromium, u, Some("Profile 7")), ["--profile-directory=Profile 7", "--app=https://a.example/open#x=1"]);
        assert_eq!(args_for(&Engine::Chromium, u, None), ["--app=https://a.example/open#x=1"]);
        assert_eq!(args_for(&Engine::Gecko, u, Some("default-nightly")), ["-P", "default-nightly", "-new-window", u]);
        assert_eq!(args_for(&Engine::Unknown, u, Some("x")), [u]);
    }
}

#[cfg(windows)]
fn system_default(url: &str) -> io::Result<()> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    // `start` treats its first quoted argument as a title: give it an empty one.
    Command::new("cmd").args(["/C", "start", "", url]).creation_flags(CREATE_NO_WINDOW).spawn().map(|_| ())
}

#[cfg(target_os = "macos")]
fn system_default(url: &str) -> io::Result<()> {
    Command::new("open").arg(url).spawn().map(|_| ())
}

#[cfg(all(unix, not(target_os = "macos")))]
fn system_default(url: &str) -> io::Result<()> {
    Command::new("xdg-open").arg(url).spawn().map(|_| ())
}
