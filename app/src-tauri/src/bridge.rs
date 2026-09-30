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

use crate::ondisk;

/// A session with no request for this long is over (the page heartbeats every 30 s).
const SESSION_IDLE: Duration = Duration::from_secs(600);

struct Session {
    path: PathBuf,
    origin: String,
    last_seen: Instant,
    /// Told to `on_reached` at the page's first authorized request: proof
    /// that its browser lets that origin reach the launcher.
    key: Option<String>,
    /// The copy written beside the file when it could not be replaced: the
    /// next copies go to it rather than to "copy 2", "copy 3"...
    copy: Option<PathBuf>,
}

#[derive(Clone)]
pub struct Bridge {
    pub port: u16,
    sessions: Arc<Mutex<HashMap<String, Session>>>,
    on_reached: fn(String),
}

impl Bridge {
    /// Binds 127.0.0.1 on a random port and serves in a background thread.
    pub fn start(on_reached: fn(String)) -> io::Result<Bridge> {
        let server = Server::http("127.0.0.1:0").map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
        let port = server
            .server_addr()
            .to_ip()
            .map(|a| a.port())
            .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "no ip listener"))?;
        let bridge = Bridge { port, sessions: Arc::new(Mutex::new(HashMap::new())), on_reached };
        let serving = bridge.clone();
        thread::spawn(move || {
            for request in server.incoming_requests() {
                serving.handle(request);
            }
        });
        Ok(bridge)
    }

    /// Opens a session on `path` for pages of `origin`; returns its token.
    pub fn open(&self, path: PathBuf, origin: String, key: Option<String>) -> String {
        let token = random_token();
        self.sessions
            .lock()
            .expect("sessions lock")
            .insert(token.clone(), Session { path, origin, last_seen: Instant::now(), key, copy: None });
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
        let reached = self.sessions.lock().expect("sessions lock").get_mut(&token).and_then(|s| {
            s.last_seen = Instant::now();
            s.key.take()
        });
        if let Some(key) = reached {
            (self.on_reached)(key);
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

        let (route, query) = rest.split_once('?').unwrap_or((rest, ""));
        match (req.method().clone(), route) {
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
            (Method::Post, "copy") => self.copy(req, &token, &path, &allowed, query),
            (Method::Post, "reveal") => self.reveal(req, &token, &path, &allowed, query),
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
            Err(e) => refused(req, allowed, &e, path),
        }
    }

    /// The page's work, when the file itself cannot take it: written BESIDE
    /// it, under "<name> (<suffix>)" (the page's word for "copy", in its
    /// language), and again to that same copy on the next refusal. Answers
    /// the copy's name and full path, for the page to say where it went.
    fn copy(&self, mut req: Request, token: &str, path: &Path, allowed: &str, query: &str) {
        let suffix = copy_suffix(query_value(query, "suffix").as_deref());
        let kept = self.sessions.lock().expect("sessions lock").get(token).and_then(|s| s.copy.clone());
        let target = kept.filter(|p| p.exists()).unwrap_or_else(|| free_copy_name(path, &suffix));
        let written =
            if target.exists() { write_in_place(&target, req.as_reader()) } else { write_new(&target, req.as_reader()) };
        match written {
            Ok(()) => {
                if let Some(session) = self.sessions.lock().expect("sessions lock").get_mut(token) {
                    session.copy = Some(target.clone());
                }
                let name = target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let body = serde_json::json!({ "name": name, "path": target.display().to_string() }).to_string();
                reply(req, allowed, 200, &body, &[("Content-Type", "application/json".to_string())])
            }
            Err(e) => refused(req, allowed, &e, &target),
        }
    }

    /// Shows the file (`which=file`) or its copy (`which=copy`) selected in the
    /// system's file manager. Only these two: a session names no other path.
    fn reveal(&self, req: Request, token: &str, path: &Path, allowed: &str, query: &str) {
        let target = match query_value(query, "which").as_deref() {
            Some("copy") => self.sessions.lock().expect("sessions lock").get(token).and_then(|s| s.copy.clone()),
            _ => Some(path.to_path_buf()),
        };
        match target.filter(|p| p.exists()) {
            Some(p) => match ondisk::reveal(&p) {
                Ok(()) => reply(req, allowed, 204, "", &[]),
                Err(e) => reply(req, allowed, 500, &format!("cannot show: {e}"), &[]),
            },
            None => reply(req, allowed, 404, "nothing to show", &[]),
        }
    }
}

/// A write the system refused, said so the page can word it: why (locked,
/// read-only, denied, failed), who holds the file when that is the reason,
/// and the system's own message. 409, as before, for older pages.
fn refused(req: Request, allowed: &str, error: &io::Error, path: &Path) {
    let why = ondisk::Refusal::of(error, path);
    let holders = if why == ondisk::Refusal::Locked { ondisk::holders(path) } else { Vec::new() };
    let body = serde_json::json!({ "reason": why.word(), "holders": holders, "detail": error.to_string() }).to_string();
    reply(req, allowed, 409, &body, &[("Content-Type", "application/json".to_string())])
}

/// A file beside `path`, hidden-ish and unique: `.<name>.<random>.<ending>`.
fn sibling(path: &Path, ending: &str) -> PathBuf {
    let dir = path.parent().unwrap_or(Path::new("."));
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    dir.join(format!(".{name}.{}.{ending}", &random_token()[..8]))
}

/// The body goes to a temporary file in the SAME directory, synced, then
/// swaps with the original. A crash leaves the old file or the new one.
fn write_in_place(path: &Path, body: &mut dyn Read) -> io::Result<()> {
    let tmp = sibling(path, "kynoko-tmp");
    let result = (|| {
        let mut out = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        io::copy(body, &mut out)?;
        out.sync_all()?;
        drop(out);
        swap_patiently(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// A file that does not exist yet: written aside, then renamed into place.
fn write_new(path: &Path, body: &mut dyn Read) -> io::Result<()> {
    let tmp = sibling(path, "kynoko-tmp");
    let result = (|| {
        let mut out = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        io::copy(body, &mut out)?;
        out.sync_all()?;
        drop(out);
        fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// Brief holds by other programs pass - an antivirus or the indexer reading
/// the file just written, a sync client, Explorer's preview pane - so the swap
/// is tried again for about two seconds before the file is said to be locked.
fn swap_patiently(tmp: &Path, target: &Path) -> io::Result<()> {
    let mut waits = [100u64, 200, 400, 600, 800].into_iter();
    loop {
        match swap(tmp, target) {
            Err(e) if ondisk::is_lock(&e) => match waits.next() {
                Some(ms) => thread::sleep(Duration::from_millis(ms)),
                None => return Err(e),
            },
            done => return done,
        }
    }
}

/// Windows: ReplaceFileW keeps what a rename loses (ACLs, attributes,
/// alternate data streams, the file's identity for other programs).
///
/// A BACKUP NAME IS GIVEN, and it is what makes every failure recoverable:
/// without one, ReplaceFileW's 1176 means "the original is deleted and the
/// new content is still under the temporary name" - which the caller then
/// deleted. With one, 1175 and 1176 leave both files where they were, and
/// 1177 leaves the original under the backup's name, from where it goes back.
#[cfg(windows)]
fn swap(tmp: &Path, target: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{ReplaceFileW, REPLACEFILE_IGNORE_MERGE_ERRORS};
    let wide = |p: &Path| p.as_os_str().encode_wide().chain(std::iter::once(0)).collect::<Vec<u16>>();
    let backup = sibling(target, "kynoko-old");
    let (target_w, tmp_w, backup_w) = (wide(target), wide(tmp), wide(&backup));
    // SAFETY: the three buffers are NUL-terminated UTF-16 paths that outlive the call.
    let ok = unsafe {
        ReplaceFileW(
            target_w.as_ptr(),
            tmp_w.as_ptr(),
            backup_w.as_ptr(),
            REPLACEFILE_IGNORE_MERGE_ERRORS,
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    if ok != 0 {
        // The original, set aside by the swap, is not needed any more.
        let _ = fs::remove_file(&backup);
        return Ok(());
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(1177) && backup.exists() && !target.exists() {
        let _ = fs::rename(&backup, target);
    }
    Err(error)
}

#[cfg(not(windows))]
fn swap(tmp: &Path, target: &Path) -> io::Result<()> {
    // Carry the permissions over before the rename (extended attributes: TODO).
    if let Ok(meta) = fs::metadata(target) {
        let _ = fs::set_permissions(tmp, meta.permissions());
    }
    fs::rename(tmp, target)
}

/// One `key=value` of a query string, percent-decoded (the page's word for
/// "copy" may be Japanese or Arabic).
fn query_value(query: &str, key: &str) -> Option<String> {
    let raw = query.split('&').find_map(|pair| pair.strip_prefix(key)?.strip_prefix('='))?;
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes
            .get(i + 1..i + 3)
            .and_then(|h| std::str::from_utf8(h).ok())
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        match (bytes[i], hex) {
            (b'%', Some(b)) => {
                out.push(b);
                i += 3;
            }
            (b'+', _) => {
                out.push(b' ');
                i += 1;
            }
            (b, _) => {
                out.push(b);
                i += 1;
            }
        }
    }
    Some(String::from_utf8_lossy(&out).into_owned())
}

/// The page's word for "copy", kept to letters, digits and spaces (it goes
/// into a file name), 24 characters at most; "copy" when it gave none.
fn copy_suffix(raw: Option<&str>) -> String {
    let word: String = raw.unwrap_or("").chars().filter(|c| c.is_alphanumeric() || *c == ' ').take(24).collect();
    let word = word.trim().to_string();
    if word.is_empty() { "copy".to_string() } else { word }
}

/// "<stem> (<suffix>)<ext>" beside `path`, then "(<suffix> 2)"... the first
/// name no file has.
fn free_copy_name(path: &Path, suffix: &str) -> PathBuf {
    let dir = path.parent().unwrap_or(Path::new("."));
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let ext = path.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    for n in 1..100 {
        let label = if n == 1 { suffix.to_string() } else { format!("{suffix} {n}") };
        let candidate = dir.join(format!("{stem} ({label}){ext}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    dir.join(format!("{stem} ({suffix} {}){ext}", &random_token()[..6]))
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

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kynoko-bridge-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Only the file, and whatever the test itself added: no temporary or
    /// backup file is ever left behind.
    fn names(dir: &Path) -> Vec<String> {
        let mut all: Vec<String> =
            fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        all.sort();
        all
    }

    #[test]
    fn writes_in_place_and_leaves_nothing_behind() {
        let dir = scratch("write");
        let file = dir.join("notes.txt");
        fs::write(&file, "old").unwrap();
        write_in_place(&file, &mut "new".as_bytes()).unwrap();
        assert_eq!(fs::read_to_string(&file).unwrap(), "new");
        assert_eq!(names(&dir), ["notes.txt"]);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn copies_are_named_beside_the_file_and_count_up() {
        let dir = scratch("copies");
        let file = dir.join("notes-audience.txt");
        fs::write(&file, "x").unwrap();
        assert_eq!(free_copy_name(&file, "copie"), dir.join("notes-audience (copie).txt"));
        fs::write(dir.join("notes-audience (copie).txt"), "x").unwrap();
        assert_eq!(free_copy_name(&file, "copie"), dir.join("notes-audience (copie 2).txt"));
        assert_eq!(free_copy_name(&dir.join("Makefile"), "copy"), dir.join("Makefile (copy)"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_copy_suffix_is_a_word_and_nothing_else() {
        assert_eq!(copy_suffix(Some("copie")), "copie");
        assert_eq!(copy_suffix(Some(r"../..\co/pie")), "copie");
        assert_eq!(copy_suffix(Some("コピー")), "コピー");
        assert_eq!(copy_suffix(Some("   ")), "copy");
        assert_eq!(copy_suffix(None), "copy");
        assert_eq!(query_value("which=copy&suffix=%E3%82%B3%E3%83%94%E3%83%BC", "suffix").as_deref(), Some("コピー"));
        assert_eq!(query_value("suffix=copie+2", "suffix").as_deref(), Some("copie 2"));
        assert_eq!(query_value("suffix=%zz", "suffix").as_deref(), Some("%zz"));
        assert_eq!(query_value("which=copy", "suffix"), None);
    }

    /// The whole route, over HTTP as the page speaks it: a held file refuses
    /// with a JSON 409 naming who holds it, the work goes beside it, and the
    /// next refusal writes that same copy again.
    #[cfg(windows)]
    #[test]
    fn a_refused_save_goes_beside_the_file() {
        use std::os::windows::fs::OpenOptionsExt;
        let dir = scratch("beside");
        let file = dir.join("notes-audience.txt");
        fs::write(&file, "old").unwrap();
        let bridge = Bridge::start(|_| {}).unwrap();
        let origin = "https://office.kynoko.com".to_string();
        let token = bridge.open(file.clone(), origin.clone(), None);
        let base = format!("http://127.0.0.1:{}/s/{token}", bridge.port);
        let (tag, _) = etag(&file).unwrap();
        let held = OpenOptions::new().read(true).share_mode(0).open(&file).unwrap();

        match ureq::put(&format!("{base}/content")).set("Origin", &origin).set("If-Match", &tag).send_string("new") {
            Err(ureq::Error::Status(409, answer)) => {
                let body: serde_json::Value = answer.into_json().unwrap();
                assert_eq!(body["reason"], "locked");
                assert!(!body["holders"].as_array().unwrap().is_empty(), "{body}");
            }
            other => panic!("expected a 409, got {other:?}"),
        }
        let copy = |text: &str| -> serde_json::Value {
            ureq::post(&format!("{base}/copy?suffix=copie"))
                .set("Origin", &origin)
                .send_string(text)
                .unwrap()
                .into_json()
                .unwrap()
        };
        let first = copy("new");
        assert_eq!(first["name"], "notes-audience (copie).txt");
        assert!(first["path"].as_str().unwrap().ends_with("notes-audience (copie).txt"));
        assert_eq!(fs::read_to_string(dir.join("notes-audience (copie).txt")).unwrap(), "new");
        let second = copy("newer");
        assert_eq!(second["name"], "notes-audience (copie).txt");
        assert_eq!(fs::read_to_string(dir.join("notes-audience (copie).txt")).unwrap(), "newer");
        // Another origin cannot use the session, nor reach the copy.
        assert!(matches!(
            ureq::post(&format!("{base}/copy")).set("Origin", "https://evil.example").send_string("x"),
            Err(ureq::Error::Status(403, _))
        ));
        drop(held);
        assert_eq!(fs::read_to_string(&file).unwrap(), "old");
        assert_eq!(names(&dir), ["notes-audience (copie).txt", "notes-audience.txt"]);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn a_brief_hold_is_waited_out() {
        use std::os::windows::fs::OpenOptionsExt;
        let dir = scratch("brief");
        let file = dir.join("held.txt");
        fs::write(&file, "old").unwrap();
        let held = OpenOptions::new().read(true).share_mode(0).open(&file).unwrap();
        let release = thread::spawn(move || {
            thread::sleep(Duration::from_millis(300));
            drop(held);
        });
        write_in_place(&file, &mut "new".as_bytes()).unwrap();
        release.join().unwrap();
        assert_eq!(fs::read_to_string(&file).unwrap(), "new");
        assert_eq!(names(&dir), ["held.txt"]);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn a_lasting_hold_is_reported_and_the_file_kept() {
        use std::os::windows::fs::OpenOptionsExt;
        let dir = scratch("lasting");
        let file = dir.join("held.txt");
        fs::write(&file, "old").unwrap();
        let held = OpenOptions::new().read(true).share_mode(0).open(&file).unwrap();
        let error = write_in_place(&file, &mut "new".as_bytes()).unwrap_err();
        assert_eq!(ondisk::Refusal::of(&error, &file), ondisk::Refusal::Locked);
        assert!(!ondisk::holders(&file).is_empty());
        drop(held);
        assert_eq!(fs::read_to_string(&file).unwrap(), "old");
        assert_eq!(names(&dir), ["held.txt"]);
        fs::remove_dir_all(&dir).unwrap();
    }
}
