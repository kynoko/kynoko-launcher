//! The system's own "Save as" dialog, for the bridge's `save-as` (docs/SPEC.md,
//! section 8): the user, not the page, picks where a file goes.
//!
//! rfd (MIT) shows the system's dialog: the common item dialog on Windows,
//! NSSavePanel on macOS, the XDG desktop portal on Linux (zenity where there
//! is no portal). Each asks before replacing a file. It is called on the
//! bridge's thread for that request, never on the window's: on macOS rfd
//! hands the panel to the main thread itself, and on Linux the portal is
//! another program.
//!
//! IN FRONT. The page that asks is in a browser, and the launcher is a
//! program in the background: Windows lets only the program in front bring
//! a window forward, so a dialog of its own could open BEHIND the browser.
//! On Windows the dialog is therefore given an owner, an empty window of the
//! launcher, always on top, put in front first (its thread shares for a
//! moment the input of the window in front, which Windows allows), centred
//! on the window that was in front so that the dialog opens over it; the
//! focus goes back to that window afterwards. Owned by a window that is
//! always on top, the dialog is on top even if Windows refused the focus.
//! macOS: rfd raises the panel above every window (the shielding level).

use std::path::PathBuf;

use crate::bridge::SaveAsk;

/// Shows the dialog for `ask` and answers the path the user picked, or None
/// when they cancelled. Blocks until then.
pub fn save_file(ask: &SaveAsk) -> Option<PathBuf> {
    let mut dialog = rfd::FileDialog::new().set_file_name(ask.name.as_str());
    if let Some(folder) = ask.folder.as_ref().filter(|f| f.is_dir()) {
        dialog = dialog.set_directory(folder);
    }
    if let Some(ext) = &ask.ext {
        // Named after the extension itself: a word would need translating.
        dialog = dialog.add_filter(format!(".{ext}"), &[ext.as_str()]);
    }
    #[cfg(windows)]
    return windows::in_front(dialog);
    #[cfg(not(windows))]
    dialog.save_file()
}

#[cfg(windows)]
mod windows {
    use std::num::NonZeroIsize;
    use std::path::PathBuf;

    use raw_window_handle::{
        DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, RawWindowHandle, Win32WindowHandle, WindowHandle,
    };
    use windows_sys::Win32::Foundation::{HWND, RECT};
    use windows_sys::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, GetForegroundWindow, GetSystemMetrics, GetWindowRect, GetWindowThreadProcessId,
        IsIconic, SetForegroundWindow, SM_CXSCREEN, SM_CYSCREEN, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP, WS_VISIBLE,
    };

    /// The dialog, owned by a window of the launcher put in front first (see
    /// the module's notes).
    pub fn in_front(dialog: rfd::FileDialog) -> Option<PathBuf> {
        // SAFETY: plain Win32 calls on windows this function creates and
        // destroys, or only reads (the one in front).
        unsafe {
            let before = GetForegroundWindow();
            let (x, y) = centre(before);
            // A system class ("STATIC"): nothing to register, and the dialog's
            // own loop runs its messages, on this same thread.
            let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
            let owner = CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
                class.as_ptr(),
                std::ptr::null(),
                WS_POPUP | WS_VISIBLE,
                x,
                y,
                0,
                0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null(),
            );
            if owner.is_null() {
                return dialog.save_file();
            }
            take_front(owner, before);
            let chosen = dialog.set_parent(&Owner(owner)).save_file();
            DestroyWindow(owner);
            if !before.is_null() {
                SetForegroundWindow(before);
            }
            chosen
        }
    }

    /// The centre of `window`, else of the main screen.
    unsafe fn centre(window: HWND) -> (i32, i32) {
        let mut rect = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        if !window.is_null() && IsIconic(window) == 0 && GetWindowRect(window, &mut rect) != 0 {
            return ((rect.left + rect.right) / 2, (rect.top + rect.bottom) / 2);
        }
        (GetSystemMetrics(SM_CXSCREEN) / 2, GetSystemMetrics(SM_CYSCREEN) / 2)
    }

    /// Puts `window` in front, borrowing for a moment the input of the
    /// thread whose window is (`before`): Windows refuses the front to a
    /// program in the background otherwise.
    unsafe fn take_front(window: HWND, before: HWND) {
        let me = GetCurrentThreadId();
        let theirs = if before.is_null() { 0 } else { GetWindowThreadProcessId(before, std::ptr::null_mut()) };
        let attached = theirs != 0 && theirs != me && AttachThreadInput(me, theirs, 1) != 0;
        SetForegroundWindow(window);
        if attached {
            AttachThreadInput(me, theirs, 0);
        }
    }

    /// The owner window, as rfd takes a parent.
    struct Owner(HWND);

    impl HasWindowHandle for Owner {
        fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
            let hwnd = NonZeroIsize::new(self.0 as isize).ok_or(HandleError::Unavailable)?;
            // SAFETY: the window outlives the dialog it owns (destroyed after it).
            Ok(unsafe { WindowHandle::borrow_raw(RawWindowHandle::Win32(Win32WindowHandle::new(hwnd))) })
        }
    }

    impl HasDisplayHandle for Owner {
        fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
            Ok(DisplayHandle::windows())
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::bridge::SaveAsk;
        use std::sync::{Arc, Mutex};
        use std::time::{Duration, Instant};
        use windows_sys::Win32::Foundation::{BOOL, LPARAM, WPARAM};
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            EnumWindows, GetClassNameW, GetWindowLongPtrW, IsWindowVisible, PostMessageW, GWL_EXSTYLE, IDCANCEL,
            WM_COMMAND,
        };

        /// The visible dialog box ("#32770") of this process, if one is open.
        fn open_dialog() -> Option<HWND> {
            unsafe extern "system" fn each(window: HWND, found: LPARAM) -> BOOL {
                let mut pid = 0u32;
                GetWindowThreadProcessId(window, &mut pid);
                let mut class = [0u16; 16];
                let len = GetClassNameW(window, class.as_mut_ptr(), class.len() as i32) as usize;
                let dialog: Vec<u16> = "#32770".encode_utf16().collect();
                if pid == std::process::id() && IsWindowVisible(window) != 0 && class[..len] == dialog[..] {
                    *(found as *mut HWND) = window;
                    return 0;
                }
                1
            }
            let mut found: HWND = std::ptr::null_mut();
            // SAFETY: the callback writes one HWND through the pointer it is given, alive for the call.
            unsafe { EnumWindows(Some(each), &mut found as *mut HWND as LPARAM) };
            (!found.is_null()).then_some(found)
        }

        /// The real dialog, on this desktop: it must be in front and on top
        /// of every window, its Cancel must answer None, and the window that
        /// was in front must be again. Shown for a moment and cancelled by
        /// the test itself; run on demand only (`cargo test -- --ignored`),
        /// never on a build machine.
        #[test]
        #[ignore = "shows the system's dialog on this desktop"]
        fn the_dialog_comes_in_front_and_cancels() {
            // SAFETY: plain Win32 calls, on windows of this process or read only.
            let before = unsafe { GetForegroundWindow() } as isize;
            let seen: Arc<Mutex<Option<(bool, bool)>>> = Arc::default();
            let watcher = {
                let seen = seen.clone();
                std::thread::spawn(move || {
                    let start = Instant::now();
                    while start.elapsed() < Duration::from_secs(20) {
                        if let Some(dialog) = open_dialog() {
                            // Let it settle, then look and close it.
                            std::thread::sleep(Duration::from_millis(700));
                            // SAFETY: as above.
                            unsafe {
                                let topmost = GetWindowLongPtrW(dialog, GWL_EXSTYLE) as u32 & WS_EX_TOPMOST != 0;
                                *seen.lock().unwrap() = Some((GetForegroundWindow() == dialog, topmost));
                                PostMessageW(dialog, WM_COMMAND, IDCANCEL as WPARAM, 0);
                            }
                            return;
                        }
                        std::thread::sleep(Duration::from_millis(50));
                    }
                })
            };
            let ask = SaveAsk {
                folder: Some(std::env::temp_dir()),
                name: "Kynoko test.kproj".into(),
                ext: Some("kproj".into()),
            };
            let chosen = crate::dialog::save_file(&ask);
            watcher.join().unwrap();
            assert_eq!(chosen, None);
            assert_eq!(*seen.lock().unwrap(), Some((true, true)), "(in front, on top)");
            // SAFETY: as above.
            assert_eq!(unsafe { GetForegroundWindow() } as isize, before, "the focus went back");
        }
    }
}
