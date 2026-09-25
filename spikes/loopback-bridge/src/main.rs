//! Phase 0 spike: the loopback channel between Kynoko Launcher and a web
//! app running in ANY browser.
//!
//! One process serves ONE file to ONE origin, behind ONE token:
//!
//!   GET    /s/<token>/meta       name, size, etag
//!   GET    /s/<token>/content    the bytes, streamed
//!   PUT    /s/<token>/content    write back (If-Match: <etag> required)
//!   POST   /s/<token>/heartbeat  keeps the session alive
//!   DELETE /s/<token>            closes the session (the process exits)
//!
//! Guards, all of them on every request:
//! - bound to 127.0.0.1 only, on a random port;
//! - Host must be exactly 127.0.0.1:<port> (DNS rebinding);
//! - Origin must be exactly the allowed app origin (no Origin = refused);
//! - the token is 256 random bits and only ever names this one file;
//! - an idle session (no heartbeat) ends by itself.
//!
//! Usage: loopback-bridge --file <path> --origin <https://app.example> [--idle-secs 120]
//! Prints one JSON line on stdout: {"port":..,"token":"..","fragment":".."}

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, UNIX_EPOCH};

use tiny_http::{Header, Method, Request, Response, Server, StatusCode};

struct Session {
    path: PathBuf,
    token: String,
    origin: String,
    host: String,
    last_seen: Instant,
    closed: bool,
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let arg = |name: &str| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let path = PathBuf::from(arg("--file").expect("--file is required"));
    let origin = arg("--origin").expect("--origin is required");
    let idle = Duration::from_secs(arg("--idle-secs").and_then(|s| s.parse().ok()).unwrap_or(120));
    if !path.is_file() {
        eprintln!("not a file: {}", path.display());
        std::process::exit(2);
    }

    let server = Server::http("127.0.0.1:0").expect("cannot bind 127.0.0.1");
    let port = server.server_addr().to_ip().expect("ip listener").port();
    let mut session = Session {
        path,
        token: random_token(),
        origin,
        host: format!("127.0.0.1:{port}"),
        last_seen: Instant::now(),
        closed: false,
    };

    let fragment = format!("kynokoBridge=127.0.0.1:{port}/{}", session.token);
    println!(
        "{}",
        serde_json::json!({ "port": port, "token": session.token, "fragment": fragment })
    );
    io::stdout().flush().ok();

    while !session.closed && session.last_seen.elapsed() < idle {
        match server.recv_timeout(Duration::from_millis(500)) {
            Ok(Some(request)) => handle(&mut session, request),
            Ok(None) => {}
            Err(e) => {
                eprintln!("recv: {e}");
                break;
            }
        }
    }
}

fn handle(s: &mut Session, mut req: Request) {
    let host = header(&req, "Host");
    let origin = header(&req, "Origin");
    // Host first: a rebound DNS name reaches this socket with ITS name.
    if host.as_deref() != Some(s.host.as_str()) {
        return reply(req, s, 421, "misdirected host", &[]);
    }
    if origin.as_deref() != Some(s.origin.as_str()) {
        // No CORS headers on purpose: the browser must not let the page read this.
        let _ = req.respond(Response::from_string("forbidden origin").with_status_code(403));
        return;
    }

    if *req.method() == Method::Options {
        let mut extra = vec![
            ("Access-Control-Allow-Methods", "GET, PUT, POST, DELETE".to_string()),
            ("Access-Control-Allow-Headers", "If-Match, Content-Type".to_string()),
            ("Access-Control-Max-Age", "600".to_string()),
        ];
        // Chromium's older Private Network Access preflight asks for this.
        if header(&req, "Access-Control-Request-Private-Network").is_some() {
            extra.push(("Access-Control-Allow-Private-Network", "true".to_string()));
        }
        return reply(req, s, 204, "", &extra);
    }

    let url = req.url().to_string();
    let mut parts = url.trim_start_matches('/').splitn(3, '/');
    let (scope, token, rest) = (parts.next(), parts.next(), parts.next().unwrap_or(""));
    if scope != Some("s") || !token.is_some_and(|t| same(t, &s.token)) {
        return reply(req, s, 404, "not found", &[]);
    }
    s.last_seen = Instant::now();

    match (req.method().clone(), rest) {
        (Method::Get, "meta") => match etag(&s.path) {
            Ok((tag, size)) => {
                let name = s.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let body = serde_json::json!({ "name": name, "size": size, "etag": tag }).to_string();
                reply(req, s, 200, &body, &[("Content-Type", "application/json".to_string())])
            }
            Err(e) => reply(req, s, 410, &format!("file gone: {e}"), &[]),
        },
        (Method::Get, "content") => match (File::open(&s.path), etag(&s.path)) {
            (Ok(file), Ok((tag, size))) => {
                let headers = cors(s, &[("ETag", tag), ("Content-Type", "application/octet-stream".into())]);
                let _ = req.respond(Response::new(StatusCode(200), headers, file, Some(size as usize), None));
            }
            (Err(e), _) | (_, Err(e)) => reply(req, s, 410, &format!("file gone: {e}"), &[]),
        },
        (Method::Put, "content") => {
            let current = etag(&s.path).map(|(t, _)| t).unwrap_or_default();
            if header(&req, "If-Match").as_deref() != Some(current.as_str()) {
                // Changed on disk since the page read it: never overwrite blindly.
                return reply(req, s, 412, "changed on disk", &[("ETag", current)]);
            }
            match write_atomically(&s.path, req.as_reader()) {
                Ok(()) => {
                    let tag = etag(&s.path).map(|(t, _)| t).unwrap_or_default();
                    let body = serde_json::json!({ "etag": tag }).to_string();
                    reply(req, s, 200, &body, &[("ETag", tag), ("Content-Type", "application/json".to_string())])
                }
                // Typically: another program holds the file without share-delete.
                Err(e) => reply(req, s, 409, &format!("cannot write: {e}"), &[]),
            }
        }
        (Method::Post, "heartbeat") => reply(req, s, 204, "", &[]),
        (Method::Delete, "") => {
            s.closed = true;
            reply(req, s, 204, "", &[])
        }
        _ => reply(req, s, 405, "method not allowed", &[]),
    }
}

/// Temp file in the SAME directory (so the rename cannot cross volumes), synced,
/// then renamed over the original: a crash leaves either the old or the new file.
fn write_atomically(path: &Path, body: &mut dyn io::Read) -> io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = dir.join(format!(".{name}.{}.kynoko-tmp", &random_token()[..8]));
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

fn etag(path: &Path) -> io::Result<(String, u64)> {
    let meta = fs::metadata(path)?;
    let mtime = meta.modified()?.duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    Ok((format!("\"{mtime:x}-{:x}\"", meta.len()), meta.len()))
}

fn cors(s: &Session, extra: &[(&str, String)]) -> Vec<Header> {
    let mut all = vec![
        ("Access-Control-Allow-Origin", s.origin.clone()),
        ("Access-Control-Expose-Headers", "ETag".to_string()),
        ("Vary", "Origin".to_string()),
        ("Cache-Control", "no-store".to_string()),
    ];
    all.extend(extra.iter().cloned());
    all.into_iter()
        .map(|(k, v)| Header::from_bytes(k.as_bytes(), v.as_bytes()).expect("valid header"))
        .collect()
}

fn reply(req: Request, s: &Session, status: u16, body: &str, extra: &[(&str, String)]) {
    let mut response = Response::from_string(body).with_status_code(status);
    for h in cors(s, extra) {
        response.add_header(h);
    }
    let _ = req.respond(response);
}

fn header(req: &Request, name: &'static str) -> Option<String> {
    req.headers()
        .iter()
        .find(|h| h.field.equiv(name))
        .map(|h| h.value.as_str().to_string())
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
