//! Windows: each app's own program, for "Open with" (docs/SPEC.md, section 7).
//!
//! Windows lists the "Open with" entries of a file type by PROGRAM: the
//! types of two apps registered with the same program show as one entry,
//! named after whichever comes first. So every app with associations gets a
//! program of its own, `open-with\kynoko-<app>.exe` next to the launcher,
//! and its types give that entry the app's name and icon. The program is the
//! launcher's binary under another name (one copy, the other apps' programs
//! hard links to it); started, it hands its arguments to the launcher and
//! quits at once, so it never holds a file an update has to replace. Chrome
//! does the same for the web apps it installs.

use std::path::{Path, PathBuf};

use crate::settings::{Artefact, Inventory};

/// Next to the launcher, the folder of the apps' programs.
const DIR: &str = "open-with";

/// The launcher's own program (the package's binary).
const LAUNCHER: &str = "kynoko-launcher.exe";

fn file_name(code: &str) -> String {
    format!("kynoko-{}.exe", code.to_ascii_lowercase())
}

/// The launcher `me` stands for, when `me` is one of the apps' programs.
fn launcher_for(me: &Path) -> Option<PathBuf> {
    let dir = me.parent()?;
    if dir.file_name()? != DIR || me.file_name()? == LAUNCHER {
        return None;
    }
    Some(dir.parent()?.join(LAUNCHER))
}

/// When this process is one of the apps' programs: starts the launcher with
/// the same arguments, and says so (the caller then quits). With the
/// launcher gone (uninstalled), nothing happens.
pub fn hand_over() -> bool {
    let Some(launcher) = std::env::current_exe().ok().as_deref().and_then(launcher_for) else {
        return false;
    };
    if launcher.is_file() {
        let _ = std::process::Command::new(launcher).args(std::env::args_os().skip(1)).spawn();
    }
    true
}

/// Same length and same modification time as the launcher: what a hard link
/// shares, and what a copy keeps (CopyFile carries the time over).
fn current(launcher: &Path, program: &Path) -> bool {
    let (Ok(a), Ok(b)) = (std::fs::metadata(launcher), std::fs::metadata(program)) else {
        return false;
    };
    let (Ok(ta), Ok(tb)) = (a.modified(), b.modified()) else {
        return false;
    };
    a.len() == b.len() && ta == tb
}

/// Puts the launcher at `path`: a hard link to a current program beside it
/// (nothing copied twice), else a copy. An outdated one is replaced (the
/// launcher was updated since); while it runs, a matter of milliseconds, it
/// cannot go, and serves as it is.
fn make(launcher: &Path, path: &Path) -> std::io::Result<()> {
    if current(launcher, path) {
        return Ok(());
    }
    let dir = path.parent().ok_or(std::io::ErrorKind::InvalidInput)?;
    std::fs::create_dir_all(dir)?;
    if path.exists() && std::fs::remove_file(path).is_err() {
        return Ok(());
    }
    let sibling = std::fs::read_dir(dir)?
        .flatten()
        .map(|e| e.path())
        .find(|p| p != path && p.extension().is_some_and(|e| e == "exe") && current(launcher, p));
    if sibling.is_some_and(|s| std::fs::hard_link(s, path).is_ok()) {
        return Ok(());
    }
    std::fs::copy(launcher, path).map(|_| ())
}

/// `code`'s program, made current. None when it cannot be had (a folder the
/// user may not write to): the launcher then opens the app's types itself,
/// still under the app's name, only merged with any other app's entry for
/// a type both open.
pub fn ensure(launcher: &Path, code: &str, inventory: &mut Inventory) -> Option<PathBuf> {
    let path = launcher.parent()?.join(DIR).join(file_name(code));
    let dir = path.parent()?.to_string_lossy().into_owned();
    inventory.record(Artefact::Dir { path: dir }).ok()?;
    inventory.record(Artefact::File { path: path.to_string_lossy().into_owned() }).ok()?;
    make(launcher, &path).ok()?;
    Some(path)
}

/// Removes `code`'s program, and the folder with the last one.
pub fn remove(code: &str, inventory: &mut Inventory) -> std::io::Result<()> {
    let tail = format!(r"\{DIR}\{}", file_name(code));
    for a in inventory.artefacts.clone() {
        let Artefact::File { path } = &a else { continue };
        if !path.ends_with(&tail) {
            continue;
        }
        let _ = std::fs::remove_file(path);
        inventory.forget(&a)?;
        if let Some(dir) = Path::new(path).parent() {
            if std::fs::remove_dir(dir).is_ok() {
                inventory.forget(&Artefact::Dir { path: dir.to_string_lossy().into_owned() })?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn programs_stand_for_the_launcher_beside_them() {
        let root = Path::new(r"C:\Users\u\AppData\Local\Kynoko Launcher");
        assert_eq!(launcher_for(&root.join(r"open-with\kynoko-office.exe")), Some(root.join("kynoko-launcher.exe")));
        // The launcher itself, wherever it is.
        assert_eq!(launcher_for(&root.join("kynoko-launcher.exe")), None);
        assert_eq!(launcher_for(&root.join(r"open-with\kynoko-launcher.exe")), None);
        assert_eq!(launcher_for(&root.join(r"elsewhere\kynoko-office.exe")), None);
        assert_eq!(file_name("MediaStudio"), "kynoko-mediastudio.exe");
    }

    #[test]
    fn one_copy_then_links_and_outdated_ones_replaced() {
        let dir = std::env::temp_dir().join(format!("kynoko-programs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let launcher = dir.join(LAUNCHER);
        std::fs::write(&launcher, b"launcher v1").unwrap();
        let office = dir.join(DIR).join(file_name("Office"));
        let media = dir.join(DIR).join(file_name("MediaStudio"));

        make(&launcher, &office).unwrap();
        assert!(current(&launcher, &office), "a copy keeps the launcher's time");
        make(&launcher, &media).unwrap();
        assert_eq!(std::fs::read(&media).unwrap(), b"launcher v1");
        // The second one is a hard link to the first: one file, two names.
        std::fs::write(&office, b"through the other name").unwrap();
        assert_eq!(std::fs::read(&media).unwrap(), b"through the other name");
        std::fs::remove_file(&office).unwrap();
        assert!(std::fs::read(&media).is_ok(), "removing one name leaves the other");

        // The launcher updated: the program is outdated, then replaced.
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&launcher, b"launcher v2, longer").unwrap();
        assert!(!current(&launcher, &media));
        make(&launcher, &media).unwrap();
        assert_eq!(std::fs::read(&media).unwrap(), b"launcher v2, longer");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
