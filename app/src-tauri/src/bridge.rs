//! The loopback bridge (docs/SPEC.md, section 8): one listener on
//! 127.0.0.1, one session per opened file, each bound to one origin by a
//! 256-bit token. Proven in spikes/loopback-bridge; this is its production
//! form (several sessions, expiry, safe in-place writes).
//!
//! Large files (a project that embeds its media weighs gigabytes): the page
//! reads the parts it needs (`Range`), and saves by adding its changed parts
//! and a new directory at the END of the file (`append`), so that nothing
//! already on disk is read or written again. "Save as" goes where the user
//! picks in the system's own dialog: the page never names a path.

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, UNIX_EPOCH};

use tiny_http::{Header, Method, Request, Response, Server, StatusCode};

use crate::ondisk;

/// A session with no request for this long is over (the page heartbeats every 30 s).
const SESSION_IDLE: Duration = Duration::from_secs(600);

/// How long a held file is waited for, in steps, before a write is refused
/// (about two seconds in all; see swap_patiently).
const PATIENCE_MS: [u64; 5] = [100, 200, 400, 600, 800];

/// What the bridge offers besides reading and writing a file whole, listed
/// in `meta` for a page to choose how it reads and saves (a launcher from
/// before them answers 405, or ignores `Range`).
const FEATURES: [&str; 3] = ["range", "append", "save-as"];

/// What the system's "Save as" dialog is asked to show: the folder it opens
/// in, the name it suggests, the file type it offers (without its dot).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveAsk {
    pub folder: Option<PathBuf>,
    pub name: String,
    pub ext: Option<String>,
}

/// Shows the system's "Save as" dialog and answers the path the user picked,
/// or None when they cancelled. The launcher's is dialog::save_file; tests
/// answer in the user's place.
pub type Chooser = Arc<dyn Fn(&SaveAsk) -> Option<PathBuf> + Send + Sync>;

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
    choose: Chooser,
}

impl Bridge {
    /// Binds 127.0.0.1 on a random port and serves in a background thread.
    /// `choose` shows the "Save as" dialog (see save_as).
    pub fn start(on_reached: fn(String), choose: Chooser) -> io::Result<Bridge> {
        let server = Server::http("127.0.0.1:0").map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
        let port = server
            .server_addr()
            .to_ip()
            .map(|a| a.port())
            .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "no ip listener"))?;
        let bridge = Bridge { port, sessions: Arc::new(Mutex::new(HashMap::new())), on_reached, choose };
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
                ("Access-Control-Allow-Headers", "If-Match, Content-Type, Range".to_string()),
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
                    let name = file_name(&path);
                    let body = serde_json::json!({ "name": name, "size": size, "etag": tag, "features": FEATURES });
                    let body = body.to_string();
                    reply(req, &allowed, 200, &body, &[("Content-Type", "application/json".to_string())])
                }
                Err(e) => reply(req, &allowed, 410, &format!("file gone: {e}"), &[]),
            },
            (Method::Get, "content") => self.content(req, &path, &allowed),
            (Method::Put, "content") => self.put(req, &path, &allowed),
            (Method::Post, "append") => self.append(req, &path, &allowed),
            (Method::Post, "save-as") => {
                // The dialog waits on the user: on a thread of its own, so
                // that the other requests (heartbeats, other files) do not.
                let (bridge, query) = (self.clone(), query.to_string());
                thread::spawn(move || bridge.save_as(req, &path, &allowed, &query));
            }
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
            drain(&mut req);
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

    /// The file, whole (200), or the one range of it the page asked for (206),
    /// streamed from disk either way, with its length. The tag and the size
    /// come from the open file itself, so they describe the bytes sent; a page
    /// reading a file in parts compares each answer's `ETag` with the one it
    /// started from (If-Range is not supported).
    fn content(&self, req: Request, path: &Path, allowed: &str) {
        let opened = File::open(path).and_then(|file| {
            let meta = file.metadata()?;
            Ok((file, tag_of(&meta)?, meta.len()))
        });
        let (mut file, tag, size) = match opened {
            Ok(found) => found,
            Err(e) => return reply(req, allowed, 410, &format!("file gone: {e}"), &[]),
        };
        let mut headers = vec![("ETag", tag), ("Accept-Ranges", "bytes".to_string())];
        match slice(header(&req, "Range").as_deref(), size) {
            Slice::Whole => {
                headers.push(("Content-Type", "application/octet-stream".to_string()));
                stream(req, allowed, 200, &headers, file, size)
            }
            Slice::Part(first, last) => match file.seek(SeekFrom::Start(first)) {
                Ok(_) => {
                    headers.push(("Content-Type", "application/octet-stream".to_string()));
                    headers.push(("Content-Range", format!("bytes {first}-{last}/{size}")));
                    stream(req, allowed, 206, &headers, file, last - first + 1)
                }
                Err(e) => reply(req, allowed, 410, &format!("file gone: {e}"), &[]),
            },
            Slice::Unsatisfiable => {
                headers.push(("Content-Range", format!("bytes */{size}")));
                reply(req, allowed, 416, "range not satisfiable", &headers)
            }
        }
    }

    /// The body added at the END of the file, in place: how a page saves a
    /// large container (its changed parts, then a new directory) without
    /// writing again what is already on disk. If-Match as for PUT, checked
    /// again once the file is locked (see append_in_place).
    fn append(&self, mut req: Request, path: &Path, allowed: &str) {
        let current = etag(path).map(|(t, _)| t).unwrap_or_default();
        let Some(expected) = header(&req, "If-Match").filter(|asked| *asked == current) else {
            drain(&mut req);
            return reply(req, allowed, 412, "changed on disk", &[("ETag", current)]);
        };
        match append_in_place(path, &expected, req.as_reader()) {
            Ok(Appended::Done) => {
                let (tag, size) = etag(path).unwrap_or_default();
                let body = serde_json::json!({ "etag": tag, "size": size }).to_string();
                reply(req, allowed, 200, &body, &[("ETag", tag), ("Content-Type", "application/json".to_string())])
            }
            Ok(Appended::Changed(tag)) => {
                drain(&mut req);
                reply(req, allowed, 412, "changed on disk", &[("ETag", tag)])
            }
            Err(e) => refused(req, allowed, &e, path),
        }
    }

    /// "Save as": the system's own dialog (dialog.rs), opened on the file's
    /// folder with the name the page suggests and the file's type; the body
    /// is written where the user picked, the safe way (see write_in_place),
    /// and a NEW session is opened on that file for the same origin. The page
    /// suggests a name, never a path, and learns only the new file's name.
    /// Runs on a thread of its own (see handle).
    fn save_as(&self, mut req: Request, path: &Path, allowed: &str, query: &str) {
        let ask = SaveAsk {
            folder: path.parent().map(Path::to_path_buf),
            name: suggested_name(query_value(query, "name").as_deref(), path),
            ext: path.extension().map(|e| e.to_string_lossy().into_owned()).filter(|e| !e.is_empty()),
        };
        let chosen = {
            // One dialog at a time: a second waits for the first to close.
            static TURN: Mutex<()> = Mutex::new(());
            let _turn = TURN.lock().unwrap_or_else(|e| e.into_inner());
            (self.choose)(&ask)
        };
        let Some(target) = chosen else {
            drain(&mut req);
            let body = serde_json::json!({ "cancelled": true }).to_string();
            return reply(req, allowed, 200, &body, &[("Content-Type", "application/json".to_string())]);
        };
        let body = req.as_reader();
        let written = if target.exists() { write_in_place(&target, body) } else { write_new(&target, body) };
        match written {
            Ok(()) => {
                let tag = etag(&target).map(|(t, _)| t).unwrap_or_default();
                let name = file_name(&target);
                let token = self.open(target, allowed.to_string(), None);
                let body = serde_json::json!({ "token": token, "name": name, "etag": tag }).to_string();
                reply(req, allowed, 200, &body, &[("ETag", tag), ("Content-Type", "application/json".to_string())])
            }
            Err(e) => refused(req, allowed, &e, &target),
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
fn refused(mut req: Request, allowed: &str, error: &io::Error, path: &Path) {
    drain(&mut req);
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
        pour(body, &mut out)?;
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
        pour(body, &mut out)?;
        out.sync_all()?;
        drop(out);
        fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// What an append found once it held the file.
#[derive(Debug, PartialEq, Eq)]
enum Appended {
    Done,
    /// The file is no longer the version the page named (its tag now):
    /// nothing was written.
    Changed(String),
}

/// Adds `body` at the end of `path` IN PLACE: no temporary copy and no
/// swap, which would write gigabytes again to add a few megabytes. The file
/// is locked exclusively first (open_locked), its tag checked against
/// `expected` under the lock, and the new bytes synced before the answer.
///
/// A failure while writing (a full disk, a page gone mid-request) cuts the
/// file back to its length and its date, so that the page's tag still names
/// it. A crash, or a power cut, can leave the old content whole followed by
/// a torn tail: by design, the apps' container formats read the last
/// complete directory and ignore what follows it.
fn append_in_place(path: &Path, expected: &str, body: &mut dyn Read) -> io::Result<Appended> {
    let mut file = open_locked(path)?;
    let before = file.metadata()?;
    let tag = tag_of(&before)?;
    if tag != expected {
        return Ok(Appended::Changed(tag));
    }
    let end = file.seek(SeekFrom::End(0))?;
    let written = pour(body, &mut file).and_then(|_| file.sync_all());
    if let Err(e) = written {
        let _ = file.set_len(end);
        if let Ok(modified) = before.modified() {
            let _ = file.set_modified(modified);
        }
        let _ = file.sync_all();
        return Err(e);
    }
    // The lock goes with the handle.
    Ok(Appended::Done)
}

/// `path` opened to write, under an exclusive lock, held files waited for as
/// a swap waits for them (PATIENCE_MS). Windows: nobody else may write while
/// it is open (share mode), and the lock keeps other programs' reads out
/// too until the append is over. macOS and Linux: an advisory lock (flock),
/// which the programs that lock files respect.
fn open_locked(path: &Path) -> io::Result<File> {
    let mut waits = PATIENCE_MS.into_iter();
    loop {
        match try_open_locked(path) {
            Err(e) if ondisk::is_lock(&e) => match waits.next() {
                Some(ms) => thread::sleep(Duration::from_millis(ms)),
                None => return Err(e),
            },
            done => return done,
        }
    }
}

fn try_open_locked(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ);
    }
    let file = options.open(path)?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(fs::TryLockError::WouldBlock) => Err(ondisk::held_lock()),
        Err(fs::TryLockError::Error(e)) => Err(e),
    }
}

/// The request's body into `out`, a megabyte at a time (a project file may
/// weigh gigabytes). Answers how many bytes.
fn pour(body: &mut dyn Read, out: &mut File) -> io::Result<u64> {
    let mut buffered = BufWriter::with_capacity(1 << 20, out);
    let poured = io::copy(body, &mut buffered)?;
    buffered.flush()?;
    Ok(poured)
}

/// Reads what is left of a request's body and throws it away, BEFORE the
/// answer. tiny_http would do it after, in a single buffer the size of what
/// is left: gigabytes, for a large file refused or a "Save as" cancelled.
fn drain(req: &mut Request) {
    let _ = io::copy(req.as_reader(), &mut io::sink());
}

/// Brief holds by other programs pass - an antivirus or the indexer reading
/// the file just written, a sync client, Explorer's preview pane - so the swap
/// is tried again for about two seconds before the file is said to be locked.
fn swap_patiently(tmp: &Path, target: &Path) -> io::Result<()> {
    let mut waits = PATIENCE_MS.into_iter();
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

/// The name the "Save as" dialog suggests: the page's, kept to a plain file
/// name (no folder, nothing a system refuses in a name, 120 characters at
/// most), with the session file's extension unless it already ends with it;
/// the session file's own name when the page gave none. The user can still
/// change it: the dialog, not the page, decides.
fn suggested_name(raw: Option<&str>, session: &Path) -> String {
    let ext = session.extension().map(|e| e.to_string_lossy().into_owned()).filter(|e| !e.is_empty());
    let kept: String =
        raw.unwrap_or("").chars().filter(|c| !c.is_control() && !r#"<>:"/\|?*"#.contains(*c)).take(120).collect();
    let kept = kept.trim().trim_start_matches('.').trim_end_matches(['.', ' ']).trim();
    if kept.is_empty() {
        return file_name(session);
    }
    // Names Windows keeps for devices, whatever their extension.
    let stem = kept.split('.').next().unwrap_or("").trim_end().to_ascii_uppercase();
    let numbered = |prefix| stem.len() == 4 && stem.starts_with(prefix) && stem.as_bytes()[3].is_ascii_digit();
    let device = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL") || numbered("COM") || numbered("LPT");
    let mut name = if device { format!("_{kept}") } else { kept.to_string() };
    if let Some(ext) = ext {
        if !name.to_lowercase().ends_with(&format!(".{}", ext.to_lowercase())) {
            name = format!("{name}.{ext}");
        }
    }
    name
}

/// What a `Range` request header asks of a file of `size` bytes.
#[derive(Debug, PartialEq, Eq)]
enum Slice {
    /// No range, or one served whole: several ranges, another unit, a
    /// malformed one (ignored, as HTTP allows). 200.
    Whole,
    /// Bytes `first..=last`, within the file. 206.
    Part(u64, u64),
    /// Nothing of the file: a start at or past its end, an empty suffix, an
    /// empty file. 416.
    Unsatisfiable,
}

/// `bytes=a-b`, `bytes=a-` or `bytes=-n` (the last n bytes), one range only.
fn slice(range: Option<&str>, size: u64) -> Slice {
    let Some((unit, set)) = range.and_then(|r| r.split_once('=')) else { return Slice::Whole };
    if !unit.trim().eq_ignore_ascii_case("bytes") || set.contains(',') {
        return Slice::Whole;
    }
    let Some((first, last)) = set.trim().split_once('-') else { return Slice::Whole };
    let number = |s: &str| {
        let s = s.trim();
        // Past u64: further than any file, which is all it can mean.
        (!s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())).then(|| s.parse::<u64>().unwrap_or(u64::MAX))
    };
    match (first.trim().is_empty(), number(first), number(last)) {
        // The last n bytes (all of them when the file is shorter).
        (true, _, Some(n)) if n > 0 && size > 0 => Slice::Part(size - n.min(size), size - 1),
        (true, _, Some(_)) => Slice::Unsatisfiable,
        (false, Some(a), None) if last.trim().is_empty() => {
            if a < size { Slice::Part(a, size - 1) } else { Slice::Unsatisfiable }
        }
        (false, Some(a), Some(b)) if a <= b => {
            if a < size { Slice::Part(a, b.min(size - 1)) } else { Slice::Unsatisfiable }
        }
        _ => Slice::Whole,
    }
}

/// `len` bytes of `body`, from where it stands, as the response, with their
/// length (never chunked: the page can follow its progress).
fn stream(req: Request, origin: &str, status: u16, extra: &[(&str, String)], body: File, len: u64) {
    let response = Response::new(StatusCode(status), cors(origin, extra), body.take(len), Some(len as usize), None)
        .with_chunked_threshold(usize::MAX);
    let _ = req.respond(response);
}

fn file_name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

fn etag(path: &Path) -> io::Result<(String, u64)> {
    let meta = fs::metadata(path)?;
    Ok((tag_of(&meta)?, meta.len()))
}

/// `"<mtime ns hex>-<size hex>"`.
fn tag_of(meta: &fs::Metadata) -> io::Result<String> {
    let mtime = meta.modified()?.duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    Ok(format!("\"{mtime:x}-{:x}\"", meta.len()))
}

fn cors(origin: &str, extra: &[(&str, String)]) -> Vec<Header> {
    let mut all = vec![
        ("Access-Control-Allow-Origin", origin.to_string()),
        ("Access-Control-Expose-Headers", "ETag, Content-Range, Accept-Ranges, Content-Length".to_string()),
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
        let bridge = Bridge::start(|_| {}, Arc::new(|_: &SaveAsk| None)).unwrap();
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

    /* Large files: ranges, appends, "Save as". */

    const ORIGIN: &str = "https://media.kynoko.com";

    /// A bridge with one session on `file` for ORIGIN, its "Save as" dialog
    /// answered by `choose`; the session's address.
    fn serve(file: &Path, choose: Chooser) -> (Bridge, String) {
        let bridge = Bridge::start(|_| {}, choose).unwrap();
        let token = bridge.open(file.to_path_buf(), ORIGIN.to_string(), None);
        let base = format!("http://127.0.0.1:{}/s/{token}", bridge.port);
        (bridge, base)
    }

    fn never_asked() -> Chooser {
        Arc::new(|ask: &SaveAsk| panic!("no dialog expected, asked {ask:?}"))
    }

    /// Status and response, whatever the status.
    fn answer(sent: Result<ureq::Response, ureq::Error>) -> (u16, ureq::Response) {
        match sent {
            Ok(r) => (r.status(), r),
            Err(ureq::Error::Status(code, r)) => (code, r),
            Err(e) => panic!("no answer: {e}"),
        }
    }

    fn bytes(r: ureq::Response) -> Vec<u8> {
        let mut all = Vec::new();
        r.into_reader().read_to_end(&mut all).unwrap();
        all
    }

    fn json(r: ureq::Response) -> serde_json::Value {
        r.into_json().unwrap()
    }

    #[test]
    fn ranges_are_read_as_http_says() {
        use Slice::*;
        let at = |r: &str, size| slice(Some(r), size);
        assert_eq!(slice(None, 1000), Whole);
        assert_eq!(at("bytes=0-99", 1000), Part(0, 99));
        assert_eq!(at("bytes=990-2000", 1000), Part(990, 999));
        assert_eq!(at("bytes=500-", 1000), Part(500, 999));
        assert_eq!(at("bytes=999-999", 1000), Part(999, 999));
        assert_eq!(at("BYTES = 0-0", 1000), Part(0, 0));
        // The last n bytes, all of them when the file is shorter.
        assert_eq!(at("bytes=-100", 1000), Part(900, 999));
        assert_eq!(at("bytes=-5000", 1000), Part(0, 999));
        // Nothing of the file.
        assert_eq!(at("bytes=1000-", 1000), Unsatisfiable);
        assert_eq!(at("bytes=1000-1001", 1000), Unsatisfiable);
        assert_eq!(at("bytes=99999999999999999999999-", 1000), Unsatisfiable);
        assert_eq!(at("bytes=-0", 1000), Unsatisfiable);
        assert_eq!(at("bytes=0-", 0), Unsatisfiable);
        assert_eq!(at("bytes=-1", 0), Unsatisfiable);
        // Served whole: malformed, several ranges, another unit.
        assert_eq!(at("bytes=5-3", 1000), Whole);
        assert_eq!(at("bytes=0-1,5-6", 1000), Whole);
        assert_eq!(at("items=0-1", 1000), Whole);
        assert_eq!(at("bytes=x-1", 1000), Whole);
        assert_eq!(at("bytes=-", 1000), Whole);
        assert_eq!(at("bytes=", 1000), Whole);
        assert_eq!(at("bytes", 1000), Whole);
        assert_eq!(at("bytes=1-2-3", 1000), Whole);
    }

    /// Over HTTP, as a page reads a container: one range at a time, each
    /// with the file's tag; the whole file still answers 200, now with its
    /// length; the preflight lets `Range` through and the page read the
    /// range's headers.
    #[test]
    fn a_page_reads_a_file_in_parts() {
        let dir = scratch("ranges");
        let file = dir.join("film.kproj");
        let content: Vec<u8> = (0..1000u32).map(|i| (i % 251) as u8).collect();
        fs::write(&file, &content).unwrap();
        let (tag, _) = etag(&file).unwrap();
        let (_bridge, base) = serve(&file, never_asked());
        let get = |range: Option<&str>| {
            let mut request = ureq::get(&format!("{base}/content")).set("Origin", ORIGIN);
            if let Some(range) = range {
                request = request.set("Range", range);
            }
            answer(request.call())
        };

        let (status, r) = get(Some("bytes=10-19"));
        assert_eq!(status, 206);
        assert_eq!(r.header("Content-Range"), Some("bytes 10-19/1000"));
        assert_eq!(r.header("Content-Length"), Some("10"));
        assert_eq!(r.header("Accept-Ranges"), Some("bytes"));
        assert_eq!(r.header("ETag"), Some(tag.as_str()));
        assert!(r.header("Access-Control-Expose-Headers").unwrap().contains("Content-Range"));
        assert_eq!(bytes(r), &content[10..20]);

        let (status, r) = get(Some("bytes=-5"));
        assert_eq!(status, 206);
        assert_eq!(r.header("Content-Range"), Some("bytes 995-999/1000"));
        assert_eq!(bytes(r), &content[995..]);

        let (status, r) = get(Some("bytes=600-"));
        assert_eq!((status, r.header("Content-Range")), (206, Some("bytes 600-999/1000")));
        assert_eq!(bytes(r), &content[600..]);

        let (status, r) = get(Some("bytes=1000-"));
        assert_eq!(status, 416);
        assert_eq!(r.header("Content-Range"), Some("bytes */1000"));
        assert_eq!(r.header("ETag"), Some(tag.as_str()));

        for whole in [None, Some("bytes=0-1,5-6"), Some("bytes=9-2")] {
            let (status, r) = get(whole);
            assert_eq!(status, 200, "{whole:?}");
            assert_eq!(r.header("Accept-Ranges"), Some("bytes"));
            assert_eq!(r.header("Content-Length"), Some("1000"));
            assert_eq!(r.header("Content-Range"), None);
            assert_eq!(bytes(r), content);
        }

        // A file larger than tiny_http's chunking threshold still says its length.
        let big = vec![7u8; 100_000];
        fs::write(&file, &big).unwrap();
        let (status, r) = get(None);
        assert_eq!((status, r.header("Content-Length")), (200, Some("100000")));
        assert_eq!(bytes(r).len(), 100_000);

        let (status, r) = answer(
            ureq::request("OPTIONS", &format!("{base}/content"))
                .set("Origin", ORIGIN)
                .set("Access-Control-Request-Method", "GET")
                .set("Access-Control-Request-Headers", "range")
                .call(),
        );
        assert_eq!(status, 204);
        assert!(r.header("Access-Control-Allow-Headers").unwrap().contains("Range"));

        let (status, r) = answer(ureq::get(&format!("{base}/meta")).set("Origin", ORIGIN).call());
        assert_eq!(status, 200);
        assert_eq!(json(r)["features"], serde_json::json!(["range", "append", "save-as"]));

        // Another origin reads nothing, not even a part.
        let foreign = ureq::get(&format!("{base}/content")).set("Origin", "https://evil.example");
        let (status, _) = answer(foreign.set("Range", "bytes=0-9").call());
        assert_eq!(status, 403);
        fs::remove_dir_all(&dir).unwrap();
    }

    /// An append adds to the end of the very file (a hard link to it sees the
    /// new bytes: no copy was swapped in), answers the new tag and size, and
    /// refuses a stale or missing If-Match without touching the file.
    #[test]
    fn appending_adds_to_the_end_of_the_file_itself() {
        let dir = scratch("append");
        let file = dir.join("film.kproj");
        let link = dir.join("link.kproj");
        fs::write(&file, "head").unwrap();
        fs::hard_link(&file, &link).unwrap();
        let (_bridge, base) = serve(&file, never_asked());
        let append = |tag: Option<&str>, body: &[u8]| {
            let mut request = ureq::post(&format!("{base}/append")).set("Origin", ORIGIN);
            if let Some(tag) = tag {
                request = request.set("If-Match", tag);
            }
            answer(request.send_bytes(body))
        };

        let (first, _) = etag(&file).unwrap();
        let (status, r) = append(Some(&first), b"-tail");
        assert_eq!(status, 200);
        let header_tag = r.header("ETag").unwrap().to_string();
        let body = json(r);
        let (now, size) = etag(&file).unwrap();
        assert_eq!((body["etag"].as_str(), body["size"].as_u64()), (Some(now.as_str()), Some(9)));
        assert_eq!((header_tag.as_str(), size), (now.as_str(), 9));
        assert_ne!(now, first);
        assert_eq!(fs::read_to_string(&file).unwrap(), "head-tail");
        assert_eq!(fs::read_to_string(&link).unwrap(), "head-tail");

        // The version before: refused, the current tag said, nothing written.
        // A body past tiny_http's 1 KiB is drained, and the bridge goes on.
        let (status, r) = append(Some(&first), &[b'x'; 300_000]);
        assert_eq!((status, r.header("ETag")), (412, Some(now.as_str())));
        let (status, _) = append(None, b"x");
        assert_eq!(status, 412);
        assert_eq!(fs::read_to_string(&file).unwrap(), "head-tail");

        // Larger than a megabyte (the copy's buffer), on top.
        let more: Vec<u8> = (0..3_000_000u32).map(|i| (i % 253) as u8).collect();
        let (status, r) = append(Some(&now), &more);
        assert_eq!(status, 200);
        assert_eq!(json(r)["size"].as_u64(), Some(9 + more.len() as u64));
        let on_disk = fs::read(&link).unwrap();
        assert_eq!((&on_disk[..9], &on_disk[9..]), (&b"head-tail"[..], &more[..]));
        assert_eq!(names(&dir), ["film.kproj", "link.kproj"]);
        fs::remove_dir_all(&dir).unwrap();
    }

    /// A body that fails half-way (the page gone, a full disk) leaves the
    /// file as the page knew it: same bytes, same tag.
    #[test]
    fn a_failed_append_leaves_the_file_as_it_was() {
        struct Breaks(usize);
        impl Read for Breaks {
            fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
                if self.0 == 0 {
                    return Err(io::Error::new(io::ErrorKind::ConnectionReset, "the page went away"));
                }
                let n = buf.len().min(self.0);
                buf[..n].fill(b'z');
                self.0 -= n;
                Ok(n)
            }
        }
        let dir = scratch("torn");
        let file = dir.join("film.kproj");
        fs::write(&file, "whole").unwrap();
        let (tag, _) = etag(&file).unwrap();
        thread::sleep(Duration::from_millis(20));
        let error = append_in_place(&file, &tag, &mut Breaks(3_000_000)).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::ConnectionReset);
        assert_eq!(fs::read_to_string(&file).unwrap(), "whole");
        assert_eq!(etag(&file).unwrap().0, tag);
        // And a stale tag writes nothing.
        assert_eq!(append_in_place(&file, "\"0-0\"", &mut &b"x"[..]).unwrap(), Appended::Changed(tag));
        assert_eq!(fs::read_to_string(&file).unwrap(), "whole");
        fs::remove_dir_all(&dir).unwrap();
    }

    /// Another program's lock on the file: waited for about two seconds, then
    /// refused as for PUT (409 "locked", with who holds it on Windows); a
    /// lock released meanwhile is waited out.
    #[test]
    fn an_append_waits_for_a_lock_then_says_it_is_locked() {
        let dir = scratch("append-lock");
        let file = dir.join("film.kproj");
        fs::write(&file, "head").unwrap();
        let (tag, _) = etag(&file).unwrap();
        let (_bridge, base) = serve(&file, never_asked());
        let append = |body: &[u8]| {
            answer(ureq::post(&format!("{base}/append")).set("Origin", ORIGIN).set("If-Match", &tag).send_bytes(body))
        };

        // Another program reading it, under a lock of its own.
        let other = File::open(&file).unwrap();
        other.lock().unwrap();
        let (status, r) = append(b"-tail");
        assert_eq!(status, 409);
        let body = json(r);
        assert_eq!(body["reason"], "locked");
        if cfg!(windows) {
            assert!(!body["holders"].as_array().unwrap().is_empty(), "{body}");
        }
        // Untouched (read through the lock's own handle: Windows keeps
        // every other one out of a locked file).
        let mut seen = String::new();
        (&other).read_to_string(&mut seen).unwrap();
        assert_eq!(seen, "head");

        let release = thread::spawn(move || {
            thread::sleep(Duration::from_millis(300));
            drop(other);
        });
        let (status, _) = append(b"-tail");
        release.join().unwrap();
        assert_eq!(status, 200);
        assert_eq!(fs::read_to_string(&file).unwrap(), "head-tail");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_suggested_name_is_a_file_name_and_nothing_else() {
        let session = Path::new("projects/film.kproj");
        let name = |raw: Option<&str>| suggested_name(raw, session);
        assert_eq!(name(Some("Projet final")), "Projet final.kproj");
        assert_eq!(name(Some("Projet final.KPROJ")), "Projet final.KPROJ");
        assert_eq!(name(Some("version 2.1")), "version 2.1.kproj");
        assert_eq!(name(Some("../../evil.kproj")), "evil.kproj");
        assert_eq!(name(Some(r"C:\Windows\x")), "CWindowsx.kproj");
        assert_eq!(name(Some("a<b>:c\"d|e?f*\u{7}g")), "abcdefg.kproj");
        assert_eq!(name(Some("notes. ")), "notes.kproj");
        assert_eq!(name(Some("プロジェクト")), "プロジェクト.kproj");
        assert_eq!(name(Some("CON")), "_CON.kproj");
        assert_eq!(name(Some("com1.txt")), "_com1.txt.kproj");
        assert_eq!(name(Some("console")), "console.kproj");
        assert_eq!(name(Some(" ... ")), "film.kproj");
        assert_eq!(name(None), "film.kproj");
        assert_eq!(name(Some(&"x".repeat(500))), format!("{}.kproj", "x".repeat(120)));
        assert_eq!(suggested_name(Some("notes"), Path::new("Makefile")), "notes");
        assert_eq!(suggested_name(None, Path::new("Makefile")), "Makefile");
    }

    /// "Save as": the dialog is asked for the file's folder, a clean name and
    /// the file's type; the body lands where the user picked (a new file, or
    /// one replaced), under a NEW session for the same origin only; a cancel
    /// writes nothing.
    #[test]
    fn save_as_writes_where_the_user_picked_under_a_new_session() {
        let dir = scratch("save-as");
        let file = dir.join("film.kproj");
        fs::write(&file, "original").unwrap();
        let asked: Arc<Mutex<Vec<SaveAsk>>> = Arc::default();
        let pick: Arc<Mutex<Option<PathBuf>>> = Arc::default();
        let choose: Chooser = {
            let (asked, pick) = (asked.clone(), pick.clone());
            Arc::new(move |ask: &SaveAsk| {
                asked.lock().unwrap().push(ask.clone());
                pick.lock().unwrap().clone()
            })
        };
        let (bridge, base) = serve(&file, choose);
        let save_as = |query: &str, body: &[u8]| {
            answer(ureq::post(&format!("{base}/save-as{query}")).set("Origin", ORIGIN).send_bytes(body))
        };

        // A new file.
        *pick.lock().unwrap() = Some(dir.join("picked.kproj"));
        let (status, r) = save_as("?name=Projet%20final", b"new work");
        assert_eq!(status, 200);
        let saved = json(r);
        assert_eq!(
            asked.lock().unwrap().last(),
            Some(&SaveAsk { folder: Some(dir.clone()), name: "Projet final.kproj".into(), ext: Some("kproj".into()) })
        );
        assert_eq!(saved["name"], "picked.kproj");
        assert_eq!(saved["etag"].as_str(), Some(etag(&dir.join("picked.kproj")).unwrap().0.as_str()));
        assert_eq!(fs::read_to_string(dir.join("picked.kproj")).unwrap(), "new work");
        assert_eq!(fs::read_to_string(&file).unwrap(), "original");
        let token = saved["token"].as_str().unwrap();
        assert!(token.len() == 64 && token.bytes().all(|b| b.is_ascii_hexdigit()));
        assert!(saved.get("path").is_none(), "the page learns no path");

        // The new session reads the new file, for this origin only.
        let new = format!("http://127.0.0.1:{}/s/{token}", bridge.port);
        let (status, r) = answer(ureq::get(&format!("{new}/meta")).set("Origin", ORIGIN).call());
        assert_eq!((status, json(r)["name"].as_str().map(str::to_string)), (200, Some("picked.kproj".into())));
        let (status, _) = answer(ureq::get(&format!("{new}/meta")).set("Origin", "https://evil.example").call());
        assert_eq!(status, 403);

        // A file that exists (the dialog asked the user before replacing it).
        let (status, r) = save_as("", b"newer work");
        assert_eq!(status, 200);
        assert_eq!(json(r)["name"], "picked.kproj");
        assert_eq!(asked.lock().unwrap().last().unwrap().name, "film.kproj");
        assert_eq!(fs::read_to_string(dir.join("picked.kproj")).unwrap(), "newer work");

        // Cancelled: nothing written, a large body drained, the bridge goes on.
        *pick.lock().unwrap() = None;
        let (status, r) = save_as("?name=other", &[b'x'; 300_000]);
        assert_eq!(status, 200);
        assert_eq!(json(r), serde_json::json!({ "cancelled": true }));
        assert_eq!(names(&dir), ["film.kproj", "picked.kproj"]);

        // Another origin: refused before any dialog.
        let before = asked.lock().unwrap().len();
        let (status, _) =
            answer(ureq::post(&format!("{base}/save-as")).set("Origin", "https://evil.example").send_bytes(b"x"));
        assert_eq!(status, 403);
        assert_eq!(asked.lock().unwrap().len(), before);
        fs::remove_dir_all(&dir).unwrap();
    }

    /// The dialog waits on the user; the other requests do not wait on it.
    #[test]
    fn an_open_dialog_holds_up_nothing_else() {
        let dir = scratch("dialog-open");
        let file = dir.join("film.kproj");
        fs::write(&file, "x").unwrap();
        let slow: Chooser = Arc::new(|_: &SaveAsk| {
            thread::sleep(Duration::from_millis(1500));
            None
        });
        let (_bridge, base) = serve(&file, slow);
        let saving = {
            let url = format!("{base}/save-as");
            thread::spawn(move || answer(ureq::post(&url).set("Origin", ORIGIN).send_bytes(b"x")).0)
        };
        thread::sleep(Duration::from_millis(200));
        let start = Instant::now();
        let (status, _) = answer(ureq::post(&format!("{base}/heartbeat")).set("Origin", ORIGIN).call());
        assert_eq!(status, 204);
        assert!(start.elapsed() < Duration::from_millis(800), "{:?}", start.elapsed());
        assert_eq!(saving.join().unwrap(), 200);
        fs::remove_dir_all(&dir).unwrap();
    }
}
