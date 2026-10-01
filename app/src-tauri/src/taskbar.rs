//! What a Kynoko window is to the system: its icon and, on Windows, its
//! application identity (docs/SPEC.md, section 5).
//!
//! A Kynoko window belongs to the launcher's process. Left as it is, Windows
//! draws every such window with the launcher's own icon and puts them all
//! under the launcher's one taskbar button: an Office document and a Photo
//! Studio picture side by side under the same icon. So an app's window gets
//! the app's icon at once, and its own application identity (an
//! AppUserModelID, the way Chromium's installed web apps do it) with what a
//! pinned button must run, show and draw. Office's windows then share one
//! button, apart from Photo Studio's, and a pinned Office opens Office. The
//! app's Start menu entries carry the same identity, so a pinned entry and
//! the windows it opens are one button. The page then puts its facade's icon
//! on its window (own_window_set_icon).
//!
//! macOS has one Dock icon per program and no window icon: nothing to do.
//! Linux: the window's icon only; the desktop groups windows its own way.

/// The application identity of an app's windows and Start menu entries.
/// Codes are plain (letters and digits, see plain_code), which the identity
/// format accepts as they are.
pub fn app_id(code: &str) -> String {
    format!("Kynoko.App.{code}")
}

/// Bigger than any icon a page draws (a 128 px PNG is about 25 kB).
const MAX_ICON_BYTES: usize = 1 << 20;

/// A PNG as a window icon: a real PNG, of an icon's size.
#[cfg_attr(target_os = "macos", allow(dead_code))]
pub fn window_icon(png: &[u8]) -> Result<tauri::image::Image<'static>, String> {
    if png.len() > MAX_ICON_BYTES || !png.starts_with(b"\x89PNG") {
        return Err("not a PNG icon".into());
    }
    let image = image::load_from_memory_with_format(png, image::ImageFormat::Png).map_err(|e| e.to_string())?;
    let (width, height) = (image.width(), image.height());
    if !(16..=1024).contains(&width) || !(16..=1024).contains(&height) {
        return Err(format!("an icon is not {width}x{height}"));
    }
    Ok(tauri::image::Image::new_owned(image.to_rgba8().into_raw(), width, height))
}

/// Puts `png` on `window` (taskbar, Alt+Tab, window switchers).
pub fn set_icon(window: &tauri::WebviewWindow, png: &[u8]) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let _ = (window, png);
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        window.set_icon(window_icon(png)?).map_err(|e| e.to_string())?;
        #[cfg(windows)]
        set_big_icon(window, png)?;
        Ok(())
    }
}

/// Windows has two icons per window, and Tauri sets the small one only: the
/// taskbar, which draws the big one, then fell back on the program's (the
/// launcher's). Made from the PNG itself (Windows reads PNG icons), at its
/// own size: the taskbar scales it down cleanly.
#[cfg(windows)]
fn set_big_icon(window: &tauri::WebviewWindow, png: &[u8]) -> Result<(), String> {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateIconFromResourceEx, DestroyIcon, SendMessageW, ICON_BIG, LR_DEFAULTCOLOR, WM_SETICON,
    };
    /// The big icon made here for each window: the only ones freed here.
    static MADE: OnceLock<Mutex<HashMap<isize, isize>>> = OnceLock::new();
    let hwnd = window.hwnd().map_err(|e| e.to_string())?.0;
    // SAFETY: the PNG outlives the call, and the system copies it into the icon.
    let icon = unsafe { CreateIconFromResourceEx(png.as_ptr(), png.len() as u32, 1, 0x0003_0000, 0, 0, LR_DEFAULTCOLOR) };
    if icon.is_null() {
        return Err("Windows could not make an icon of this PNG".into());
    }
    // SAFETY: a window of this process, and an icon it now holds.
    let previous = unsafe { SendMessageW(hwnd as _, WM_SETICON, ICON_BIG as usize, icon as isize) };
    let mut made = MADE.get_or_init(Default::default).lock().expect("icons lock");
    if previous != 0 && made.get(&(hwnd as isize)) == Some(&previous) {
        // SAFETY: an icon made here, which the window no longer holds.
        unsafe { DestroyIcon(previous as _) };
    }
    made.insert(hwnd as isize, icon as isize);
    Ok(())
}

/// What a pinned button of an app runs, is called, and shows.
#[cfg(windows)]
pub struct Relaunch {
    /// `"<launcher>" launch <code>`
    pub command: String,
    /// "Kynoko Office"
    pub name: String,
    /// The app's icon as an .ico file, when there is one yet.
    pub icon: Option<std::path::PathBuf>,
}

/// Gives `window` the identity `id` and what pinning it needs. Called on the
/// window's own (main) thread, before the window is shown: Windows files a
/// window under its identity when it first appears.
#[cfg(windows)]
pub fn identify(window: &tauri::WebviewWindow, id: &str, relaunch: &Relaunch) -> Result<(), String> {
    let hwnd = window.hwnd().map_err(|e| e.to_string())?.0;
    com::with_com(|| {
        let store = com::Store::of_window(hwnd)?;
        store.set(&com::RELAUNCH_COMMAND, &relaunch.command)?;
        store.set(&com::RELAUNCH_NAME, &relaunch.name)?;
        if let Some(icon) = &relaunch.icon {
            store.set(&com::RELAUNCH_ICON, &format!("{},0", icon.display()))?;
        }
        store.set(&com::ID, id)?;
        store.commit()
    })
}

/// Gives a Start menu entry the identity `id` (the windows it opens have the
/// same, so that pinning the entry pins the app's button).
#[cfg(windows)]
pub fn stamp_shortcut(lnk: &std::path::Path, id: &str) -> Result<(), String> {
    com::with_com(|| {
        let store = com::Store::of_file(lnk)?;
        store.set(&com::ID, id)?;
        store.commit()
    })
}

/// The few shell calls the identity needs, without a COM crate: the property
/// store of a window or of a file, and string values written into it.
#[cfg(windows)]
mod com {
    use std::ffi::c_void;

    use windows_sys::core::{GUID, HRESULT};
    use windows_sys::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};
    use windows_sys::Win32::UI::Shell::PropertiesSystem::{
        SHGetPropertyStoreForWindow, SHGetPropertyStoreFromParsingName, GPS_READWRITE, PROPERTYKEY,
    };

    const IID_IPROPERTYSTORE: GUID = GUID::from_u128(0x886d8eeb_8cf2_4446_8d02_cdba1dbdcf99);
    /// The property set of System.AppUserModel.* (propkey.h).
    const APP_USER_MODEL: GUID = GUID::from_u128(0x9f4c2855_9f79_4b39_a8d0_e1d42de1d5f3);
    pub const RELAUNCH_COMMAND: PROPERTYKEY = PROPERTYKEY { fmtid: APP_USER_MODEL, pid: 2 };
    pub const RELAUNCH_ICON: PROPERTYKEY = PROPERTYKEY { fmtid: APP_USER_MODEL, pid: 3 };
    pub const RELAUNCH_NAME: PROPERTYKEY = PROPERTYKEY { fmtid: APP_USER_MODEL, pid: 4 };
    pub const ID: PROPERTYKEY = PROPERTYKEY { fmtid: APP_USER_MODEL, pid: 5 };

    /// A PROPVARIANT holding a VT_LPWSTR: the type, three reserved words, the
    /// string, and the rest of the union (16 bytes on 32-bit, 24 on 64-bit).
    #[repr(C)]
    struct StringVariant {
        vt: u16,
        reserved: [u16; 3],
        value: *const u16,
        rest: *const c_void,
    }
    const VT_LPWSTR: u16 = 31;

    /// IPropertyStore's vtable: IUnknown, then GetCount, GetAt, GetValue,
    /// SetValue, Commit. Only what is called is typed.
    #[repr(C)]
    struct Vtbl {
        query_interface: usize,
        add_ref: usize,
        release: unsafe extern "system" fn(*mut c_void) -> u32,
        get_count: usize,
        get_at: usize,
        get_value: usize,
        set_value: unsafe extern "system" fn(*mut c_void, *const PROPERTYKEY, *const StringVariant) -> HRESULT,
        commit: unsafe extern "system" fn(*mut c_void) -> HRESULT,
    }

    /// An IPropertyStore, released when dropped.
    pub struct Store(*mut c_void);

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn check(hr: HRESULT, what: &str) -> Result<(), String> {
        if hr < 0 { Err(format!("{what}: 0x{:08x}", hr as u32)) } else { Ok(()) }
    }

    impl Store {
        pub fn of_window(hwnd: *mut c_void) -> Result<Store, String> {
            let mut store = std::ptr::null_mut();
            // SAFETY: a window handle of this process; the out pointer is ours.
            check(unsafe { SHGetPropertyStoreForWindow(hwnd as _, &IID_IPROPERTYSTORE, &mut store) }, "window store")?;
            if store.is_null() { Err("no window store".into()) } else { Ok(Store(store)) }
        }

        pub fn of_file(path: &std::path::Path) -> Result<Store, String> {
            let path = wide(&path.to_string_lossy());
            let mut store = std::ptr::null_mut();
            // SAFETY: a NUL-terminated path that outlives the call; the out pointer is ours.
            check(
                unsafe { SHGetPropertyStoreFromParsingName(path.as_ptr(), std::ptr::null_mut(), GPS_READWRITE, &IID_IPROPERTYSTORE, &mut store) },
                "file store",
            )?;
            if store.is_null() { Err("no file store".into()) } else { Ok(Store(store)) }
        }

        fn vtbl(&self) -> &Vtbl {
            // SAFETY: a COM object starts with its vtable pointer.
            unsafe { &**(self.0 as *const *const Vtbl) }
        }

        /// Writes a string value (the store copies it).
        pub fn set(&self, key: &PROPERTYKEY, value: &str) -> Result<(), String> {
            let text = wide(value);
            let variant = StringVariant { vt: VT_LPWSTR, reserved: [0; 3], value: text.as_ptr(), rest: std::ptr::null() };
            // SAFETY: a live store; the key and the value outlive the call.
            check(unsafe { (self.vtbl().set_value)(self.0, key, &variant) }, "set value")
        }

        pub fn commit(&self) -> Result<(), String> {
            // SAFETY: a live store.
            check(unsafe { (self.vtbl().commit)(self.0) }, "commit")
        }
    }

    impl Drop for Store {
        fn drop(&mut self) {
            // SAFETY: the reference the store was handed with.
            unsafe { (self.vtbl().release)(self.0) };
        }
    }

    /// Runs `f` with COM ready on this thread (and leaves it as it was).
    pub fn with_com<T>(f: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
        // SAFETY: balanced below when it succeeded; RPC_E_CHANGED_MODE (COM
        // already set up otherwise on this thread) is fine as it is.
        let hr = unsafe { CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32) };
        let result = f();
        if hr >= 0 {
            // SAFETY: pairs the successful CoInitializeEx above.
            unsafe { CoUninitialize() };
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(width: u32, height: u32) -> Vec<u8> {
        use image::ImageEncoder;
        let mut out = Vec::new();
        image::codecs::png::PngEncoder::new(&mut out)
            .write_image(&vec![200u8; (width * height * 4) as usize], width, height, image::ExtendedColorType::Rgba8)
            .unwrap();
        out
    }

    #[test]
    fn an_app_has_one_identity() {
        assert_eq!(app_id("Office"), "Kynoko.App.Office");
        assert_ne!(app_id("Office"), app_id("PhotoStudio"));
    }

    #[test]
    fn only_a_png_of_an_icon_size_becomes_an_icon() {
        let icon = window_icon(&png(128, 128)).unwrap();
        assert_eq!((icon.width(), icon.height()), (128, 128));
        assert!(window_icon(b"GIF89a....").is_err());
        assert!(window_icon(&png(8, 8)).is_err());
        assert!(window_icon(&png(2048, 16)).is_err());
        let mut huge = png(16, 16);
        huge.resize(MAX_ICON_BYTES + 1, 0);
        assert!(window_icon(&huge).is_err());
    }

    /// A shortcut takes the identity, and reads it back (Windows only, on a
    /// shortcut of the test's own, removed after).
    #[cfg(windows)]
    #[test]
    fn a_start_menu_entry_takes_the_app_identity() {
        let dir = std::env::temp_dir().join(format!("kynoko-taskbar-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let lnk = dir.join("Test.lnk");
        let exe = std::env::current_exe().unwrap();
        mslnk::ShellLink::new(&exe).unwrap().create_lnk(&lnk).unwrap();
        stamp_shortcut(&lnk, &app_id("Office")).unwrap();
        let read = std::process::Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                &format!(
                    "$s = (New-Object -ComObject Shell.Application).NameSpace('{}').ParseName('Test.lnk'); $s.ExtendedProperty('System.AppUserModel.ID')",
                    dir.display()
                ),
            ])
            .output()
            .unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(String::from_utf8_lossy(&read.stdout).trim(), "Kynoko.App.Office");
    }
}
