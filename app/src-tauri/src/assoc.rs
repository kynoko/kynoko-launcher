//! File associations (docs/SPEC.md, section 7).
//!
//! Windows: everything under HKEY_CURRENT_USER (no admin rights), each key
//! and value recorded in the inventory BEFORE it is written, so that
//! `remove_all` takes away exactly what was added. The default handler of an
//! extension cannot be set by a program on Windows (its choice is hashed);
//! the launcher registers itself so that it appears in "Open with" and in
//! Settings > Default apps, where the user picks it.

use crate::catalogue::App;
use crate::settings::{Artefact, Inventory};

#[cfg(windows)]
const CLASSES: &str = r"Software\Classes";
#[cfg(windows)]
const CAPABILITIES: &str = r"Software\Kynoko\Launcher\Capabilities";
#[cfg(windows)]
const REGISTERED_APPS: &str = r"Software\RegisteredApplications";
#[cfg(windows)]
pub const APPLICATION_NAME: &str = "Kynoko Launcher";

#[cfg(windows)]
fn prog_id(app: &App, ext: &str) -> String {
    format!("Kynoko.{}.{}", app.code, ext)
}

/// Registers the launcher for every file type `app` opens.
#[cfg(windows)]
pub fn register(app: &App, inventory: &mut Inventory) -> std::io::Result<()> {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;

    let exe = std::env::current_exe()?.to_string_lossy().into_owned();
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);

    // The launcher as an application Windows can list under Default apps.
    inventory.record(Artefact::RegistryKey { path: CAPABILITIES.into() })?;
    let (caps, _) = hkcu.create_subkey(CAPABILITIES)?;
    caps.set_value("ApplicationName", &APPLICATION_NAME)?;
    caps.set_value("ApplicationDescription", &"Opens your files in the Kynoko apps")?;
    inventory.record(Artefact::RegistryValue { path: REGISTERED_APPS.into(), name: APPLICATION_NAME.into() })?;
    let (registered, _) = hkcu.create_subkey(REGISTERED_APPS)?;
    registered.set_value(APPLICATION_NAME, &CAPABILITIES)?;
    let (file_assoc, _) = hkcu.create_subkey(format!(r"{CAPABILITIES}\FileAssociations"))?;

    for ext in app.extensions() {
        let id = prog_id(app, &ext);
        let class = format!(r"{CLASSES}\{id}");
        inventory.record(Artefact::RegistryKey { path: class.clone() })?;
        let (key, _) = hkcu.create_subkey(&class)?;
        key.set_value("", &format!("{} ({})", app.name("en"), ext.to_uppercase()))?;
        let (icon, _) = key.create_subkey("DefaultIcon")?;
        icon.set_value("", &format!("\"{exe}\",0"))?;
        let (command, _) = key.create_subkey(r"shell\open\command")?;
        command.set_value("", &format!("\"{exe}\" open \"%1\""))?;

        // In the extension's "Open with" list: a value in a key that is not ours.
        let with = format!(r"{CLASSES}\.{ext}\OpenWithProgids");
        inventory.record(Artefact::RegistryValue { path: with.clone(), name: id.clone() })?;
        let (open_with, _) = hkcu.create_subkey(&with)?;
        open_with.set_raw_value(&id, &winreg::RegValue { bytes: vec![], vtype: winreg::enums::RegType::REG_NONE })?;

        file_assoc.set_value(format!(".{ext}"), &id)?;
    }
    notify_shell();
    Ok(())
}

/// Removes what `register` wrote for `app` (other apps keep theirs).
#[cfg(windows)]
pub fn unregister(app: &App, inventory: &mut Inventory) -> std::io::Result<()> {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    for ext in app.extensions() {
        let id = prog_id(app, &ext);
        let class = Artefact::RegistryKey { path: format!(r"{CLASSES}\{id}") };
        let with = Artefact::RegistryValue { path: format!(r"{CLASSES}\.{ext}\OpenWithProgids"), name: id.clone() };
        remove(&hkcu, &with);
        inventory.forget(&with)?;
        remove(&hkcu, &class);
        inventory.forget(&class)?;
        if let Ok(fa) = hkcu.open_subkey_with_flags(format!(r"{CAPABILITIES}\FileAssociations"), winreg::enums::KEY_WRITE) {
            let _ = fa.delete_value(format!(".{ext}"));
        }
    }
    notify_shell();
    Ok(())
}

/// The launcher's own address, `kynoko-launcher://`: what the apps' menu
/// entry opens. Registered at every start (idempotent) and recorded, so the
/// cleanup takes it away with the rest.
pub const SCHEME: &str = "kynoko-launcher";

#[cfg(windows)]
pub fn register_scheme(inventory: &mut Inventory) -> std::io::Result<()> {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;
    let exe = std::env::current_exe()?.to_string_lossy().into_owned();
    let path = format!(r"{CLASSES}\{SCHEME}");
    inventory.record(Artefact::RegistryKey { path: path.clone() })?;
    let (key, _) = RegKey::predef(HKEY_CURRENT_USER).create_subkey(&path)?;
    key.set_value("", &format!("URL:{APPLICATION_NAME}"))?;
    key.set_value("URL Protocol", &"")?;
    let (icon, _) = key.create_subkey("DefaultIcon")?;
    icon.set_value("", &format!("\"{exe}\",0"))?;
    let (command, _) = key.create_subkey(r"shell\open\command")?;
    command.set_value("", &format!("\"{exe}\" \"%1\""))?;
    Ok(())
}

#[cfg(not(windows))]
pub fn register_scheme(_inventory: &mut Inventory) -> std::io::Result<()> {
    // macOS declares it in Info.plist, Linux in the .desktop file (next milestones).
    Ok(())
}

/// Removes EVERYTHING in the inventory, newest first (uninstall, "Remove everything").
pub fn remove_all(inventory: &mut Inventory) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        use winreg::enums::HKEY_CURRENT_USER;
        use winreg::RegKey;
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        for artefact in inventory.artefacts.clone().iter().rev() {
            remove(&hkcu, artefact);
            inventory.forget(artefact)?;
        }
        // Our own vendor key, now empty.
        let _ = hkcu.delete_subkey(r"Software\Kynoko\Launcher");
        let _ = hkcu.delete_subkey(r"Software\Kynoko");
        notify_shell();
    }
    #[cfg(not(windows))]
    {
        for artefact in inventory.artefacts.clone().iter().rev() {
            if let Artefact::File { path } = artefact {
                let _ = std::fs::remove_file(path);
            }
            inventory.forget(artefact)?;
        }
    }
    Ok(())
}

#[cfg(windows)]
fn remove(hkcu: &winreg::RegKey, artefact: &Artefact) {
    match artefact {
        Artefact::RegistryKey { path } => {
            let _ = hkcu.delete_subkey_all(path);
        }
        Artefact::RegistryValue { path, name } => {
            if let Ok(key) = hkcu.open_subkey_with_flags(path, winreg::enums::KEY_ALL_ACCESS) {
                let _ = key.delete_value(name);
            }
            // Creating `.ext\OpenWithProgids` also created `.ext` when the
            // user had none. Both go if WE left them empty; a key holding
            // anything else belongs to someone and stays.
            if path.ends_with(r"\OpenWithProgids") {
                prune_if_empty(hkcu, path);
                if let Some((parent, _)) = path.rsplit_once('\\') {
                    prune_if_empty(hkcu, parent);
                }
            }
        }
        Artefact::File { path } => {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// Deletes the key at `path` only when it holds no value (default included)
/// and no subkey.
#[cfg(windows)]
fn prune_if_empty(hkcu: &winreg::RegKey, path: &str) {
    let empty = hkcu
        .open_subkey(path)
        .map(|key| key.enum_values().next().is_none() && key.enum_keys().next().is_none())
        .unwrap_or(false);
    if empty {
        let _ = hkcu.delete_subkey(path);
    }
}

/// Tells Explorer the associations changed, so "Open with" is right at once.
#[cfg(windows)]
fn notify_shell() {
    use windows_sys::Win32::UI::Shell::{SHChangeNotify, SHCNE_ASSOCCHANGED, SHCNF_IDLIST};
    // SAFETY: documented call with null item pointers for SHCNE_ASSOCCHANGED.
    unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED as i32, SHCNF_IDLIST, std::ptr::null(), std::ptr::null()) };
}

#[cfg(not(windows))]
pub fn register(_app: &App, _inventory: &mut Inventory) -> std::io::Result<()> {
    Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "file associations: Windows only in this build"))
}

#[cfg(not(windows))]
pub fn unregister(_app: &App, _inventory: &mut Inventory) -> std::io::Result<()> {
    Ok(())
}
