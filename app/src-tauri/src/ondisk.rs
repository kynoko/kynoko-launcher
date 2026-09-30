//! What the system can say about a file the bridge could not write, and how
//! to show a file in the system's file manager.
//!
//! WHO HOLDS IT. Windows keeps the answer: the Restart Manager - what Explorer
//! asks for its "the file is open in another program" message - lists the
//! processes that have a file open. macOS and Linux lock nothing by default,
//! so there is nothing to name there.
//!
//! SHOWN, SELECTED. Explorer's `/select`, Finder's `open -R`, and on Linux the
//! freedesktop FileManager1 interface (the major file managers implement it),
//! else the folder alone.

use std::io;
use std::path::Path;
use std::process::Command;

/// Why a write failed, in the words the page reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// Another program has the file open (Windows sharing rules).
    Locked,
    /// The file is marked read-only.
    ReadOnly,
    /// The system refused the access (permissions, a protected folder).
    Denied,
    /// Anything else.
    Failed,
}

impl Refusal {
    pub fn word(self) -> &'static str {
        match self {
            Refusal::Locked => "locked",
            Refusal::ReadOnly => "readonly",
            Refusal::Denied => "denied",
            Refusal::Failed => "failed",
        }
    }

    pub fn of(error: &io::Error, path: &Path) -> Refusal {
        if std::fs::metadata(path).map(|m| m.permissions().readonly()).unwrap_or(false) {
            return Refusal::ReadOnly;
        }
        if is_lock(error) {
            return Refusal::Locked;
        }
        match error.kind() {
            io::ErrorKind::PermissionDenied => Refusal::Denied,
            _ => Refusal::Failed,
        }
    }
}

/// Another program's open handle, as Windows reports it: a sharing or lock
/// violation, or ReplaceFileW unable to move the files around (1175-1177).
pub fn is_lock(error: &io::Error) -> bool {
    cfg!(windows) && matches!(error.raw_os_error(), Some(32 | 33 | 1175 | 1176 | 1177))
}

/// The programs that have `path` open, by the names the system gives them,
/// without repeats. Empty when nobody does, or the system cannot say.
#[cfg(windows)]
pub fn holders(path: &Path) -> Vec<String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::System::RestartManager::{
        RmEndSession, RmGetList, RmRegisterResources, RmStartSession, CCH_RM_SESSION_KEY, RM_PROCESS_INFO,
    };
    const ERROR_MORE_DATA: u32 = 234;

    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut session = 0u32;
    let mut key = [0u16; CCH_RM_SESSION_KEY as usize + 1];
    // SAFETY: the out-pointers are valid for the call, the key buffer has the documented size.
    if unsafe { RmStartSession(&mut session, 0, key.as_mut_ptr()) } != 0 {
        return Vec::new();
    }
    let names = (|| {
        let files = [wide.as_ptr()];
        // SAFETY: one NUL-terminated path, alive for the call.
        if unsafe { RmRegisterResources(session, 1, files.as_ptr(), 0, std::ptr::null(), 0, std::ptr::null()) } != 0 {
            return Vec::new();
        }
        let mut infos: Vec<RM_PROCESS_INFO> = Vec::new();
        // The list can grow between the size question and the answer: ask again.
        for _ in 0..3 {
            let (mut needed, mut count, mut reasons) = (0u32, infos.len() as u32, 0u32);
            let buffer = if infos.is_empty() { std::ptr::null_mut() } else { infos.as_mut_ptr() };
            // SAFETY: `buffer` holds `count` entries (or is null with count 0).
            let status = unsafe { RmGetList(session, &mut needed, &mut count, buffer, &mut reasons) };
            if status == 0 {
                infos.truncate(count as usize);
                let mut names: Vec<String> = Vec::new();
                for info in &infos {
                    let end = info.strAppName.iter().position(|&c| c == 0).unwrap_or(info.strAppName.len());
                    let name = String::from_utf16_lossy(&info.strAppName[..end]).trim().to_string();
                    if !name.is_empty() && !names.contains(&name) {
                        names.push(name);
                    }
                }
                return names;
            }
            if status != ERROR_MORE_DATA {
                return Vec::new();
            }
            // SAFETY: RM_PROCESS_INFO is plain data; all-zero is a valid value.
            infos = vec![unsafe { std::mem::zeroed() }; needed as usize];
        }
        Vec::new()
    })();
    // SAFETY: the session was started above.
    unsafe { RmEndSession(session) };
    names
}

#[cfg(not(windows))]
pub fn holders(_path: &Path) -> Vec<String> {
    Vec::new()
}

/// Shows `path` selected in the system's file manager.
pub fn reveal(path: &Path) -> io::Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // Explorer reads `/select,"<path>"` as ONE argument: quoted by hand,
        // since the standard quoting would wrap the switch too.
        Command::new("explorer.exe").raw_arg(format!("/select,\"{}\"", path.display())).spawn()?;
        Ok(())
    }
    #[cfg(target_os = "macos")]
    {
        Command::new("open").arg("-R").arg(path).spawn()?;
        Ok(())
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let shown = Command::new("dbus-send")
            .args([
                "--session",
                "--print-reply",
                "--dest=org.freedesktop.FileManager1",
                "--type=method_call",
                "/org/freedesktop/FileManager1",
                "org.freedesktop.FileManager1.ShowItems",
            ])
            .arg(format!("array:string:{}", file_uri(path)))
            .arg("string:")
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !shown {
            Command::new("xdg-open").arg(path.parent().unwrap_or(path)).spawn()?;
        }
        Ok(())
    }
}

/// A `file://` URI, every byte but the unreserved ones and `/` escaped.
#[cfg(all(unix, not(target_os = "macos")))]
fn file_uri(path: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    let mut uri = String::from("file://");
    for &b in path.as_os_str().as_bytes() {
        if b.is_ascii_alphanumeric() || b"-._~/".contains(&b) {
            uri.push(b as char);
        } else {
            uri.push_str(&format!("%{b:02X}"));
        }
    }
    uri
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_read_only_file_is_said_to_be_one() {
        let dir = std::env::temp_dir().join(format!("kynoko-ondisk-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("ro.txt");
        std::fs::write(&file, "x").unwrap();
        let mut perms = std::fs::metadata(&file).unwrap().permissions();
        perms.set_readonly(true);
        std::fs::set_permissions(&file, perms.clone()).unwrap();
        let error = io::Error::from(io::ErrorKind::PermissionDenied);
        assert_eq!(Refusal::of(&error, &file), Refusal::ReadOnly);
        #[allow(clippy::permissions_set_readonly_false)]
        perms.set_readonly(false);
        std::fs::set_permissions(&file, perms).unwrap();
        assert_eq!(Refusal::of(&error, &file), Refusal::Denied);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn windows_names_who_holds_a_file() {
        use std::os::windows::fs::OpenOptionsExt;
        let dir = std::env::temp_dir().join(format!("kynoko-holders-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("held.txt");
        std::fs::write(&file, "x").unwrap();
        let held = std::fs::OpenOptions::new().read(true).share_mode(0).open(&file).unwrap();
        // This very process holds it: the system must name at least one program.
        assert!(!holders(&file).is_empty());
        drop(held);
        assert!(holders(&file).is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn uris_escape_what_they_must() {
        assert_eq!(file_uri(Path::new("/home/a b/é.txt")), "file:///home/a%20b/%C3%A9.txt");
    }
}
