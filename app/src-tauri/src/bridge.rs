//! The loopback bridge (docs/SPEC.md, section 8): one listener on
//! 127.0.0.1, one session per opened file, each bound to one origin by a
//! 256-bit token. Proven in spikes/loopback-bridge; this is its production
//! form (several sessions, expiry, safe in-place writes).

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, UNIX_EPOCH};

use tiny_http::{Header, Method, Request, Response, Server, StatusCode};

/// A session with no request for this long is over (the page heartbeats every 30 s).
const SESSION_IDLE: Duration = Duration::from_secs(600);

struct Session {
    path: PathBuf,
    origin: String,
    last_seen: Instant,
}

#[derive(Clone)]
pub struct Bridge {
    pub port: u16,
    sessions: Arc<Mutex<HashMap<String, Session>>>,
}

impl Bridge {
    /// Binds 127.0.0.1 on a random port and serves in a background thread.
    pub fn start() -> io::Result<Bridge> {
        let server = Server::http("127.0.0.1:0").map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
        let port = server
            .server_addr()
            .to_ip()
            .map(|a| a.port())
            .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "no ip listener"))?;
        let bridge = Bridge { port, sessions: Arc::new(Mutex::new(HashMap::new())) };
        let serving = bridge.clone();
        thread::spawn(move || {
            for request in server.incoming_requests() {
                serving.handle(request);
            }
        });
        Ok(bridge)
    }

    /// Opens a session on `path` for pages of `origin`; returns its token.
    pub fn open(&self, path: PathBuf, origin: String) -> String {
        let token = random_token();
        self.sessions
            .lock()
            .expect("sessions lock")
            .insert(token.clone(), Session { path, origin, last_seen: Instant::now() });
        token
    }

    /// Sessions still alive (expired ones are dropped on the way).
    pub fn live(&self) -> usize {
        let mut sessions = self.sessions.lock().expect("sessions lock");
        sessions.retain(|_, s| s.last_seen.elapsed() < SESSION_IDLE);
        sessions.len()
    }

    fn handle(&self, req: Request) {
        let host = header(&req, "Host");
        let origin = header(&req, "Origin");
        if host.as_deref() != Some(format!("127.0.0.1:{}", self.port).as_str()) {
            let _ = req.respond(Response::from_string("misdirected host").with_status_code(421));
            return;
        }
        let url = req.url().to_string();
        let mut parts = url.trim_start_matches('/').splitn(3, '/');
        let (scope, token, rest) = (parts.next(), parts.next().unwrap_or(""), parts.next().unwrap_or(""));

        // Find the session, and check the origin against IT: a token is only
        // ever valid for the app it was opened for.
        let found = {
            let sessions = self.sessions.lock().expect("sessions lock");
            sessions
                .iter()
                .find(|(t, _)| same(t, token))
                .map(|(t, s)| (t.clone(), s.path.clone(), s.origin.clone()))
        };
        let Some((token, path, allowed)) = found.filter(|_| scope == Some("s")) else {
            // Unknown token: nothing to say, and no CORS header to read it with.
            let _ = req.respond(Response::from_string("not found").with_status_code(404));
            return;
        };
        if origin.as_deref() != Some(allowed.as_str()) {
            let _ = req.respond(Response::from_string("forbidden origin").with_status_code(403));
            return;
        }
        if let Some(s) = self.sessions.lock().expect("sessions lock").get_mut(&token) {
            s.last_seen = Instant::now();
        }

        if *req.method() == Method::Options {
            let mut extra = vec![
                ("Access-Control-Allow-Methods", "GET, PUT, POST, DELETE".to_string()),
                ("Access-Control-Allow-Headers", "If-Match, Content-Type".to_string()),
                ("Access-Control-Max-Age", "600".to_string()),
            ];
            if header(&req, "Access-Control-Request-Private-Network").is_some() {
                extra.push(("Access-Control-Allow-Private-Network", "true".to_string()));
            }
            return reply(req, &allowed, 204, "", &extra);
        }

        match (req.method().clone(), rest) {
            (Method::Get, "meta") => match etag(&path) {
                Ok((tag, size)) => {
                    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    let body = serde_json::json!({ "name": name, "size": size, "etag": tag }).to_string();
                    reply(req, &allowed, 200, &body, &[("Content-Type", "application/json".to_string())])
                }
                Err(e) => reply(req, &allowed, 410, &format!("file gone: {e}"), &[]),
            },
            (Method::Get, "content") => match (File::open(&path), etag(&path)) {
                (Ok(file), Ok((tag, size))) => {
                    let headers = cors(&allowed, &[("ETag", tag), ("Content-Type", "application/octet-stream".into())]);
                    let _ = req.respond(Response::new(StatusCode(200), headers, file, Some(size as usize), None));
                }
                (Err(e), _) | (_, Err(e)) => reply(req, &allowed, 410, &format!("file gone: {e}"), &[]),
            },
            (Method::Put, "content") => self.put(req, &path, &allowed),
            (Method::Post, "heartbeat") => reply(req, &allowed, 204, "", &[]),
            (Method::Delete, "") => {
                self.sessions.lock().expect("sessions lock").remove(&token);
                reply(req, &allowed, 204, "", &[])
            }
            _ => reply(req, &allowed, 405, "method not allowed", &[]),
        }
    }

    fn put(&self, mut req: Request, path: &Path, allowed: &str) {
        let current = etag(path).map(|(t, _)| t).unwrap_or_default();
        if header(&req, "If-Match").as_deref() != Some(current.as_str()) {
            return reply(req, allowed, 412, "changed on disk", &[("ETag", current)]);
        }
        match write_in_place(path, req.as_reader()) {
            Ok(()) => {
                let tag = etag(path).map(|(t, _)| t).unwrap_or_default();
                let body = serde_json::json!({ "etag": tag }).to_string();
                reply(req, allowed, 200, &body, &[("ETag", tag), ("Content-Type", "application/json".to_string())])
            }
            Err(e) => reply(req, allowed, 409, &format!("cannot write: {e}"), &[]),
        }
    }
}

/// The body goes to a temporary file in the SAME directory, synced, then
/// swaps with the original. A crash leaves the old file or the new one.
fn write_in_place(path: &Path, body: &mut dyn Read) -> io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = dir.join(format!(".{name}.{}.kynoko-tmp", &random_token()[..8]));
    let result = (|| {
        let mut out = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        io::copy(body, &mut out)?;
        out.sync_all()?;
        drop(out);
        swap(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// Windows: ReplaceFileW keeps what a rename loses (ACLs, attributes,
/// alternate data streams, the file's identity for other programs).
#[cfg(windows)]
fn swap(tmp: &Path, target: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{ReplaceFileW, REPLACEFILE_IGNORE_MERGE_ERRORS};
    let wide = |p: &Path| p.as_os_str().encode_wide().chain(std::iter::once(0)).collect::<Vec<u16>>();
    let (target_w, tmp_w) = (wide(target), wide(tmp));
    // SAFETY: both buffers are NUL-terminated UTF-16 paths that outlive the call.
    let ok = unsafe {
        ReplaceFileW(
            target_w.as_ptr(),
            tmp_w.as_ptr(),
            std::ptr::null(),
            REPLACEFILE_IGNORE_MERGE_ERRORS,
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    if ok != 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(not(windows))]
fn swap(tmp: &Path, target: &Path) -> io::Result<()> {
    // Carry the permissions over before the rename (extended attributes: TODO).
    if let Ok(meta) = fs::metadata(target) {
        let _ = fs::set_permissions(tmp, meta.permissions());
    }
    fs::rename(tmp, target)
}

fn etag(path: &Path) -> io::Result<(String, u64)> {
    let meta = fs::metadata(path)?;
    let mtime = meta.modified()?.duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    Ok((format!("\"{mtime:x}-{:x}\"", meta.len()), meta.len()))
}

fn cors(origin: &str, extra: &[(&str, String)]) -> Vec<Header> {
    let mut all = vec![
        ("Access-Control-Allow-Origin", origin.to_string()),
        ("Access-Control-Expose-Headers", "ETag".to_string()),
        ("Vary", "Origin".to_string()),
        ("Cache-Control", "no-store".to_string()),
    ];
    all.extend(extra.iter().cloned());
    all.into_iter()
        .map(|(k, v)| Header::from_bytes(k.as_bytes(), v.as_bytes()).expect("valid header"))
        .collect()
}

fn reply(req: Request, origin: &str, status: u16, body: &str, extra: &[(&str, String)]) {
    let mut response = Response::from_string(body).with_status_code(status);
    for h in cors(origin, extra) {
        response.add_header(h);
    }
    let _ = req.respond(response);
}

fn header(req: &Request, name: &'static str) -> Option<String> {
    req.headers().iter().find(|h| h.field.equiv(name)).map(|h| h.value.as_str().to_string())
}

fn random_token() -> String {
    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes).expect("os randomness");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Constant-time comparison: the token must not leak through response timing.
fn same(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}
