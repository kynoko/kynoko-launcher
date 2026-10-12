//! Assistants: the Model Context Protocol, as the agent speaks it.
//!
//! Dual era (MCP 2026-07-28, "Versioning and Compatibility"): an assistant
//! that opens with `initialize` is served the legacy way (2024-11-05 to
//! 2025-11-25), one that sends the version in each request's `_meta` is
//! served the modern, stateless way (2026-07-28, `server/discover`). Only
//! tools: no resources, prompts or sampling.
//!
//! The tools are the apps' own: each app publishes what an assistant may do
//! with it in its manifest (`assets/kynoko-app.json`, block `automation`,
//! written from the app's declaration at build time), read here from every
//! app of the catalogue. Two more belong to the launcher: `kynoko_apps`
//! (which apps, which tools, which windows are open) and `kynoko_open`.
//! A call to an app's tool goes to a window of that app working for the
//! assistant; without one, the launcher opens it, and the call waits for
//! the user to allow the assistant there (automation.rs).

use std::collections::{HashMap, HashSet};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};

use crate::automation::Automation;

pub const MODERN: &[&str] = &["2026-07-28"];
pub const LEGACY: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];
const META_VERSION: &str = "io.modelcontextprotocol/protocolVersion";
const META_CLIENT: &str = "io.modelcontextprotocol/clientInfo";
const META_SERVER: &str = "io.modelcontextprotocol/serverInfo";

/// How long a window opened for the assistant has to come up and be allowed.
const OPEN_WAIT: Duration = Duration::from_secs(120);
/// How long a tool may run in its window.
const CALL_WAIT: Duration = Duration::from_secs(180);
/// How long the apps' tools are kept before their manifests are read again.
const TOOLS_FRESH: Duration = Duration::from_secs(300);
/// Where the last good copy of each app's tools is kept, for when an app
/// cannot be reached.
const TOOLS_CACHE: &str = "automation-tools.json";

const INSTRUCTIONS: &str = "Tools of the Kynoko apps (office documents, photos, layouts, QR codes...) on this computer. \
Each app's tools start with its prefix (kynoko_apps lists them). Calling one opens the app in a window the user sees: \
the first time, the user must allow the assistant there, so a first call may wait for them. \
Each change is one step of the app's history that the user can undo, and what the user already did stays theirs: \
read before you change. The files stay on this computer.";

/// An app the catalogue knows: its code, its name, the address it opens at.
pub struct AppInfo {
    pub code: String,
    pub name: String,
    pub url: String,
}

/// What the protocol needs from the launcher.
pub struct Host {
    pub apps: Box<dyn Fn() -> Vec<AppInfo> + Send + Sync>,
    /// Opens a window of the app (code) for the assistant, at a facade: the
    /// page's token.
    pub open: Box<dyn Fn(&Automation, &str, Option<&str>) -> Result<String, String> + Send + Sync>,
    pub version: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ToolDef {
    pub name: String,
    pub title: Option<String>,
    pub description: String,
    pub input: Value,
    pub effect: String,
    pub facade: Option<String>,
    /// The argument that names the facade, when the assistant chooses it.
    pub facade_from: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AppTools {
    pub code: String,
    pub name: String,
    pub prefix: String,
    pub tools: Vec<ToolDef>,
}

pub struct Mcp {
    host: Host,
    tools: Mutex<Option<(Instant, Vec<AppTools>)>>,
    /// Held while a call looks for its app's window and opens one: calls
    /// running side by side (an assistant may send several at once) find
    /// the same window instead of opening one each.
    opening: Mutex<()>,
}

impl Mcp {
    pub fn new(host: Host) -> Arc<Mcp> {
        Arc::new(Mcp { host, tools: Mutex::new(None), opening: Mutex::new(()) })
    }

    /// One message from an assistant; answers go through `out`.
    pub fn handle(self: &Arc<Self>, automation: &Automation, client: u64, text: &str, out: &Sender<String>) {
        let msg: Value = match serde_json::from_str(text) {
            Ok(v) => v,
            Err(_) => return send(out, error(Value::Null, -32700, "Parse error", None)),
        };
        let Some(method) = msg.get("method").and_then(Value::as_str) else { return };
        let id = msg.get("id").cloned();
        let params = msg.get("params").cloned().unwrap_or_else(|| json!({}));
        let meta = params.get("_meta");
        let modern = meta.and_then(|m| m.get(META_VERSION)).and_then(Value::as_str).map(str::to_string);
        if let Some(version) = &modern {
            if !MODERN.contains(&version.as_str()) {
                if let Some(id) = id {
                    let supported: Vec<&str> = MODERN.iter().chain(LEGACY).copied().collect();
                    send(out, error(id, -32022, "Unsupported protocol version", Some(json!({ "supported": supported, "requested": version }))));
                }
                return;
            }
            if let Some(name) = meta.and_then(|m| m.get(META_CLIENT)).and_then(client_name) {
                automation.set_client_name(client, Some(name));
            }
        }
        let Some(id) = id else { return }; // a notification: nothing to answer
        let finish = |result: Value| if modern.is_some() { self.modern(result) } else { result };
        match method {
            "initialize" => {
                let asked = params.get("protocolVersion").and_then(Value::as_str).unwrap_or("");
                let version = if LEGACY.contains(&asked) { asked } else { LEGACY[0] };
                if let Some(name) = params.get("clientInfo").and_then(client_name) {
                    automation.set_client_name(client, Some(name));
                }
                send(out, ok(id, json!({
                    "protocolVersion": version,
                    "capabilities": { "tools": { "listChanged": false } },
                    "serverInfo": self.server_info(),
                    "instructions": INSTRUCTIONS,
                })));
            }
            "ping" => send(out, ok(id, finish(json!({})))),
            "server/discover" => {
                let supported: Vec<&str> = MODERN.iter().chain(LEGACY).copied().collect();
                send(out, ok(id, self.modern(json!({
                    "supportedVersions": supported,
                    "capabilities": { "tools": {} },
                    "instructions": INSTRUCTIONS,
                    "ttlMs": 3_600_000,
                    "cacheScope": "private",
                }))));
            }
            "tools/list" => {
                let mut result = json!({ "tools": self.tool_list() });
                if modern.is_some() {
                    result["ttlMs"] = json!(TOOLS_FRESH.as_millis() as u64);
                    result["cacheScope"] = json!("private");
                }
                send(out, ok(id, finish(result)));
            }
            "tools/call" => {
                let name = params.get("name").and_then(Value::as_str).unwrap_or_default().to_string();
                let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
                if !self.knows(&name) {
                    return send(out, error(id, -32602, &format!("Unknown tool: {name}"), None));
                }
                if !args.is_object() {
                    return send(out, error(id, -32602, "The arguments must be an object.", None));
                }
                // A tool may wait minutes (the user answering, an export):
                // other messages go on meanwhile.
                let mcp = self.clone();
                let automation = automation.clone();
                let out = out.clone();
                let modern = modern.is_some();
                thread::spawn(move || {
                    let result = mcp.call_tool(&automation, &name, args);
                    send(&out, ok(id, if modern { mcp.modern(result) } else { result }));
                });
            }
            _ => send(out, error(id, -32601, &format!("Method not found: {method}"), None)),
        }
    }

    fn server_info(&self) -> Value {
        json!({ "name": "kynoko-launcher", "title": "Kynoko", "version": self.host.version })
    }

    /// A modern result: says it is complete, and who answered.
    fn modern(&self, mut result: Value) -> Value {
        if let Some(obj) = result.as_object_mut() {
            obj.insert("resultType".into(), json!("complete"));
            let meta = obj.entry("_meta").or_insert_with(|| json!({}));
            if let Some(meta) = meta.as_object_mut() {
                meta.insert(META_SERVER.into(), self.server_info());
            }
        }
        result
    }

    /* ------------------------------- tools ------------------------------- */

    fn tool_list(&self) -> Vec<Value> {
        let mut list = vec![
            json!({
                "name": "kynoko_apps",
                "title": "List the Kynoko apps",
                "description": "Lists the Kynoko apps an assistant can use on this computer: each app's code, name, \
tool prefix and tools, and whether a window of it is already open for you.",
                "inputSchema": { "type": "object", "properties": {} },
                "annotations": { "title": "List the Kynoko apps", "readOnlyHint": true, "openWorldHint": false },
            }),
            json!({
                "name": "kynoko_open",
                "title": "Open a Kynoko app",
                "description": "Opens a window of a Kynoko app for you (its code or its tool prefix), optionally on one of its \
tools (facade), or finds the one already open for you. The user sees it and must allow you there the first time: \
this waits for them. You rarely need it: calling an app's tool opens the app by itself.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "app": { "type": "string", "description": "The app's code (e.g. QrcodeStudio) or its tool prefix (e.g. qr)." },
                        "facade": { "type": "string", "description": "Optional: the tool of the app to open on (e.g. create)." }
                    },
                    "required": ["app"]
                },
                "annotations": { "title": "Open a Kynoko app", "readOnlyHint": false, "destructiveHint": false, "openWorldHint": false },
            }),
        ];
        for app in self.apps_tools() {
            for t in &app.tools {
                let title = t.title.clone().unwrap_or_else(|| t.name.clone());
                list.push(json!({
                    "name": t.name,
                    "title": title,
                    "description": format!("{} ({})", t.description, app.name),
                    "inputSchema": t.input,
                    "annotations": {
                        "title": title,
                        "readOnlyHint": t.effect == "read",
                        "destructiveHint": t.effect == "destructive",
                        "openWorldHint": false,
                    },
                }));
            }
        }
        list
    }

    fn knows(&self, name: &str) -> bool {
        name == "kynoko_apps" || name == "kynoko_open" || self.find(name).is_some()
    }

    fn find(&self, name: &str) -> Option<(AppTools, ToolDef)> {
        self.apps_tools()
            .into_iter()
            .find_map(|app| app.tools.iter().find(|t| t.name == name).cloned().map(|t| (app, t)))
    }

    fn call_tool(&self, automation: &Automation, name: &str, args: Value) -> Value {
        match name {
            "kynoko_apps" => {
                let apps: Vec<Value> = self
                    .apps_tools()
                    .iter()
                    .map(|a| json!({
                        "app": a.code,
                        "name": a.name,
                        "prefix": a.prefix,
                        "tools": a.tools.iter().map(|t| t.name.clone()).collect::<Vec<_>>(),
                        "windowOpen": automation.connected_page(&a.code).is_some(),
                    }))
                    .collect();
                structured(json!({ "apps": apps }))
            }
            "kynoko_open" => {
                let asked = args.get("app").and_then(Value::as_str).unwrap_or_default();
                let facade = args.get("facade").and_then(Value::as_str);
                let Some(app) = self.apps_tools().into_iter().find(|a| a.code.eq_ignore_ascii_case(asked) || a.prefix == asked) else {
                    return failure(&format!("No Kynoko app \"{asked}\" takes an assistant here: kynoko_apps lists those that do."));
                };
                match self.window(automation, &app, facade) {
                    Ok(_) => text(&format!("{} is open and ready for you.", app.name)),
                    Err(message) => failure(&message),
                }
            }
            _ => {
                let Some((app, tool)) = self.find(name) else { return failure(&format!("Unknown tool {name}.")) };
                // The facade an argument names (a product), else the tool's own.
                let chosen = tool
                    .facade_from
                    .as_ref()
                    .and_then(|key| args.get(key))
                    .and_then(Value::as_str)
                    .filter(|f| !f.is_empty() && f.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '/'))
                    .map(str::to_string);
                let facade = chosen.or_else(|| tool.facade.clone());
                let token = match self.window(automation, &app, facade.as_deref()) {
                    Ok(token) => token,
                    Err(message) => return failure(&message),
                };
                match automation.call(&token, name, args, CALL_WAIT) {
                    Ok(result) => tool_result(result),
                    Err(e) => failure(&e.message),
                }
            }
        }
    }

    /// The window of `app` working for the assistant, once its user allowed
    /// it: the one open, else the one opened a moment ago whose user is still
    /// reading the question, else a new one. One window per app: a second
    /// would ask the user a second time, for the same work.
    fn window(&self, automation: &Automation, app: &AppTools, facade: Option<&str>) -> Result<String, String> {
        if automation.stopped_recently(&app.code) {
            return Err(format!(
                "The user stopped you in {} a moment ago. Ask them before working there again.",
                app.name
            ));
        }
        let token = {
            let _one_at_a_time = self.opening.lock().expect("opening lock");
            if let Some(token) = automation.connected_page(&app.code) {
                return Ok(token);
            }
            match automation.awaited_page(&app.code, OPEN_WAIT) {
                Some(token) => token,
                None => (self.host.open)(automation, &app.code, facade)?,
            }
        };
        if automation.wait_connected(&token, OPEN_WAIT) {
            Ok(token)
        } else {
            Err(format!(
                "{} was opened, but the user has not allowed you there (or the browser could not reach Kynoko Launcher). \
Ask the user to choose \"Allow\" in the app's window, then try again.",
                app.name
            ))
        }
    }

    /* ----------------------- the apps' declarations ----------------------- */

    fn apps_tools(&self) -> Vec<AppTools> {
        if let Some((at, tools)) = self.tools.lock().expect("tools lock").as_ref() {
            if at.elapsed() < TOOLS_FRESH {
                return tools.clone();
            }
        }
        let fresh = self.read_apps();
        *self.tools.lock().expect("tools lock") = Some((Instant::now(), fresh.clone()));
        fresh
    }

    /// Every app's declaration, read from its manifest (all at once), the
    /// last good copy for an app that cannot be reached.
    fn read_apps(&self) -> Vec<AppTools> {
        let apps = (self.host.apps)();
        let mut kept: HashMap<String, Value> = crate::settings::read_named(TOOLS_CACHE);
        let fetched: Vec<(String, Option<Value>)> = thread::scope(|scope| {
            let handles: Vec<_> = apps
                .iter()
                .map(|a| (a.code.clone(), scope.spawn(|| fetch_declaration(&a.url))))
                .collect();
            handles.into_iter().map(|(code, h)| (code, h.join().ok().flatten())).collect()
        });
        let mut changed = false;
        for (code, value) in &fetched {
            if let Some(v) = value {
                if kept.get(code) != Some(v) {
                    kept.insert(code.clone(), v.clone());
                    changed = true;
                }
            }
        }
        if changed {
            let _ = crate::settings::write_named(TOOLS_CACHE, &kept);
        }
        let mut prefixes = HashSet::new();
        let mut names = HashSet::from(["kynoko_apps".to_string(), "kynoko_open".to_string()]);
        let mut out = Vec::new();
        for app in &apps {
            let Some(decl) = kept.get(&app.code) else { continue };
            let Some(mut parsed) = parse_declaration(app, decl) else { continue };
            // First come, first served: a prefix or a name another app took is not offered twice.
            if !prefixes.insert(parsed.prefix.clone()) {
                continue;
            }
            parsed.tools.retain(|t| names.insert(t.name.clone()));
            if !parsed.tools.is_empty() {
                out.push(parsed);
            }
        }
        out
    }
}

/// The `automation` block of an app's manifest, or None.
fn fetch_declaration(url: &str) -> Option<Value> {
    let agent = ureq::AgentBuilder::new().timeout(Duration::from_secs(6)).build();
    let manifest: Value = agent.get(&format!("{}/assets/kynoko-app.json", url.trim_end_matches('/'))).call().ok()?.into_json().ok()?;
    manifest.get("automation").filter(|a| a.is_object()).cloned()
}

/// An app's declaration, checked: anything that does not hold is left out.
pub fn parse_declaration(app: &AppInfo, decl: &Value) -> Option<AppTools> {
    let prefix = decl.get("prefix")?.as_str()?.to_string();
    let prefix_ok = (2..=16).contains(&prefix.len())
        && prefix.starts_with(|c: char| c.is_ascii_lowercase())
        && prefix.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
    if !prefix_ok || prefix == "kynoko" {
        return None;
    }
    let tools = decl
        .get("tools")?
        .as_array()?
        .iter()
        .filter_map(|t| {
            let name = t.get("name")?.as_str()?.to_string();
            let name_ok = name.len() <= 64
                && name.starts_with(&format!("{prefix}_"))
                && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
            let description = t.get("description")?.as_str()?.trim().to_string();
            let input = t.get("input")?.clone();
            let effect = t.get("effect").and_then(Value::as_str).unwrap_or("write").to_string();
            if !name_ok || description.is_empty() || input.get("type").and_then(Value::as_str) != Some("object") {
                return None;
            }
            if !["read", "write", "destructive"].contains(&effect.as_str()) {
                return None;
            }
            Some(ToolDef {
                name,
                title: t.get("title").and_then(Value::as_str).map(str::to_string),
                description,
                input,
                effect,
                facade: t.get("facade").and_then(Value::as_str).map(str::to_string),
                facade_from: t.get("facadeFrom").and_then(Value::as_str).map(str::to_string),
            })
        })
        .collect();
    Some(AppTools { code: app.code.clone(), name: app.name.clone(), prefix, tools })
}

/// The name an assistant gives itself, for the window's banner.
fn client_name(info: &Value) -> Option<String> {
    let raw = info.get("title").and_then(Value::as_str).or_else(|| info.get("name").and_then(Value::as_str))?;
    let lower = raw.to_ascii_lowercase();
    let name = if lower.starts_with("claude") {
        "Claude".to_string()
    } else if lower.contains("codex") {
        "Codex".to_string()
    } else if lower.contains("qwen") {
        "Qwen Code".to_string()
    } else if lower.contains("lm studio") || lower.contains("lmstudio") {
        "LM Studio".to_string()
    } else {
        raw.trim().to_string()
    };
    let name: String = name.chars().filter(|c| !c.is_control()).take(60).collect();
    (!name.is_empty()).then_some(name)
}

/// What a page answered, as a tool result: only text and pictures pass.
fn tool_result(result: Value) -> Value {
    let Some(obj) = result.as_object() else { return text("Done.") };
    let content: Vec<Value> = obj
        .get("content")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter(|c| match c.get("type").and_then(Value::as_str) {
                    Some("text") => c.get("text").is_some_and(Value::is_string),
                    Some("image") => c.get("data").is_some_and(Value::is_string) && c.get("mimeType").is_some_and(Value::is_string),
                    _ => false,
                })
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    let mut out = Map::new();
    out.insert("content".into(), if content.is_empty() { json!([{ "type": "text", "text": "Done." }]) } else { Value::Array(content) });
    if let Some(s) = obj.get("structuredContent").filter(|s| s.is_object()) {
        out.insert("structuredContent".into(), s.clone());
    }
    if obj.get("isError") == Some(&Value::Bool(true)) {
        out.insert("isError".into(), Value::Bool(true));
    }
    Value::Object(out)
}

fn text(s: &str) -> Value {
    json!({ "content": [{ "type": "text", "text": s }] })
}

fn structured(value: Value) -> Value {
    json!({ "content": [{ "type": "text", "text": value.to_string() }], "structuredContent": value })
}

/// A tool that could not do it: said to the model, which tells the user.
fn failure(s: &str) -> Value {
    json!({ "content": [{ "type": "text", "text": s }], "isError": true })
}

fn ok(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn error(id: Value, code: i64, message: &str, data: Option<Value>) -> Value {
    let mut e = json!({ "code": code, "message": message });
    if let Some(d) = data {
        e["data"] = d;
    }
    json!({ "jsonrpc": "2.0", "id": id, "error": e })
}

fn send(out: &Sender<String>, message: Value) {
    let _ = out.send(message.to_string());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::automation::OnClient;
    use std::sync::mpsc;

    fn app() -> AppInfo {
        AppInfo { code: "QrcodeStudio".into(), name: "Kynoko QR Code Studio".into(), url: "https://qrcode-studio.kynoko.com/".into() }
    }

    fn declaration() -> Value {
        json!({ "v": 1, "prefix": "qr", "tools": [
            { "name": "qr_create", "title": "Create a QR code", "description": "Creates a QR code.", "effect": "write",
              "input": { "type": "object", "properties": { "format": { "type": "string" } }, "required": ["format"] }, "facade": "create" },
            { "name": "layout_steal", "description": "Not this app's prefix.", "input": { "type": "object" } },
            { "name": "qr_bad", "description": "A schema that is not an object.", "input": { "type": "string" } },
            { "name": "qr_odd", "description": "An unknown effect.", "effect": "delete", "input": { "type": "object" } },
        ] })
    }

    #[test]
    fn a_declaration_keeps_only_what_holds() {
        let parsed = parse_declaration(&app(), &declaration()).expect("declaration");
        assert_eq!(parsed.prefix, "qr");
        assert_eq!(parsed.tools.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), ["qr_create"]);
        assert_eq!(parsed.tools[0].facade.as_deref(), Some("create"));
        assert!(parse_declaration(&app(), &json!({ "prefix": "kynoko", "tools": [] })).is_none(), "the launcher's own prefix");
        assert!(parse_declaration(&app(), &json!({ "prefix": "QR", "tools": [] })).is_none());
    }

    #[test]
    fn names_are_made_readable() {
        assert_eq!(client_name(&json!({ "name": "claude-ai", "version": "1" })).as_deref(), Some("Claude"));
        assert_eq!(client_name(&json!({ "name": "codex-mcp-client" })).as_deref(), Some("Codex"));
        assert_eq!(client_name(&json!({ "name": "x", "title": "My Agent\u{7}" })).as_deref(), Some("My Agent"));
        assert_eq!(client_name(&json!({})), None);
    }

    #[test]
    fn only_text_and_pictures_come_back() {
        let r = tool_result(json!({ "content": [
            { "type": "text", "text": "ok" },
            { "type": "image", "data": "AAAA", "mimeType": "image/png" },
            { "type": "resource", "resource": { "uri": "file:///etc/passwd" } },
            { "type": "text", "text": 3 }
        ], "structuredContent": { "n": 1 }, "isError": false }));
        assert_eq!(r["content"].as_array().map(Vec::len), Some(2));
        assert_eq!(r["structuredContent"]["n"], 1);
        assert!(r.get("isError").is_none());
        assert_eq!(tool_result(json!("weird"))["content"][0]["text"], "Done.");
    }

    fn mcp_with(apps: Vec<AppTools>) -> Arc<Mcp> {
        let host = Host { apps: Box::new(Vec::new), open: Box::new(|_, _, _| Err("no browser in tests".into())), version: "0.0.0".into() };
        let mcp = Mcp::new(host);
        *mcp.tools.lock().unwrap() = Some((Instant::now(), apps));
        mcp
    }

    fn exchange(mcp: &Arc<Mcp>, request: Value) -> Value {
        let on_client: OnClient = Arc::new(|_, _, _, _| {});
        let automation = Automation::start(on_client).expect("listener");
        let (tx, rx) = mpsc::channel();
        mcp.handle(&automation, 1, &request.to_string(), &tx);
        serde_json::from_str(&rx.recv_timeout(Duration::from_secs(5)).expect("an answer")).expect("json")
    }

    #[test]
    fn legacy_and_modern_assistants_are_both_served() {
        let mcp = mcp_with(vec![parse_declaration(&app(), &declaration()).unwrap()]);
        let init = exchange(&mcp, json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": { "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "claude-ai", "version": "1" } } }));
        assert_eq!(init["result"]["protocolVersion"], "2025-06-18");
        assert_eq!(init["result"]["serverInfo"]["name"], "kynoko-launcher");
        let newer = exchange(&mcp, json!({ "jsonrpc": "2.0", "id": 2, "method": "initialize", "params": { "protocolVersion": "2099-01-01" } }));
        assert_eq!(newer["result"]["protocolVersion"], LEGACY[0]);
        let meta = json!({ META_VERSION: "2026-07-28", META_CLIENT: { "name": "x", "version": "1" } });
        let discover = exchange(&mcp, json!({ "jsonrpc": "2.0", "id": "d", "method": "server/discover", "params": { "_meta": meta } }));
        assert_eq!(discover["result"]["resultType"], "complete");
        assert_eq!(discover["result"]["supportedVersions"][0], "2026-07-28");
        let list = exchange(&mcp, json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/list", "params": { "_meta": meta } }));
        let names: Vec<&str> = list["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["kynoko_apps", "kynoko_open", "qr_create"]);
        assert_eq!(list["result"]["cacheScope"], "private");
        assert_eq!(list["result"]["tools"][2]["annotations"]["readOnlyHint"], false);
        let wrong = exchange(&mcp, json!({ "jsonrpc": "2.0", "id": 4, "method": "tools/list",
            "params": { "_meta": { META_VERSION: "1900-01-01" } } }));
        assert_eq!(wrong["error"]["code"], -32022);
        assert_eq!(wrong["error"]["data"]["requested"], "1900-01-01");
    }

    #[test]
    fn an_unknown_tool_or_method_is_an_error() {
        let mcp = mcp_with(vec![]);
        let call = exchange(&mcp, json!({ "jsonrpc": "2.0", "id": 5, "method": "tools/call", "params": { "name": "rm_rf", "arguments": {} } }));
        assert_eq!(call["error"]["code"], -32602);
        let method = exchange(&mcp, json!({ "jsonrpc": "2.0", "id": 6, "method": "resources/list" }));
        assert_eq!(method["error"]["code"], -32601);
    }

    #[test]
    fn calls_side_by_side_share_one_window() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        const ORIGIN: &str = "https://qrcode-studio.kynoko.com";
        let opened = Arc::new(AtomicUsize::new(0));
        let counter = opened.clone();
        let host = Host {
            apps: Box::new(Vec::new),
            open: Box::new(move |automation, code, _| {
                counter.fetch_add(1, Ordering::SeqCst);
                Ok(automation.new_page(code, ORIGIN))
            }),
            version: "0.0.0".into(),
        };
        let mcp = Mcp::new(host);
        let qr = parse_declaration(&app(), &declaration()).unwrap();
        let automation = Automation::start(Arc::new(|_, _, _, _| {})).expect("listener");
        // Three calls at once, while the user has not answered yet.
        let calls: Vec<_> = (0..3)
            .map(|_| {
                let (mcp, automation, qr) = (mcp.clone(), automation.clone(), qr.clone());
                thread::spawn(move || mcp.window(&automation, &qr, Some("create")))
            })
            .collect();
        let token = loop {
            if let Some(t) = automation.awaited_page("QrcodeStudio", OPEN_WAIT) {
                break t;
            }
            thread::sleep(Duration::from_millis(10));
        };
        thread::sleep(Duration::from_millis(200));
        // The user allows the assistant in the one window that opened.
        let stream = std::net::TcpStream::connect(("127.0.0.1", automation.port())).expect("tcp");
        let mut req = tungstenite::client::IntoClientRequest::into_client_request(format!("ws://127.0.0.1:{}/a/{token}", automation.port())).expect("request");
        req.headers_mut().insert("Origin", ORIGIN.parse().expect("header"));
        let _page = tungstenite::client(req, stream).expect("the page");
        for call in calls {
            assert_eq!(call.join().expect("call"), Ok(token.clone()));
        }
        // kynoko_open finds it as well.
        assert_eq!(mcp.window(&automation, &qr, None), Ok(token));
        assert_eq!(opened.load(Ordering::SeqCst), 1, "one window");
    }

    #[test]
    fn a_tool_that_cannot_open_its_app_says_why() {
        let mcp = mcp_with(vec![parse_declaration(&app(), &declaration()).unwrap()]);
        let call = exchange(&mcp, json!({ "jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": { "name": "qr_create", "arguments": { "format": "url" } } }));
        assert_eq!(call["result"]["isError"], true);
        assert!(call["result"]["content"][0]["text"].as_str().unwrap().contains("no browser in tests"));
    }
}
