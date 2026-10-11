//! Assistants: telling the AI assistants installed on the computer where the
//! Kynoko apps are, by adding one entry, `kynoko`, to their MCP
//! configuration: `kynoko-launcher mcp` (mcp_stdio.rs).
//!
//! The files are the user's, and other servers live in them. So: only our
//! entry is added or removed, the rest is kept as it is (key order
//! included); a file that is not plain JSON (comments) is left alone, and
//! the window then shows what to add by hand; the file as it was is copied
//! aside once (`<file>.kynoko-backup`) before the first change. Every entry
//! is in the inventory (Artefact::AssistantEntry): removing everything, or
//! uninstalling, takes it away again. An isolated run (tests) works in a
//! home of its own, never in the user's.

use std::fs;
use std::path::PathBuf;

use serde::Serialize;
use serde_json::{json, Map, Value};

use crate::settings::{Artefact, Inventory};

/// The name of our entry in every assistant's configuration.
const KEY: &str = "kynoko";
const BACKUP: &str = "kynoko-backup";

#[derive(Clone, Copy, PartialEq, Debug)]
enum Format {
    /// `{ "mcpServers": { "kynoko": { "command", "args" } } }`: Claude
    /// Desktop, LM Studio, Cursor, Qwen Code.
    McpServers,
    /// `{ "servers": { "kynoko": { "type": "stdio", ... } } }`: VS Code.
    VsCode,
    /// `[mcp_servers.kynoko]` in `config.toml`: Codex.
    CodexToml,
}

struct Client {
    id: &'static str,
    name: &'static str,
    format: Format,
    /// The assistant's own folder: there means installed.
    dir: Option<PathBuf>,
    file: &'static str,
}

/// One assistant, as the window shows it.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AssistantView {
    pub id: String,
    pub name: String,
    pub installed: bool,
    pub connected: bool,
}

fn home() -> Option<PathBuf> {
    if crate::settings::isolated() {
        return Some(crate::settings::dir().join("home"));
    }
    dirs::home_dir()
}

fn config() -> Option<PathBuf> {
    if crate::settings::isolated() {
        return Some(crate::settings::dir().join("home").join("config"));
    }
    dirs::config_dir()
}

fn clients() -> Vec<Client> {
    vec![
        Client { id: "claude", name: "Claude Desktop", format: Format::McpServers, dir: config().map(|c| c.join("Claude")), file: "claude_desktop_config.json" },
        Client { id: "lmstudio", name: "LM Studio", format: Format::McpServers, dir: home().map(|h| h.join(".lmstudio")), file: "mcp.json" },
        Client { id: "cursor", name: "Cursor", format: Format::McpServers, dir: home().map(|h| h.join(".cursor")), file: "mcp.json" },
        Client { id: "qwen", name: "Qwen Code", format: Format::McpServers, dir: home().map(|h| h.join(".qwen")), file: "settings.json" },
        Client { id: "vscode", name: "Visual Studio Code", format: Format::VsCode, dir: config().map(|c| c.join("Code").join("User")), file: "mcp.json" },
        Client { id: "codex", name: "Codex", format: Format::CodexToml, dir: home().map(|h| h.join(".codex")), file: "config.toml" },
    ]
}

fn client(id: &str) -> Result<Client, String> {
    clients().into_iter().find(|c| c.id == id).ok_or_else(|| format!("unknown assistant {id}"))
}

/// The program an assistant starts: the installed launcher (on Linux, the
/// AppImage's installed copy).
pub fn launcher_exe() -> String {
    #[cfg(target_os = "linux")]
    {
        crate::linux::exe()
    }
    #[cfg(not(target_os = "linux"))]
    {
        std::env::current_exe().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default()
    }
}

/// The assistants this launcher knows, installed or not, and whether ours
/// is in their configuration.
pub fn views() -> Vec<AssistantView> {
    clients()
        .into_iter()
        .map(|c| {
            let installed = c.dir.as_ref().is_some_and(|d| d.is_dir());
            let connected = c.dir.as_ref().is_some_and(|d| has_entry(&d.join(c.file), c.format));
            AssistantView { id: c.id.into(), name: c.name.into(), installed, connected }
        })
        .collect()
}

/// What to add by hand to another assistant's configuration.
pub fn snippet() -> String {
    let entry = json!({ "mcpServers": { KEY: { "command": launcher_exe(), "args": ["mcp"] } } });
    serde_json::to_string_pretty(&entry).unwrap_or_default()
}

pub fn connect(id: &str, inventory: &mut Inventory) -> Result<(), String> {
    let c = client(id)?;
    let dir = c.dir.as_ref().filter(|d| d.is_dir()).ok_or_else(|| format!("{} is not installed", c.name))?;
    let path = dir.join(c.file);
    let before = fs::read_to_string(&path).ok();
    let after = with_entry(before.as_deref(), c.format, &launcher_exe())?;
    if let Some(before) = &before {
        let backup = path.with_extension(format!("{}.{BACKUP}", path.extension().and_then(|e| e.to_str()).unwrap_or("")));
        if !backup.exists() {
            fs::write(&backup, before).map_err(|e| e.to_string())?;
            inventory.record(Artefact::File { path: backup.to_string_lossy().into_owned() }).map_err(|e| e.to_string())?;
        }
    }
    write_safely(&path, &after)?;
    let artefact = Artefact::AssistantEntry { path: path.to_string_lossy().into_owned(), client: c.id.into() };
    if !inventory.artefacts.contains(&artefact) {
        inventory.record(artefact).map_err(|e| e.to_string())?;
    }
    Ok(())
}

pub fn disconnect(id: &str, inventory: &mut Inventory) -> Result<(), String> {
    let c = client(id)?;
    let Some(dir) = c.dir else { return Ok(()) };
    let path = dir.join(c.file);
    remove_entry(&path.to_string_lossy(), c.id)?;
    let artefact = Artefact::AssistantEntry { path: path.to_string_lossy().into_owned(), client: c.id.into() };
    inventory.forget(&artefact).map_err(|e| e.to_string())
}

/// Takes our entry out of one assistant's configuration (also what removing
/// everything does with an Artefact::AssistantEntry).
pub fn remove_entry(path: &str, client_id: &str) -> Result<(), String> {
    let Some(format) = clients().into_iter().find(|c| c.id == client_id).map(|c| c.format) else { return Ok(()) };
    let path = PathBuf::from(path);
    let Ok(text) = fs::read_to_string(&path) else { return Ok(()) };
    match without_entry(&text, format)? {
        Some(after) => write_safely(&path, &after),
        None => Ok(()),
    }
}

/* -------------------------------- formats -------------------------------- */

fn section(format: Format) -> &'static str {
    if format == Format::VsCode {
        "servers"
    } else {
        "mcpServers"
    }
}

fn has_entry(path: &std::path::Path, format: Format) -> bool {
    let Ok(text) = fs::read_to_string(path) else { return false };
    match format {
        Format::CodexToml => text.lines().any(|l| l.trim() == "[mcp_servers.kynoko]"),
        _ => serde_json::from_str::<Value>(&text).ok().is_some_and(|v| v.get(section(format)).and_then(|s| s.get(KEY)).is_some()),
    }
}

/// The configuration with our entry in (replacing an older one of ours).
fn with_entry(before: Option<&str>, format: Format, exe: &str) -> Result<String, String> {
    if format == Format::CodexToml {
        let kept = match before {
            Some(text) => without_entry(text, format)?.unwrap_or_else(|| text.to_string()),
            None => String::new(),
        };
        let mut out = kept.trim_end().to_string();
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        // A literal string: a Windows path needs no escaping there.
        if exe.contains('\'') {
            return Err("the launcher's path holds a quote, which TOML cannot take as it is".into());
        }
        out.push_str(&format!("[mcp_servers.kynoko]\ncommand = '{exe}'\nargs = [\"mcp\"]\n"));
        return Ok(out);
    }
    let mut root: Value = match before.map(str::trim).filter(|t| !t.is_empty()) {
        Some(text) => serde_json::from_str(text).map_err(|_| "NOT_PLAIN_JSON".to_string())?,
        None => json!({}),
    };
    let Some(obj) = root.as_object_mut() else { return Err("NOT_PLAIN_JSON".into()) };
    let servers = obj.entry(section(format)).or_insert_with(|| Value::Object(Map::new()));
    let Some(servers) = servers.as_object_mut() else { return Err("NOT_PLAIN_JSON".into()) };
    let entry = if format == Format::VsCode {
        json!({ "type": "stdio", "command": exe, "args": ["mcp"] })
    } else {
        json!({ "command": exe, "args": ["mcp"] })
    };
    servers.insert(KEY.into(), entry);
    serde_json::to_string_pretty(&root).map(|s| s + "\n").map_err(|e| e.to_string())
}

/// The configuration without our entry; None when it had none.
fn without_entry(text: &str, format: Format) -> Result<Option<String>, String> {
    if format == Format::CodexToml {
        let mut out = Vec::new();
        let mut skipping = false;
        let mut found = false;
        for line in text.lines() {
            let t = line.trim();
            if t.starts_with('[') {
                skipping = t == "[mcp_servers.kynoko]";
                found |= skipping;
            }
            if !skipping {
                out.push(line);
            }
        }
        if !found {
            return Ok(None);
        }
        let joined = out.join("\n");
        return Ok(Some(if joined.trim().is_empty() { String::new() } else { joined.trim_end().to_string() + "\n" }));
    }
    let mut root: Value = serde_json::from_str(text).map_err(|_| "NOT_PLAIN_JSON".to_string())?;
    let key = section(format);
    let Some(servers) = root.get_mut(key).and_then(Value::as_object_mut) else { return Ok(None) };
    if servers.remove(KEY).is_none() {
        return Ok(None);
    }
    serde_json::to_string_pretty(&root).map(|s| Some(s + "\n")).map_err(|e| e.to_string())
}

/// Written beside, then put in place: never half a configuration.
fn write_safely(path: &std::path::Path, text: &str) -> Result<(), String> {
    let tmp = path.with_extension("kynoko-tmp");
    fs::write(&tmp, text).map_err(|e| e.to_string())?;
    fs::rename(&tmp, path).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        e.to_string()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXE: &str = r"C:\Users\x\AppData\Local\Kynoko Launcher\kynoko-launcher.exe";

    #[test]
    fn json_keeps_the_rest_and_its_order() {
        let before = r#"{ "zeta": 1, "mcpServers": { "other": { "command": "x" } }, "alpha": true }"#;
        let after = with_entry(Some(before), Format::McpServers, EXE).unwrap();
        let v: Value = serde_json::from_str(&after).unwrap();
        assert_eq!(v["mcpServers"]["kynoko"]["args"][0], "mcp");
        assert_eq!(v["mcpServers"]["kynoko"]["command"], EXE);
        assert_eq!(v["mcpServers"]["other"]["command"], "x");
        let keys: Vec<&String> = v.as_object().unwrap().keys().collect();
        assert_eq!(keys, ["zeta", "mcpServers", "alpha"], "order kept");
        let back = without_entry(&after, Format::McpServers).unwrap().unwrap();
        let v: Value = serde_json::from_str(&back).unwrap();
        assert!(v["mcpServers"].get("kynoko").is_none());
        assert_eq!(v["mcpServers"]["other"]["command"], "x");
        assert!(without_entry(&back, Format::McpServers).unwrap().is_none(), "nothing more to remove");
    }

    #[test]
    fn a_file_with_comments_is_left_alone() {
        let jsonc = "{\n  // my servers\n  \"servers\": {}\n}";
        assert_eq!(with_entry(Some(jsonc), Format::VsCode, EXE), Err("NOT_PLAIN_JSON".into()));
        assert!(with_entry(Some("[1,2]"), Format::McpServers, EXE).is_err());
    }

    #[test]
    fn vs_code_takes_its_own_shape() {
        let after = with_entry(None, Format::VsCode, EXE).unwrap();
        let v: Value = serde_json::from_str(&after).unwrap();
        assert_eq!(v["servers"]["kynoko"]["type"], "stdio");
    }

    #[test]
    fn toml_adds_and_removes_one_section() {
        let before = "model = \"o4\"\n\n[mcp_servers.other]\ncommand = \"x\"\n";
        let after = with_entry(Some(before), Format::CodexToml, EXE).unwrap();
        assert!(after.contains("[mcp_servers.other]\ncommand = \"x\""));
        assert!(after.contains(&format!("[mcp_servers.kynoko]\ncommand = '{EXE}'\nargs = [\"mcp\"]")));
        // Twice: still one section.
        let again = with_entry(Some(&after), Format::CodexToml, EXE).unwrap();
        assert_eq!(again.matches("[mcp_servers.kynoko]").count(), 1);
        let back = without_entry(&again, Format::CodexToml).unwrap().unwrap();
        assert_eq!(back, before);
    }
}
