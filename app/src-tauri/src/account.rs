//! The Kynoko account: the sign-in kept on this computer and handed to every
//! browser the launcher opens an app in (docs/SPEC.md, section 20; the
//! platform's side is its `backend/specifications/LAUNCHER_SIGN_IN.md`).
//!
//! - Signing in: OAuth 2.1 with PKCE S256 in a Kynoko window (lib.rs opens it
//!   at `authorize_url` and catches the redirect, which it never loads); the
//!   code and its verifier are traded for an access token (10 minutes, kept in
//!   memory) and a device token (a rotating refresh token, kept in the
//!   system's credential store and nowhere else).
//! - Handing it over: before opening an app address, `carry` asks the
//!   platform for a one-time code bound to that app and answers the address
//!   to open instead (the platform's exchange, which sets the browser's own
//!   session and sends it on to the app). The app address's fragment rides on
//!   it, so that the browser carries it through the redirect and the platform
//!   never sees it. Two seconds at most: past them, the app opens as before.
//! - Never: a local endpoint that hands out anything, the browser's cookie or
//!   session token in the launcher's hands, a token or a code in a log.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::settings::{self, Settings};

pub const CLIENT_ID: &str = "kynoko-launcher";
/// The platform's front (consent) and API (tokens, codes, exchange).
const ACCOUNT_URL: &str = "https://kynoko.com/";
const ACCOUNT_API_URL: &str = "https://api.kynoko.com/";
/// The credential store entry.
const STORE_SERVICE: &str = "Kynoko Launcher";
const STORE_ENTRY: &str = "device-token";
/// A sign-in code: asked for before each opening, never waited for longer.
const CODE_WAIT: Duration = Duration::from_secs(2);
/// The token requests (sign-in, renewal, sign-out).
const TOKEN_WAIT: Duration = Duration::from_secs(15);
/// An access token is renewed this long before it expires.
const ACCESS_MARGIN: Duration = Duration::from_secs(60);
/// A sign-in left open this long is forgotten.
const PENDING_FOR: Duration = Duration::from_secs(30 * 60);

/// What the window shows.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AccountView {
    pub signed_in: bool,
    /// A sign-in window is open.
    pub pending: bool,
    pub email: Option<String>,
    pub name: Option<String>,
    /// The store refused the token: signed in for this run only.
    pub not_kept: bool,
}

#[derive(Default)]
struct State {
    /// The device token, once read from the store (or kept in memory: an
    /// isolated run, a system without a store).
    device_token: Option<String>,
    loaded: bool,
    not_kept: bool,
    access: Option<(String, Instant)>,
    pending: Option<Pending>,
}

struct Pending {
    state: String,
    verifier: String,
    since: Instant,
}

#[derive(Default)]
pub struct Account {
    inner: Mutex<State>,
}

impl Account {
    // ----------------------------------------------------------- signing in

    /// A new sign-in: the address of the consent, for the sign-in window.
    pub fn begin(&self, settings: &Settings, version: &str) -> String {
        let state = random_b64(32);
        let verifier = random_b64(64);
        let challenge = b64url(&Sha256::digest(verifier.as_bytes()));
        self.lock().pending = Some(Pending { state: state.clone(), verifier, since: Instant::now() });
        let mut url = tauri::Url::parse(&format!("{}oauth/authorize", front(settings))).expect("account url");
        url.query_pairs_mut()
            .append_pair("response_type", "code")
            .append_pair("client_id", CLIENT_ID)
            .append_pair("redirect_uri", &redirect_uri(settings))
            .append_pair("state", &state)
            .append_pair("code_challenge", &challenge)
            .append_pair("code_challenge_method", "S256")
            .append_pair("device_name", &device_name())
            .append_pair("device_os", std::env::consts::OS)
            .append_pair("client_version", version);
        url.to_string()
    }

    /// Whether a navigation is the sign-in's redirect (the window catches it).
    pub fn is_redirect(settings: &Settings, url: &str) -> bool {
        let redirect = redirect_uri(settings);
        url.strip_prefix(&redirect).is_some_and(|rest| rest.is_empty() || rest.starts_with('?') || rest.starts_with('#'))
    }

    /// The redirect arrived: its code and the verifier traded for the tokens.
    pub fn finish(&self, settings: &mut Settings, redirect: &str) -> Result<(), String> {
        let url = tauri::Url::parse(redirect).map_err(|e| e.to_string())?;
        let get = |name: &str| url.query_pairs().find(|(k, _)| k == name).map(|(_, v)| v.into_owned());
        let pending = self.lock().pending.take().ok_or("no sign-in under way")?;
        if pending.since.elapsed() > PENDING_FOR {
            return Err("the sign-in took too long".into());
        }
        let state = get("state").unwrap_or_default();
        if !constant_eq(state.as_bytes(), pending.state.as_bytes()) {
            return Err("the sign-in's state does not match".into());
        }
        if let Some(error) = get("error") {
            return Err(error);
        }
        let code = get("code").ok_or("no code")?;
        let answer = token_request(settings, &[
            ("grant_type", "authorization_code"),
            ("code", &code),
            ("redirect_uri", &redirect_uri(settings)),
            ("client_id", CLIENT_ID),
            ("code_verifier", &pending.verifier),
        ])?;
        self.take_tokens(settings, &answer)
    }

    pub fn cancel(&self) {
        self.lock().pending = None;
    }

    // --------------------------------------------------------- handing over

    /// The address to open for `url` (an app address, fragment included):
    /// the platform's exchange when a code could be had, else `url` itself.
    pub fn carry(&self, settings: &mut Settings, url: &str) -> String {
        if !self.signed_in() {
            return url.to_string();
        }
        let (base, fragment) = match url.split_once('#') {
            Some((b, f)) => (b, Some(f)),
            None => (url, None),
        };
        let Some(access) = self.access_token(settings) else { return url.to_string() };
        let answer = ureq::post(&format!("{}api/launcher/sign-in-code/", api(settings)))
            .timeout(CODE_WAIT)
            .set("Authorization", &format!("Bearer {access}"))
            .send_json(serde_json::json!({ "to": base }));
        let exchange = match answer {
            Ok(r) => r.into_json::<Value>().ok().and_then(|v| v.get("url").and_then(Value::as_str).map(str::to_string)),
            Err(ureq::Error::Status(401, _)) => {
                // The access token is no longer honoured (device revoked): the
                // next renewal says whether the sign-in still holds.
                self.lock().access = None;
                None
            }
            Err(_) => None,
        };
        match (exchange, fragment) {
            (Some(exchange), Some(f)) if exchange.starts_with("https://") || exchange.starts_with("http://") => format!("{exchange}#{f}"),
            (Some(exchange), None) if exchange.starts_with("https://") || exchange.starts_with("http://") => exchange,
            _ => url.to_string(),
        }
    }

    // -------------------------------------------------------------- signing out

    /// Revokes the device token (best effort) and forgets it.
    pub fn sign_out(&self, settings: &mut Settings) {
        let token = {
            let mut state = self.lock();
            state.access = None;
            state.pending = None;
            state.loaded = true;
            state.device_token.take()
        }
        .or_else(store_get);
        if let Some(token) = token {
            let _ = ureq::post(&format!("{}endpoints/oauth/revoke/", api(settings)))
                .timeout(TOKEN_WAIT)
                .send_form(&[("token", &token), ("client_id", CLIENT_ID)]);
        }
        store_delete();
        settings.account_email = None;
        settings.account_name = None;
        let _ = settings.save();
    }

    // ------------------------------------------------------------------ state

    pub fn signed_in(&self) -> bool {
        let mut state = self.lock();
        if !state.loaded {
            state.loaded = true;
            if !settings::isolated() {
                state.device_token = store_get();
            }
        }
        state.device_token.is_some()
    }

    pub fn view(&self, settings: &Settings) -> AccountView {
        let signed_in = self.signed_in();
        let state = self.lock();
        AccountView {
            signed_in,
            pending: state.pending.as_ref().is_some_and(|p| p.since.elapsed() < PENDING_FOR),
            email: signed_in.then(|| settings.account_email.clone()).flatten(),
            name: signed_in.then(|| settings.account_name.clone()).flatten(),
            not_kept: state.not_kept,
        }
    }

    // -------------------------------------------------------------- the tokens

    /// A valid access token: the one in memory, else a renewal (which rotates
    /// the device token, written back before use). None when there is none to
    /// be had now; a refused renewal signs out.
    fn access_token(&self, settings: &mut Settings) -> Option<String> {
        {
            let state = self.lock();
            if let Some((token, until)) = &state.access {
                if Instant::now() + ACCESS_MARGIN < *until {
                    return Some(token.clone());
                }
            }
        }
        let device = self.lock().device_token.clone()?;
        match token_request(settings, &[("grant_type", "refresh_token"), ("refresh_token", &device), ("client_id", CLIENT_ID)]) {
            Ok(answer) => {
                self.take_tokens(settings, &answer).ok()?;
                self.lock().access.as_ref().map(|(t, _)| t.clone())
            }
            Err(e) if e == "invalid_grant" || e == "invalid_client" => {
                // Revoked, expired, or reused elsewhere: the sign-in is over.
                {
                    let mut state = self.lock();
                    state.device_token = None;
                    state.access = None;
                }
                store_delete();
                settings.account_email = None;
                settings.account_name = None;
                let _ = settings.save();
                None
            }
            Err(_) => None,
        }
    }

    /// Keeps a token answer: the device token to the store FIRST (a rotated
    /// one lost would sign the launcher out), then the access token.
    fn take_tokens(&self, settings: &mut Settings, answer: &Value) -> Result<(), String> {
        let access = answer.get("access_token").and_then(Value::as_str).ok_or("no access token")?;
        let device = answer.get("refresh_token").and_then(Value::as_str).ok_or("no device token")?;
        let expires_in = answer.get("expires_in").and_then(Value::as_u64).unwrap_or(600);
        let kept = settings::isolated() || store_set(device).is_ok();
        {
            let mut state = self.lock();
            state.device_token = Some(device.to_string());
            state.loaded = true;
            state.not_kept = !kept;
            state.access = Some((access.to_string(), Instant::now() + Duration::from_secs(expires_in)));
        }
        if let Some(account) = answer.get("account") {
            settings.account_email = account.get("email").and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string);
            settings.account_name = account.get("name").and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string);
            let _ = settings.save();
        }
        Ok(())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.inner.lock().expect("account lock")
    }
}

/// A token request; the OAuth error code (`invalid_grant`...) on refusal.
fn token_request(settings: &Settings, form: &[(&str, &str)]) -> Result<Value, String> {
    match ureq::post(&format!("{}endpoints/oauth/token/", api(settings))).timeout(TOKEN_WAIT).send_form(form) {
        Ok(r) => r.into_json::<Value>().map_err(|e| e.to_string()),
        Err(ureq::Error::Status(_, r)) => Err(r
            .into_json::<Value>()
            .ok()
            .and_then(|v| v.get("error").and_then(Value::as_str).map(str::to_string))
            .unwrap_or_else(|| "refused".into())),
        Err(e) => Err(e.to_string()),
    }
}

fn front(settings: &Settings) -> String {
    with_slash(settings.account_url.as_deref().unwrap_or(ACCOUNT_URL))
}

fn api(settings: &Settings) -> String {
    with_slash(settings.account_api_url.as_deref().unwrap_or(ACCOUNT_API_URL))
}

/// The platform's callback page, which the sign-in window never loads.
pub fn redirect_uri(settings: &Settings) -> String {
    format!("{}oauth/launcher-callback", front(settings))
}

/// The hosts the sign-in window may show: the platform's front and its
/// subdomains (the sign-in and sign-up pages, nothing else).
pub fn may_show(settings: &Settings, url: &tauri::Url) -> bool {
    let Some(host) = url.host_str() else { return false };
    let front_host = tauri::Url::parse(&front(settings)).ok().and_then(|u| u.host_str().map(str::to_string)).unwrap_or_default();
    let within = |d: &str| host == d || host.ends_with(&format!(".{d}"));
    url.scheme() == "https" && (within("kynoko.com") || (!front_host.is_empty() && host == front_host))
}

fn with_slash(url: &str) -> String {
    if url.ends_with('/') { url.to_string() } else { format!("{url}/") }
}

/// The computer's name, as the account's device list shows it.
fn device_name() -> String {
    let name = gethostname::gethostname().to_string_lossy().trim().to_string();
    if name.is_empty() { "?".into() } else { name.chars().take(120).collect() }
}

fn random_b64(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    getrandom::getrandom(&mut buf).expect("random bytes");
    b64url(&buf)
}

/// Base64url without padding (RFC 7636's encoding).
fn b64url(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (u32::from(chunk[0]) << 16) | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8) | u32::from(*chunk.get(2).unwrap_or(&0));
        out.push(ALPHABET[(n >> 18) as usize & 63] as char);
        out.push(ALPHABET[(n >> 12) as usize & 63] as char);
        if chunk.len() > 1 {
            out.push(ALPHABET[(n >> 6) as usize & 63] as char);
        }
        if chunk.len() > 2 {
            out.push(ALPHABET[n as usize & 63] as char);
        }
    }
    out
}

fn constant_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

// ------------------------------------------------------------ the store

fn entry() -> Option<keyring::Entry> {
    keyring::Entry::new(STORE_SERVICE, STORE_ENTRY).ok()
}

fn store_get() -> Option<String> {
    if settings::isolated() {
        return None;
    }
    entry()?.get_password().ok().filter(|t| !t.is_empty())
}

fn store_set(token: &str) -> Result<(), String> {
    entry().ok_or("no credential store")?.set_password(token).map_err(|e| e.to_string())
}

fn store_delete() {
    if settings::isolated() {
        return;
    }
    if let Some(e) = entry() {
        let _ = e.delete_credential();
    }
}

/// "Reset everything" and the uninstaller: signed out, store emptied.
pub fn forget(settings: &mut Settings) {
    Account::default().sign_out(settings);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_follows_rfc_7636() {
        // RFC 7636, appendix B.
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        assert_eq!(b64url(&Sha256::digest(verifier.as_bytes())), "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
        assert_eq!(random_b64(64).len(), 86);
    }

    #[test]
    fn only_the_callback_is_caught() {
        let s = Settings::default();
        assert!(Account::is_redirect(&s, "https://kynoko.com/oauth/launcher-callback?code=a&state=b"));
        assert!(!Account::is_redirect(&s, "https://kynoko.com/oauth/launcher-callbackX?code=a"));
        assert!(!Account::is_redirect(&s, "https://evil.example/oauth/launcher-callback?code=a"));
    }

    #[test]
    fn the_window_shows_the_platform_only() {
        let s = Settings::default();
        let u = |x: &str| tauri::Url::parse(x).unwrap();
        assert!(may_show(&s, &u("https://kynoko.com/login")));
        assert!(may_show(&s, &u("https://api.kynoko.com/x")));
        assert!(!may_show(&s, &u("http://kynoko.com/login")));
        assert!(!may_show(&s, &u("https://kynoko.com.evil.example/")));
    }

    #[test]
    fn the_consent_carries_pkce_and_the_device() {
        let a = Account::default();
        let url = tauri::Url::parse(&a.begin(&Settings::default(), "0.4.0")).unwrap();
        let q: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(url.as_str().split('?').next(), Some("https://kynoko.com/oauth/authorize"));
        assert_eq!(q["client_id"], CLIENT_ID);
        assert_eq!(q["code_challenge_method"], "S256");
        assert_eq!(q["redirect_uri"], "https://kynoko.com/oauth/launcher-callback");
        assert_eq!(q["state"].len(), 43);
        assert!(a.view(&Settings::default()).pending);
    }

    #[test]
    fn a_redirect_with_another_state_is_refused() {
        let a = Account::default();
        let mut s = Settings::default();
        a.begin(&s, "0.4.0");
        let err = a.finish(&mut s, "https://kynoko.com/oauth/launcher-callback?code=x&state=forged").unwrap_err();
        assert!(err.contains("state"), "{err}");
        assert!(!a.view(&s).pending, "a refused redirect ends the sign-in");
    }

    #[test]
    fn signed_out_the_address_is_untouched() {
        // Signed out, without asking the system's store (a test must not).
        let a = Account { inner: Mutex::new(State { loaded: true, ..State::default() }) };
        let mut s = Settings::default();
        let url = "https://office.kynoko.com/open#kynokoBridge=127.0.0.1:1/t";
        assert_eq!(a.carry(&mut s, url), url);
    }
}
