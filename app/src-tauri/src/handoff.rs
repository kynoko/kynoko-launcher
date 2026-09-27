//! The hand-off to Firefox's app windows (docs/SPEC.md, section 5).
//!
//! Firefox has no `--app` mode. What it has on Windows is Taskbar Tabs:
//! `firefox -taskbar-tab <id> -new-window <url>` opens a site in its own
//! window, without tabs or address bar. But that window always starts at the
//! ROOT of the site it was created for, whatever address the command gives
//! (Firefox keeps only the URL's origin), so a facade or a file (whose bridge
//! address travels in the URL) would be lost.
//!
//! Hence a gateway: a tiny page at the root of its own host (the launcher's
//! `gatewayUrl`, https://launch.kynoko.com/ by default), opened only by the
//! launcher. The launcher leaves the real address here first; the gateway
//! page asks for it and navigates there. A Taskbar Tab window may navigate
//! anywhere under the same base domain without leaving app mode, so the app,
//! its facade and its file open in the app window.
//!
//! The page cannot be told a random port, so this listener takes the first
//! free port of a short FIXED list, which the page tries in order. What it
//! hands out is one-shot, short-lived, and only to the gateway's origin (and
//! to a request addressed to 127.0.0.1 by name, against DNS rebinding).

use std::collections::VecDeque;
use std::io;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use tiny_http::{Header, Method, Request, Response, Server};

/// Tried in this order, here and by the gateway page (keep both lists equal).
pub const PORTS: [u16; 3] = [47318, 47319, 47320];

/// An address not asked for by then is dropped: the user may take a while to
/// answer Firefox's permission prompt, never this long.
const TTL: Duration = Duration::from_secs(120);

struct Pending {
    url: String,
    /// Remembered once the gateway got this address (see Settings::loopback_ok).
    key: String,
    at: Instant,
}

#[derive(Clone)]
pub struct Handoff {
    port: u16,
    origin: String,
    pending: Arc<Mutex<VecDeque<Pending>>>,
    on_reached: fn(String),
}

impl Handoff {
    /// Listens on the first free port of PORTS, for pages of `origin` only.
    /// `on_reached` is told the key of each address the gateway picked up:
    /// proof that Firefox lets that origin reach the launcher.
    pub fn start(origin: String, on_reached: fn(String)) -> io::Result<Handoff> {
        let (server, port) = PORTS
            .iter()
            .find_map(|&p| Server::http(("127.0.0.1", p)).ok().map(|s| (s, p)))
            .ok_or_else(|| io::Error::new(io::ErrorKind::AddrInUse, "no hand-off port free"))?;
        let handoff = Handoff { port, origin, pending: Arc::new(Mutex::new(VecDeque::new())), on_reached };
        let serving = handoff.clone();
        thread::spawn(move || {
            for request in server.incoming_requests() {
                serving.handle(request);
            }
        });
        Ok(handoff)
    }

    pub fn origin(&self) -> &str {
        &self.origin
    }

    /// Leaves `url` for the next gateway page that asks.
    pub fn push(&self, url: String, key: String) {
        let mut pending = self.pending.lock().expect("hand-off lock");
        pending.retain(|p| p.at.elapsed() < TTL);
        pending.push_back(Pending { url, key, at: Instant::now() });
    }

    /// The oldest address still valid, taken out.
    fn pop(&self) -> Option<Pending> {
        let mut pending = self.pending.lock().expect("hand-off lock");
        pending.retain(|p| p.at.elapsed() < TTL);
        pending.pop_front()
    }

    fn handle(&self, req: Request) {
        let host_ok = header(&req, "Host").as_deref() == Some(format!("127.0.0.1:{}", self.port).as_str());
        let origin_ok = header(&req, "Origin").as_deref() == Some(self.origin.as_str());
        if !host_ok {
            let _ = req.respond(Response::from_string("misdirected host").with_status_code(421));
            return;
        }
        if !origin_ok {
            // No CORS header: whoever asked cannot read even this.
            let _ = req.respond(Response::from_string("forbidden origin").with_status_code(403));
            return;
        }
        match (req.method(), req.url()) {
            (Method::Options, _) => {
                let mut extra = vec![("Access-Control-Allow-Methods", "GET".to_string())];
                if header(&req, "Access-Control-Request-Private-Network").is_some() {
                    extra.push(("Access-Control-Allow-Private-Network", "true".to_string()));
                }
                self.reply(req, 204, "", &extra);
            }
            (Method::Get, "/next") => match self.pop() {
                Some(p) => {
                    (self.on_reached)(p.key);
                    let body = serde_json::json!({ "url": p.url }).to_string();
                    self.reply(req, 200, &body, &[("Content-Type", "application/json".to_string())]);
                }
                None => self.reply(req, 204, "", &[]),
            },
            _ => self.reply(req, 404, "not found", &[]),
        }
    }

    fn reply(&self, req: Request, status: u16, body: &str, extra: &[(&str, String)]) {
        let mut response = Response::from_string(body).with_status_code(status);
        let mut headers = vec![
            ("Access-Control-Allow-Origin", self.origin.clone()),
            ("Cache-Control", "no-store".to_string()),
            ("Vary", "Origin".to_string()),
        ];
        headers.extend(extra.iter().map(|(k, v)| (*k, v.clone())));
        for (k, v) in headers {
            if let Ok(h) = Header::from_bytes(k.as_bytes(), v.as_bytes()) {
                response.add_header(h);
            }
        }
        let _ = req.respond(response);
    }
}

fn header(req: &Request, name: &'static str) -> Option<String> {
    req.headers().iter().find(|h| h.field.equiv(name)).map(|h| h.value.as_str().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static REACHED: AtomicUsize = AtomicUsize::new(0);

    fn get(port: u16, host: &str, origin: &str) -> (u16, String, String) {
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        write!(s, "GET /next HTTP/1.1\r\nHost: {host}\r\nOrigin: {origin}\r\nConnection: close\r\n\r\n").unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).unwrap();
        let status = out[9..12].parse().unwrap();
        let (head, body) = out.split_once("\r\n\r\n").unwrap_or((&out, ""));
        (status, head.to_string(), body.to_string())
    }

    #[test]
    fn hands_out_once_to_the_gateway_only() {
        let origin = "https://launch.example".to_string();
        let h = Handoff::start(origin.clone(), |_| {
            REACHED.fetch_add(1, Ordering::SeqCst);
        })
        .unwrap();
        let host = format!("127.0.0.1:{}", h.port);
        h.push("https://office.example/open#kynokoBridge=x".into(), "k".into());

        // Wrong origin, wrong host: refused, and the address is still there.
        assert_eq!(get(h.port, &host, "https://evil.example").0, 403);
        assert_eq!(get(h.port, &format!("localhost:{}", h.port), &origin).0, 421);

        let (status, head, body) = get(h.port, &host, &origin);
        assert_eq!(status, 200);
        assert!(head.contains("Access-Control-Allow-Origin: https://launch.example"));
        assert!(body.contains("https://office.example/open#kynokoBridge=x"));
        assert_eq!(REACHED.load(Ordering::SeqCst), 1);

        // One-shot.
        assert_eq!(get(h.port, &host, &origin).0, 204);
    }
}
