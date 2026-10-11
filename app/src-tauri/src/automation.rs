//! Assistants: the loopback relay between an assistant and the app pages.
//!
//! An assistant installed on the computer (Claude Desktop, Codex, Qwen Code,
//! LM Studio...) starts `kynoko-launcher mcp`, which speaks MCP on its
//! standard input and output and passes every message, as it is, to this
//! agent over a WebSocket on the loopback (mcp_stdio.rs). The agent answers
//! the protocol (mcp.rs) and relays each tool call to the page of the app
//! that runs it, over another WebSocket on the same listener: the page runs
//! the call in its own engine and history, and answers.
//!
//! The launcher opens that page for the assistant, at
//! `<app>#kynokoAutomation=127.0.0.1:<port>/<token>`, exactly as it hands a
//! file over with `#kynokoBridge=`. Guards (SPEC section 19):
//! - the listener is bound to 127.0.0.1 and answers only `Host:
//!   127.0.0.1:<port>`, which defeats DNS rebinding;
//! - a page is reached at `/a/<token>`: 256 random bits that name ONE page,
//!   good only for the origin of the app it was made for (exact `Origin`);
//! - an assistant is reached at `/c/<token>`, the token kept in the state
//!   folder for this run (`automation.json`), compared in constant time, and
//!   only without an `Origin` header: a web page always sends one, so no
//!   page can take an assistant's place.
//! A page never names a path, a file or a command of the launcher: it only
//! answers the calls it is sent.

use std::collections::HashMap;
use std::io::ErrorKind;
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tungstenite::http::StatusCode;
use tungstenite::protocol::frame::coding::CloseCode;
use tungstenite::protocol::CloseFrame;
use tungstenite::{Message, WebSocket};

/// Version of the messages between the launcher and a page.
pub const PROTOCOL: u64 = 1;
/// How often a connection's loop looks for messages to send.
const POLL: Duration = Duration::from_millis(40);
/// A page opened for an assistant that never connected is forgotten after
/// this (the user may still be reading the question it asks).
const NEVER_CAME: Duration = Duration::from_secs(300);
/// A page that left (closed, reloaded) is forgotten after this; a reload
/// comes back long before.
const LEFT: Duration = Duration::from_secs(600);
/// After the user stops the assistant in an app, its calls to that app are
/// refused this long: an assistant must not reopen what it was told to leave.
pub const STOPPED_FOR: Duration = Duration::from_secs(600);

#[derive(Clone)]
pub struct Automation {
    inner: Arc<Inner>,
}

/// What an assistant's connection does with each message it sends: answer
/// it, now or later, through the sender (one JSON message per string).
pub type OnClient = Arc<dyn Fn(&Automation, u64, &str, &Sender<String>) + Send + Sync>;

struct Inner {
    port: u16,
    client_token: String,
    state: Mutex<State>,
    changed: Condvar,
    next: AtomicU64,
    on_client: OnClient,
}

#[derive(Default)]
struct State {
    pages: HashMap<String, Page>,
    pending: HashMap<u64, Pending>,
    clients: HashMap<u64, Client>,
    /// App code -> when the user stopped the assistant there.
    stopped: HashMap<String, Instant>,
}

struct Page {
    app: String,
    origin: String,
    opened: Instant,
    /// The live socket: its serial and how to send to it.
    link: Option<(u64, Sender<String>)>,
    connected_at: Option<Instant>,
    left_at: Option<Instant>,
}

struct Pending {
    page: String,
    reply: Sender<Reply>,
}

enum Reply {
    Result(Value),
    Error(String, String),
}

struct Client {
    name: Option<String>,
}

/// Why a call has no result: a code and a sentence for the model.
#[derive(Debug, Clone, PartialEq)]
pub struct CallError {
    pub code: String,
    pub message: String,
}

impl CallError {
    fn new(code: &str, message: impl Into<String>) -> Self {
        CallError { code: code.to_string(), message: message.into() }
    }
}

impl Automation {
    /// Binds 127.0.0.1 on a free port and serves pages and assistants.
    pub fn start(on_client: OnClient) -> std::io::Result<Automation> {
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        let port = listener.local_addr()?.port();
        let inner = Arc::new(Inner {
            port,
            client_token: random_token(),
            state: Mutex::new(State::default()),
            changed: Condvar::new(),
            next: AtomicU64::new(1),
            on_client,
        });
        let accepting = inner.clone();
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let inner = accepting.clone();
                thread::spawn(move || serve(inner, stream));
            }
        });
        Ok(Automation { inner })
    }

    pub fn port(&self) -> u16 {
        self.inner.port
    }

    pub fn client_token(&self) -> &str {
        &self.inner.client_token
    }

    /// A new page for `app`, to be opened at `origin`: its token.
    pub fn new_page(&self, app: &str, origin: &str) -> String {
        let token = random_token();
        let mut state = self.lock();
        sweep(&mut state);
        state.pages.insert(
            token.clone(),
            Page { app: app.to_string(), origin: origin.to_string(), opened: Instant::now(), link: None, connected_at: None, left_at: None },
        );
        token
    }

    /// The page of `app` that connected last and is still there.
    pub fn connected_page(&self, app: &str) -> Option<String> {
        let state = self.lock();
        state
            .pages
            .iter()
            .filter(|(_, p)| p.link.is_some() && p.app.eq_ignore_ascii_case(app))
            .max_by_key(|(_, p)| p.connected_at)
            .map(|(t, _)| t.clone())
    }

    /// Waits until the page connects (the user said yes), at most `timeout`.
    pub fn wait_connected(&self, token: &str, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut state = self.lock();
        loop {
            match state.pages.get(token) {
                Some(p) if p.link.is_some() => return true,
                Some(_) => {}
                None => return false,
            }
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            state = self.inner.changed.wait_timeout(state, deadline - now).expect("automation lock").0;
        }
    }

    /// Sends one tool call to a page and waits for its answer.
    pub fn call(&self, token: &str, tool: &str, args: Value, timeout: Duration) -> Result<Value, CallError> {
        let id = self.inner.next.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = mpsc::channel();
        {
            let mut state = self.lock();
            let link = state
                .pages
                .get(token)
                .and_then(|p| p.link.as_ref().map(|(_, l)| l.clone()))
                .ok_or_else(|| CallError::new("GONE", "The app's window is no longer connected."))?;
            state.pending.insert(id, Pending { page: token.to_string(), reply: tx });
            let message = json!({ "type": "call", "id": id, "tool": tool, "args": args }).to_string();
            if link.send(message).is_err() {
                state.pending.remove(&id);
                return Err(CallError::new("GONE", "The app's window is no longer connected."));
            }
        }
        match rx.recv_timeout(timeout) {
            Ok(Reply::Result(value)) => Ok(value),
            Ok(Reply::Error(code, message)) => Err(CallError { code, message }),
            Err(_) => {
                self.lock().pending.remove(&id);
                Err(CallError::new("TIMEOUT", "The app did not answer in time."))
            }
        }
    }

    /// Whether the user stopped the assistant in `app` a moment ago.
    pub fn stopped_recently(&self, app: &str) -> bool {
        self.lock().stopped.iter().any(|(code, at)| code.eq_ignore_ascii_case(app) && at.elapsed() < STOPPED_FOR)
    }

    /// An assistant connected, or a page working for one: the agent stays.
    pub fn busy(&self) -> bool {
        let state = self.lock();
        !state.clients.is_empty() || state.pages.values().any(|p| p.link.is_some())
    }

    /// How many assistants are connected.
    pub fn clients(&self) -> usize {
        self.lock().clients.len()
    }

    /// An assistant said its name (MCP's clientInfo): the pages show it.
    pub fn set_client_name(&self, client: u64, name: Option<String>) {
        let mut state = self.lock();
        if let Some(c) = state.clients.get_mut(&client) {
            if c.name == name {
                return;
            }
            c.name = name;
        }
        broadcast_client(&state);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.inner.state.lock().expect("automation lock")
    }
}

/// The name the pages show: the assistant that connected last.
fn current_client(state: &State) -> Option<String> {
    state.clients.iter().max_by_key(|(id, _)| **id).and_then(|(_, c)| c.name.clone())
}

fn broadcast_client(state: &State) {
    let message = json!({ "type": "client", "client": current_client(state) }).to_string();
    for page in state.pages.values() {
        if let Some((_, link)) = &page.link {
            let _ = link.send(message.clone());
        }
    }
}

fn sweep(state: &mut State) {
    state.pages.retain(|_, p| match (p.link.is_some(), p.connected_at, p.left_at) {
        (true, _, _) => true,
        (false, None, _) => p.opened.elapsed() < NEVER_CAME,
        (false, Some(_), Some(left)) => left.elapsed() < LEFT,
        (false, Some(_), None) => true,
    });
    state.stopped.retain(|_, at| at.elapsed() < STOPPED_FOR);
}

/* ------------------------------ connections ------------------------------ */

enum Route {
    Page(String),
    Client,
}

fn serve(inner: Arc<Inner>, stream: TcpStream) {
    let mut route = None;
    let ws = {
        let route = &mut route;
        let inner = &inner;
        tungstenite::accept_hdr(stream, move |req: &Request, res: Response| -> Result<Response, ErrorResponse> {
            match admit(inner, req) {
                Ok(r) => {
                    *route = Some(r);
                    Ok(res)
                }
                Err(status) => {
                    let mut refusal = ErrorResponse::new(None);
                    *refusal.status_mut() = status;
                    Err(refusal)
                }
            }
        })
    };
    let Ok(ws) = ws else { return };
    match route {
        Some(Route::Page(token)) => serve_page(inner, token, ws),
        Some(Route::Client) => serve_client(inner, ws),
        None => {}
    }
}

/// Who may open a socket: checked before the WebSocket handshake answers.
fn admit(inner: &Inner, req: &Request) -> Result<Route, StatusCode> {
    let header = |name: &str| req.headers().get(name).and_then(|v| v.to_str().ok());
    if header("host") != Some(format!("127.0.0.1:{}", inner.port).as_str()) {
        return Err(StatusCode::MISDIRECTED_REQUEST);
    }
    let path = req.uri().path();
    if let Some(token) = path.strip_prefix("/a/") {
        let state = inner.state.lock().expect("automation lock");
        let page = state.pages.get(token).ok_or(StatusCode::NOT_FOUND)?;
        if header("origin") != Some(page.origin.as_str()) {
            return Err(StatusCode::FORBIDDEN);
        }
        return Ok(Route::Page(token.to_string()));
    }
    if let Some(token) = path.strip_prefix("/c/") {
        if header("origin").is_some() || !same(token.as_bytes(), inner.client_token.as_bytes()) {
            return Err(StatusCode::FORBIDDEN);
        }
        return Ok(Route::Client);
    }
    Err(StatusCode::NOT_FOUND)
}

fn serve_page(inner: Arc<Inner>, token: String, mut ws: WebSocket<TcpStream>) {
    let serial = inner.next.fetch_add(1, Ordering::SeqCst);
    let (tx, rx) = mpsc::channel::<String>();
    {
        let mut state = inner.state.lock().expect("automation lock");
        let welcome = json!({ "type": "welcome", "protocol": PROTOCOL, "client": current_client(&state) }).to_string();
        let Some(page) = state.pages.get_mut(&token) else { return };
        // A second socket for the same page (a reload racing the old one)
        // takes over: the old one's messages are not answered any more.
        page.link = Some((serial, tx.clone()));
        page.connected_at = Some(Instant::now());
        page.left_at = None;
        let _ = tx.send(welcome);
    }
    inner.changed.notify_all();
    let _ = ws.get_mut().set_read_timeout(Some(POLL));
    let mut close: Option<(u16, &str)> = None;
    'connection: loop {
        match ws.read() {
            Ok(Message::Text(text)) => {
                if let Some(code) = page_message(&inner, &token, text.as_str()) {
                    close = Some(code);
                    break;
                }
            }
            Ok(Message::Close(_)) => break,
            Ok(_) => {}
            Err(tungstenite::Error::Io(e)) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(_) => break,
        }
        while let Ok(text) = rx.try_recv() {
            if ws.send(Message::text(text)).is_err() {
                break 'connection;
            }
        }
    }
    if let Some((code, reason)) = close {
        let _ = ws.close(Some(CloseFrame { code: CloseCode::from(code), reason: reason.into() }));
        let _ = ws.flush();
    }
    let mut state = inner.state.lock().expect("automation lock");
    if let Some(page) = state.pages.get_mut(&token) {
        if page.link.as_ref().is_some_and(|(s, _)| *s == serial) {
            page.link = None;
            page.left_at = Some(Instant::now());
            // Calls the page will never answer now: said at once, rather
            // than at the end of their wait.
            let lost: Vec<u64> = state.pending.iter().filter(|(_, p)| p.page == token).map(|(id, _)| *id).collect();
            for id in lost {
                if let Some(p) = state.pending.remove(&id) {
                    let _ = p.reply.send(Reply::Error("GONE".into(), "The app's window was closed or reloaded before it answered.".into()));
                }
            }
        }
    }
    drop(state);
    inner.changed.notify_all();
}

/// One message from a page; a close code when the page must be let go.
fn page_message(inner: &Inner, token: &str, text: &str) -> Option<(u16, &'static str)> {
    let Ok(m) = serde_json::from_str::<Value>(text) else { return None };
    let mut state = inner.state.lock().expect("automation lock");
    let app = state.pages.get(token).map(|p| p.app.clone()).unwrap_or_default();
    match m.get("type").and_then(Value::as_str) {
        Some("hello") => {
            let protocol = m.get("protocol").and_then(Value::as_u64);
            let said = m.get("app").and_then(Value::as_str).unwrap_or_default();
            if protocol != Some(PROTOCOL) {
                return Some((4426, "protocol"));
            }
            if !said.eq_ignore_ascii_case(&app) {
                return Some((4403, "app"));
            }
            None
        }
        Some(kind @ ("result" | "error")) => {
            let id = m.get("id").and_then(Value::as_u64)?;
            // Only the page a call was sent to may answer it.
            if state.pending.get(&id).is_some_and(|p| p.page == token) {
                let p = state.pending.remove(&id).expect("present");
                let reply = if kind == "result" {
                    Reply::Result(m.get("result").cloned().unwrap_or(Value::Null))
                } else {
                    Reply::Error(
                        m.get("code").and_then(Value::as_str).unwrap_or("FAILED").chars().take(40).collect(),
                        m.get("message").and_then(Value::as_str).unwrap_or("The app could not do it.").chars().take(2000).collect(),
                    )
                };
                let _ = p.reply.send(reply);
            }
            None
        }
        Some("bye") => {
            state.stopped.insert(app, Instant::now());
            Some((1000, "stopped"))
        }
        _ => None,
    }
}

fn serve_client(inner: Arc<Inner>, mut ws: WebSocket<TcpStream>) {
    let id = inner.next.fetch_add(1, Ordering::SeqCst);
    let (tx, rx) = mpsc::channel::<String>();
    inner.state.lock().expect("automation lock").clients.insert(id, Client { name: None });
    let automation = Automation { inner: inner.clone() };
    let _ = ws.get_mut().set_read_timeout(Some(POLL));
    'connection: loop {
        match ws.read() {
            Ok(Message::Text(text)) => (inner.on_client)(&automation, id, text.as_str(), &tx),
            Ok(Message::Close(_)) => break,
            Ok(_) => {}
            Err(tungstenite::Error::Io(e)) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(_) => break,
        }
        while let Ok(text) = rx.try_recv() {
            if ws.send(Message::text(text)).is_err() {
                break 'connection;
            }
        }
    }
    let mut state = inner.state.lock().expect("automation lock");
    state.clients.remove(&id);
    broadcast_client(&state);
    drop(state);
    inner.changed.notify_all();
}

/* -------------------------------- helpers -------------------------------- */

fn random_token() -> String {
    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes).expect("system random");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Compares two secrets in a time that does not depend on where they differ.
fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    fn started() -> Automation {
        // An assistant that answers each message with the same text, upper-cased.
        let on_client: OnClient = Arc::new(|_, _, text, out| {
            let _ = out.send(text.to_uppercase());
        });
        Automation::start(on_client).expect("listener")
    }

    fn connect(a: &Automation, path: &str, origin: Option<&str>) -> Result<WebSocket<TcpStream>, String> {
        let stream = TcpStream::connect(("127.0.0.1", a.port())).map_err(|e| e.to_string())?;
        let mut req = tungstenite::client::IntoClientRequest::into_client_request(format!("ws://127.0.0.1:{}{path}", a.port())).map_err(|e| e.to_string())?;
        if let Some(origin) = origin {
            req.headers_mut().insert("Origin", origin.parse().expect("header"));
        }
        tungstenite::client(req, stream).map(|(ws, _)| ws).map_err(|e| e.to_string())
    }

    fn text(ws: &mut WebSocket<TcpStream>) -> Value {
        loop {
            if let Message::Text(t) = ws.read().expect("message") {
                return serde_json::from_str(t.as_str()).unwrap_or(Value::String(t.as_str().to_string()));
            }
        }
    }

    #[test]
    fn a_page_needs_its_token_and_its_origin() {
        let a = started();
        let token = a.new_page("QrcodeStudio", "https://qrcode-studio.kynoko.com");
        assert!(connect(&a, &format!("/a/{token}"), None).is_err(), "no Origin");
        assert!(connect(&a, &format!("/a/{token}"), Some("https://evil.example")).is_err(), "another origin");
        assert!(connect(&a, &format!("/a/{}", random_token()), Some("https://qrcode-studio.kynoko.com")).is_err(), "unknown token");
        let mut ws = connect(&a, &format!("/a/{token}"), Some("https://qrcode-studio.kynoko.com")).expect("the page");
        assert_eq!(text(&mut ws)["type"], "welcome");
        assert!(a.wait_connected(&token, Duration::from_secs(2)));
        assert_eq!(a.connected_page("qrcodestudio"), Some(token));
    }

    #[test]
    fn an_assistant_needs_the_token_and_no_origin() {
        let a = started();
        let token = a.client_token().to_string();
        assert!(connect(&a, "/c/wrong", None).is_err());
        assert!(connect(&a, &format!("/c/{token}"), Some("https://kynoko.com")).is_err(), "a web page");
        let mut ws = connect(&a, &format!("/c/{token}"), None).expect("the assistant");
        ws.send(Message::text("ping")).expect("send");
        assert_eq!(text(&mut ws), Value::String("PING".into()));
        assert!(a.busy());
    }

    #[test]
    fn the_host_header_must_be_the_loopback_address() {
        let a = started();
        let mut s = TcpStream::connect(("127.0.0.1", a.port())).expect("tcp");
        let req = format!(
            "GET /c/{} HTTP/1.1\r\nHost: evil.example:{}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n",
            a.client_token(),
            a.port()
        );
        s.write_all(req.as_bytes()).expect("write");
        let mut answer = [0u8; 64];
        let n = s.read(&mut answer).expect("read");
        assert!(String::from_utf8_lossy(&answer[..n]).contains(" 421"), "{}", String::from_utf8_lossy(&answer[..n]));
    }

    #[test]
    fn a_call_is_answered_by_its_page_only() {
        let a = started();
        let origin = "https://qrcode-studio.kynoko.com";
        let token = a.new_page("QrcodeStudio", origin);
        let mut page = connect(&a, &format!("/a/{token}"), Some(origin)).expect("page");
        let other_token = a.new_page("QrcodeStudio", origin);
        let mut other = connect(&a, &format!("/a/{other_token}"), Some(origin)).expect("other");
        let _ = text(&mut page);
        let _ = text(&mut other);
        let caller = a.clone();
        let t = token.clone();
        let call = thread::spawn(move || caller.call(&t, "qr_create", json!({ "format": "url" }), Duration::from_secs(5)));
        let m = text(&mut page);
        assert_eq!(m["type"], "call");
        assert_eq!(m["args"]["format"], "url");
        let id = m["id"].as_u64().expect("id");
        // The other page cannot answer it.
        other.send(Message::text(json!({ "type": "result", "id": id, "result": { "content": [] , "forged": true } }).to_string())).expect("send");
        thread::sleep(Duration::from_millis(150));
        page.send(Message::text(json!({ "type": "result", "id": id, "result": { "content": [{ "type": "text", "text": "ok" }] } }).to_string())).expect("send");
        let result = call.join().expect("thread").expect("result");
        assert_eq!(result["content"][0]["text"], "ok");
        assert!(result.get("forged").is_none());
    }

    #[test]
    fn a_page_that_leaves_fails_its_calls_at_once() {
        let a = started();
        let origin = "https://office.kynoko.com";
        let token = a.new_page("Office", origin);
        let mut page = connect(&a, &format!("/a/{token}"), Some(origin)).expect("page");
        let _ = text(&mut page);
        let caller = a.clone();
        let t = token.clone();
        let call = thread::spawn(move || caller.call(&t, "sheet_read", json!({}), Duration::from_secs(30)));
        let _ = text(&mut page);
        drop(page);
        let started = Instant::now();
        let err = call.join().expect("thread").expect_err("no answer");
        assert_eq!(err.code, "GONE");
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn stopping_is_remembered_for_the_app() {
        let a = started();
        let origin = "https://office.kynoko.com";
        let token = a.new_page("Office", origin);
        let mut page = connect(&a, &format!("/a/{token}"), Some(origin)).expect("page");
        let _ = text(&mut page);
        page.send(Message::text(json!({ "type": "bye", "reason": "stopped" }).to_string())).expect("send");
        thread::sleep(Duration::from_millis(200));
        assert!(a.stopped_recently("office"));
        assert!(!a.stopped_recently("PhotoStudio"));
    }

    #[test]
    fn a_page_of_another_app_or_protocol_is_let_go() {
        let a = started();
        let origin = "https://office.kynoko.com";
        let token = a.new_page("Office", origin);
        let mut page = connect(&a, &format!("/a/{token}"), Some(origin)).expect("page");
        let _ = text(&mut page);
        page.send(Message::text(json!({ "type": "hello", "protocol": 1, "app": "PhotoStudio", "tools": [] }).to_string())).expect("send");
        let closed = loop {
            match page.read() {
                Ok(Message::Close(frame)) => break frame.map(|f| u16::from(f.code)),
                Ok(_) => continue,
                Err(_) => break None,
            }
        };
        assert_eq!(closed, Some(4403));
    }

    #[test]
    fn secrets_compare_whole() {
        assert!(same(b"abc", b"abc"));
        assert!(!same(b"abc", b"abd"));
        assert!(!same(b"abc", b"ab"));
    }
}
