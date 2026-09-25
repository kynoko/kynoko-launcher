//! Opening an app's page in the chosen browser (docs/SPEC.md, section 5).

use std::io;
use std::process::Command;

use crate::browsers::{Browser, Engine};

/// Opens `url`: in `browser` when one is chosen, else the system's default.
pub fn open(url: &str, browser: Option<&Browser>) -> io::Result<()> {
    match browser {
        Some(b) => {
            let mut command = Command::new(&b.exe);
            match b.engine {
                // An app window: no tabs, no address bar, same profile storage.
                Engine::Chromium => command.arg(format!("--app={url}")),
                Engine::Gecko => command.args(["-new-window", url]),
                Engine::Webkit | Engine::Unknown => command.arg(url),
            };
            command.spawn().map(|_| ())
        }
        None => system_default(url),
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
