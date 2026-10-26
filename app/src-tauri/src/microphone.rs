//! The microphone in a Kynoko window (docs/SPEC.md, section 5, "The
//! microphone in a Kynoko window").
//!
//! Only for an app the catalogue says uses it (`framePermissions`), only on
//! that app's own origin, and for sound alone: any other page in the window
//! (a payment page, another app) is refused, and so is an app that never
//! declared it. The camera is not this module's business: each engine keeps
//! answering it as it did.
//!
//! Each engine needs its own way in:
//! - Windows (WebView2): its `PermissionRequested` event. A declared app is
//!   left to WebView2's own prompt, per site; everything else is refused.
//! - macOS (WKWebView): wry answers every capture request with "grant" in its
//!   UI delegate. That answer is replaced, once, by `decide`, which grants the
//!   microphone to the windows registered here for their app's origin and
//!   refuses it to everything else. macOS then asks the user once for the
//!   whole launcher (`NSMicrophoneUsageDescription`).
//! - Linux (WebKitGTK): no media streams at all unless the program turns them
//!   on (`enable-media-stream`, off by default), and a permission request
//!   nobody answers is refused. Both are done for a declared app only. The
//!   setting counts from the next document the window makes, so such a
//!   window starts on a blank page, is set up, then goes to its app. No
//!   question to the user there: the app asks first, in its own words, and
//!   turns the microphone on only when told to (its voice switch).

use tauri::{Url, WebviewWindow};

/// Gives `window`, just built for the app at `target`, the microphone when the
/// app declares it (`declared`), and refuses it otherwise. On Linux a window
/// whose app declares it was built blank, and is sent to `target` here.
pub fn hold(window: &WebviewWindow, target: Url, declared: bool) {
    #[cfg(target_os = "linux")]
    linux::hold(window, target, declared);
    #[cfg(target_os = "macos")]
    macos::hold(window, target, declared);
    #[cfg(windows)]
    windows::hold(window, target, declared);
}

#[cfg(target_os = "linux")]
mod linux {
    use tauri::{Url, WebviewWindow};
    use webkit2gtk::glib::object::Cast;
    use webkit2gtk::{
        PermissionRequestExt, SettingsExt, UserMediaPermissionRequest, UserMediaPermissionRequestExt, WebViewExt,
    };

    pub fn hold(window: &WebviewWindow, target: Url, declared: bool) {
        if !declared {
            // Media streams stay off: the page has no microphone to ask for.
            return;
        }
        let origin = target.origin();
        let page = target.clone();
        let done = window.with_webview(move |webview| {
            let view = webview.inner();
            if let Some(settings) = WebViewExt::settings(&view) {
                settings.set_enable_media_stream(true);
            }
            view.connect_permission_request(move |view, request| {
                let Some(media) = request.downcast_ref::<UserMediaPermissionRequest>() else {
                    // Not a microphone or camera request: left to WebKitGTK.
                    return false;
                };
                let own = view.uri().and_then(|u| Url::parse(&u).ok()).is_some_and(|u| u.origin() == origin);
                if own && media.is_for_audio_device() && !media.is_for_video_device() {
                    request.allow();
                } else {
                    request.deny();
                }
                true
            });
            view.load_uri(page.as_str());
        });
        if let Err(e) = done {
            // Not set up: the window still goes to its app, without a microphone.
            eprintln!("kynoko-launcher: cannot give the window a microphone: {e}");
            let _ = window.navigate(target);
        }
    }
}

#[cfg(windows)]
mod windows {
    use tauri::{Url, WebviewWindow};
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        COREWEBVIEW2_PERMISSION_KIND, COREWEBVIEW2_PERMISSION_KIND_MICROPHONE, COREWEBVIEW2_PERMISSION_STATE_DENY,
    };
    use webview2_com::{take_pwstr, PermissionRequestedEventHandler};
    use windows_core::PWSTR;

    pub fn hold(window: &WebviewWindow, target: Url, declared: bool) {
        let own = declared.then(|| target.origin());
        let done = window.with_webview(move |webview| unsafe {
            let Ok(core) = webview.controller().CoreWebView2() else {
                eprintln!("kynoko-launcher: no WebView2 to hold the microphone in");
                return;
            };
            let mut token = 0i64;
            let handler = PermissionRequestedEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args else { return Ok(()) };
                let mut kind = COREWEBVIEW2_PERMISSION_KIND::default();
                args.PermissionKind(&mut kind)?;
                if kind != COREWEBVIEW2_PERMISSION_KIND_MICROPHONE {
                    return Ok(());
                }
                // The origin of the content asking, a frame's included.
                let mut uri = PWSTR::null();
                args.Uri(&mut uri)?;
                let asking = Url::parse(&take_pwstr(uri)).ok().map(|u| u.origin());
                if own.is_none() || asking != own {
                    args.SetState(COREWEBVIEW2_PERMISSION_STATE_DENY)?;
                }
                // Otherwise left as it is: WebView2 asks the user, per site.
                Ok(())
            }));
            if let Err(e) = core.add_PermissionRequested(&handler, &mut token) {
                eprintln!("kynoko-launcher: cannot hold the microphone: {e}");
            }
        });
        if let Err(e) = done {
            eprintln!("kynoko-launcher: cannot hold the microphone: {e}");
        }
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use block2::Block;
    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, Imp, Sel};
    use objc2::{msg_send, sel};
    use objc2_web_kit::{WKMediaCaptureType, WKPermissionDecision, WKSecurityOrigin};
    use tauri::{Url, WebviewWindow, WindowEvent};

    /// The Kynoko windows' web views, by address, with the origin each may
    /// use the microphone on (None: refused). A web view not listed (the
    /// launcher's own window) is refused too.
    static VIEWS: Mutex<Option<HashMap<usize, Option<String>>>> = Mutex::new(None);

    /// Whether wry's answer has been replaced by `decide`.
    static TAKEN_OVER: AtomicBool = AtomicBool::new(false);

    pub fn hold(window: &WebviewWindow, target: Url, declared: bool) {
        let own = declared.then(|| target.origin().ascii_serialization());
        let address = Arc::new(AtomicUsize::new(0));
        let kept = address.clone();
        let done = window.with_webview(move |webview| {
            let view = webview.inner() as usize;
            kept.store(view, Ordering::SeqCst);
            if let Ok(mut views) = VIEWS.lock() {
                views.get_or_insert_with(HashMap::new).insert(view, own);
            }
            // SAFETY: tauri hands over its live WKWebView, on the main thread.
            unsafe { take_over(view as *mut AnyObject) };
        });
        if let Err(e) = done {
            eprintln!("kynoko-launcher: cannot hold the microphone: {e}");
        }
        // A web view's address may serve another one later: forgotten with
        // its window.
        window.on_window_event(move |event| {
            if let WindowEvent::Destroyed = event {
                let view = address.load(Ordering::SeqCst);
                if let Ok(mut views) = VIEWS.lock() {
                    if let Some(views) = views.as_mut() {
                        views.remove(&view);
                    }
                }
            }
        });
    }

    /// Replaces, once, the capture answer of the class of `view`'s UI
    /// delegate (wry's) with `decide`.
    unsafe fn take_over(view: *mut AnyObject) {
        if TAKEN_OVER.load(Ordering::SeqCst) || view.is_null() {
            return;
        }
        let delegate: Option<Retained<AnyObject>> = msg_send![&*view, UIDelegate];
        let Some(delegate) = delegate else { return };
        let selector = sel!(webView:requestMediaCapturePermissionForOrigin:initiatedByFrame:type:decisionHandler:);
        let Some(method) = delegate.class().instance_method(selector) else {
            eprintln!("kynoko-launcher: wry answers no capture request any more; WebKit asks the user for every page");
            return;
        };
        let answer: unsafe extern "C-unwind" fn(
            *mut AnyObject,
            Sel,
            *mut AnyObject,
            &WKSecurityOrigin,
            *mut AnyObject,
            WKMediaCaptureType,
            &Block<dyn Fn(WKPermissionDecision)>,
        ) = decide;
        // SAFETY: `decide` has the signature of the method it replaces (wry's
        // `request_media_capture_permission`), and refuses more than it.
        method.set_implementation(std::mem::transmute::<_, Imp>(answer));
        TAKEN_OVER.store(true, Ordering::SeqCst);
    }

    /// `webView:requestMediaCapturePermissionForOrigin:initiatedByFrame:type:decisionHandler:`
    /// for every web view of the launcher.
    unsafe extern "C-unwind" fn decide(
        _this: *mut AnyObject,
        _cmd: Sel,
        view: *mut AnyObject,
        origin: &WKSecurityOrigin,
        _frame: *mut AnyObject,
        kind: WKMediaCaptureType,
        handler: &Block<dyn Fn(WKPermissionDecision)>,
    ) {
        let decision = if kind == WKMediaCaptureType::Camera {
            // Not ours: answered as wry does (macOS keeps the camera to itself
            // anyway, the bundle declares no use for it).
            WKPermissionDecision::Grant
        } else {
            let asking = origin_of(origin);
            let allowed = VIEWS
                .lock()
                .ok()
                .and_then(|views| views.as_ref().and_then(|v| v.get(&(view as usize)).cloned()))
                .flatten();
            if allowed.is_some_and(|own| own == asking) {
                WKPermissionDecision::Grant
            } else {
                WKPermissionDecision::Deny
            }
        };
        handler.call((decision,));
    }

    /// `https://host[:port]`, as `Url::origin().ascii_serialization()` writes
    /// it (no port when it is the default one, which WebKit gives as 0).
    unsafe fn origin_of(origin: &WKSecurityOrigin) -> String {
        let (scheme, host, port) = (origin.protocol().to_string(), origin.host().to_string(), origin.port());
        if port == 0 {
            format!("{scheme}://{host}")
        } else {
            format!("{scheme}://{host}:{port}")
        }
    }
}
