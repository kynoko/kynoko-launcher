//! The freedesktop.org pieces Linux integration is made of (docs/SPEC.md,
//! sections 5 to 7): desktop entries, shared-mime-info packages and
//! `mimeapps.list`. Pure text in, text out, so they are tested on every
//! system; linux.rs does the file and process work around them.
#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use std::collections::BTreeMap;

/// The `[Desktop Entry]` group of a .desktop file (other groups ignored).
pub fn desktop_entry(text: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut inside = false;
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            inside = line == "[Desktop Entry]";
            continue;
        }
        if inside {
            if let Some((k, v)) = line.split_once('=') {
                out.entry(k.trim().to_string()).or_insert_with(|| v.trim().to_string());
            }
        }
    }
    out
}

/// An Exec line as argv: quotes honoured, field codes (%u %F...) dropped,
/// `%%` kept as a literal percent.
pub fn exec_argv(exec: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut started = false;
    let mut chars = exec.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                quoted = !quoted;
                started = true;
            }
            '\\' if quoted => {
                if let Some(n) = chars.next() {
                    current.push(n);
                }
            }
            c if c.is_whitespace() && !quoted => {
                if started {
                    args.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            c => {
                current.push(c);
                started = true;
            }
        }
    }
    if started {
        args.push(current);
    }
    args.into_iter()
        .filter(|a| !(a.len() == 2 && a.starts_with('%') && a != "%%"))
        // Flatpak's file-forwarding markers wrap the field codes: nothing left to wrap.
        .filter(|a| a != "@@" && a != "@@u")
        .map(|a| a.replace("%%", "%"))
        .collect()
}

/// The program a browser really is, behind `flatpak run <id>` or a wrapper.
pub fn program_of(argv: &[String]) -> String {
    let base = |s: &str| s.rsplit('/').next().unwrap_or(s).to_string();
    match argv.first().map(|a| base(a)) {
        Some(ref f) if f == "flatpak" => argv
            .iter()
            .skip_while(|a| *a != "run")
            .skip(1)
            .find(|a| !a.starts_with('-'))
            .map(|id| id.rsplit('.').next().unwrap_or(id).to_ascii_lowercase())
            .unwrap_or_default(),
        Some(f) => f,
        None => String::new(),
    }
}

/// A desktop entry, keys written in a fixed order.
pub fn render_entry(fields: &[(&str, String)], actions: &[(String, String, String)]) -> String {
    let mut out = String::from("[Desktop Entry]\n");
    for (k, v) in fields {
        out.push_str(&format!("{k}={v}\n"));
    }
    if !actions.is_empty() {
        let ids: Vec<&str> = actions.iter().map(|(id, _, _)| id.as_str()).collect();
        out.push_str(&format!("Actions={};\n", ids.join(";")));
        for (id, name, exec) in actions {
            out.push_str(&format!("\n[Desktop Action {id}]\nName={name}\nExec={exec}\n"));
        }
    }
    out
}

/// A value for Exec: a path with spaces or quotes must be quoted.
pub fn exec_quote(arg: &str) -> String {
    if arg.chars().all(|c| c.is_ascii_alphanumeric() || "/._-+:=".contains(c)) {
        arg.to_string()
    } else {
        format!("\"{}\"", arg.replace('\\', "\\\\").replace('"', "\\\"").replace('`', "\\`").replace('$', "\\$"))
    }
}

/// A shared-mime-info package declaring each `(mime, extension)` pair: types
/// the system already knows gain nothing but a glob, new ones exist.
pub fn mime_package(pairs: &[(String, String)]) -> String {
    let mut by_type: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (mime, ext) in pairs {
        by_type.entry(mime).or_default().push(ext);
    }
    let mut out = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<mime-info xmlns=\"http://www.freedesktop.org/standards/shared-mime-info\">\n",
    );
    for (mime, exts) in by_type {
        out.push_str(&format!("  <mime-type type=\"{mime}\">\n"));
        for ext in exts {
            out.push_str(&format!("    <glob pattern=\"*.{ext}\"/>\n"));
        }
        out.push_str("  </mime-type>\n");
    }
    out.push_str("</mime-info>\n");
    out
}

const DEFAULTS: &str = "[Default Applications]";
const ADDED: &str = "[Added Associations]";

/// The first desktop file `mimeapps.list` gives `mime` in `group`.
pub fn mimeapps_get(list: &str, group: &str, mime: &str) -> Option<String> {
    let mut inside = false;
    for line in list.lines().map(str::trim) {
        if line.starts_with('[') {
            inside = line == group;
        } else if inside {
            if let Some(v) = line.strip_prefix(&format!("{mime}=")) {
                return v.split(';').map(str::trim).find(|s| !s.is_empty()).map(str::to_string);
            }
        }
    }
    None
}

/// `list` with `mime` set to `value` in `group` (None removes the line),
/// every other line kept as it was.
pub fn mimeapps_set(list: &str, group: &str, mime: &str, value: Option<&str>) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut inside = false;
    let mut seen_group = false;
    let mut done = false;
    for line in list.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            if inside && !done {
                if let Some(v) = value {
                    out.push(format!("{mime}={v};"));
                }
                done = true;
            }
            inside = t == group;
            seen_group |= inside;
            out.push(line.to_string());
            continue;
        }
        if inside && t.starts_with(&format!("{mime}=")) {
            if !done {
                if let Some(v) = value {
                    out.push(format!("{mime}={v};"));
                }
                done = true;
            }
            continue;
        }
        out.push(line.to_string());
    }
    if !done {
        if let Some(v) = value {
            if !seen_group {
                if out.last().map(|l| !l.trim().is_empty()).unwrap_or(false) {
                    out.push(String::new());
                }
                out.push(group.to_string());
            }
            out.push(format!("{mime}={v};"));
        }
    }
    let mut text = out.join("\n");
    text.push('\n');
    text
}

/// `list` with `desktop` added to (or removed from) the "Open with" list of `mime`.
pub fn mimeapps_added(list: &str, mime: &str, desktop: &str, add: bool) -> String {
    let current: Vec<String> = {
        let mut inside = false;
        let mut found = Vec::new();
        for line in list.lines().map(str::trim) {
            if line.starts_with('[') {
                inside = line == ADDED;
            } else if inside {
                if let Some(v) = line.strip_prefix(&format!("{mime}=")) {
                    found = v.split(';').map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).collect();
                }
            }
        }
        found
    };
    let mut next: Vec<String> = current.into_iter().filter(|d| d != desktop).collect();
    if add {
        next.insert(0, desktop.to_string());
    }
    let joined = next.join(";");
    mimeapps_set(list, ADDED, mime, if next.is_empty() { None } else { Some(joined.as_str()) })
}

pub fn defaults_group() -> &'static str {
    DEFAULTS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries() {
        let text = "[Desktop Entry]\nName=Firefox\nName[fr]=Firefox\nExec=/usr/lib/firefox/firefox %u\nMimeType=text/html;x-scheme-handler/https;\n[Desktop Action new-window]\nExec=nope\n";
        let e = desktop_entry(text);
        assert_eq!(e["Name"], "Firefox");
        assert_eq!(e["Exec"], "/usr/lib/firefox/firefox %u");
        assert!(e["MimeType"].contains("x-scheme-handler/https"));
    }

    #[test]
    fn argv() {
        assert_eq!(exec_argv("/usr/bin/google-chrome-stable %U"), ["/usr/bin/google-chrome-stable"]);
        assert_eq!(exec_argv(r#""/opt/My Browser/b" --x=1 %u"#), ["/opt/My Browser/b", "--x=1"]);
        assert_eq!(exec_argv("app 100%%"), ["app", "100%"]);
        let flatpak = exec_argv("/usr/bin/flatpak run --branch=stable --arch=x86_64 --command=firefox org.mozilla.firefox @@u %u @@");
        assert_eq!(program_of(&flatpak), "firefox");
        assert_eq!(flatpak.last().map(String::as_str), Some("org.mozilla.firefox"));
        assert_eq!(program_of(&exec_argv("/usr/bin/brave-browser-stable %U")), "brave-browser-stable");
    }

    #[test]
    fn quoting() {
        assert_eq!(exec_quote("/opt/kynoko/kynoko-launcher"), "/opt/kynoko/kynoko-launcher");
        assert_eq!(exec_quote("/home/u/Mes Apps/K.AppImage"), "\"/home/u/Mes Apps/K.AppImage\"");
        assert_eq!(exec_quote("a$b"), "\"a\\$b\"");
    }

    #[test]
    fn package() {
        let xml = mime_package(&[("text/csv".into(), "csv".into()), ("application/x-kynoko-kphoto".into(), "kphoto".into())]);
        assert!(xml.contains("<mime-type type=\"text/csv\">\n    <glob pattern=\"*.csv\"/>"));
        assert!(xml.contains("application/x-kynoko-kphoto"));
    }

    #[test]
    fn mimeapps() {
        let list = "[Default Applications]\ntext/html=firefox.desktop;\n\n[Added Associations]\ntext/csv=libreoffice-calc.desktop;\n";
        assert_eq!(mimeapps_get(list, DEFAULTS, "text/html").as_deref(), Some("firefox.desktop"));
        assert_eq!(mimeapps_get(list, DEFAULTS, "text/csv"), None);
        let set = mimeapps_set(list, DEFAULTS, "text/csv", Some("kynoko-launcher-open-Office.desktop"));
        assert_eq!(mimeapps_get(&set, DEFAULTS, "text/csv").as_deref(), Some("kynoko-launcher-open-Office.desktop"));
        assert!(set.contains("text/html=firefox.desktop;"), "other lines kept");
        let back = mimeapps_set(&set, DEFAULTS, "text/csv", None);
        assert_eq!(mimeapps_get(&back, DEFAULTS, "text/csv"), None);
        // A missing group is created.
        let fresh = mimeapps_set("", DEFAULTS, "text/csv", Some("x.desktop"));
        assert_eq!(fresh, "[Default Applications]\ntext/csv=x.desktop;\n");
        // Open with: added first, removed alone.
        let added = mimeapps_added(list, "text/csv", "k.desktop", true);
        assert!(added.contains("text/csv=k.desktop;libreoffice-calc.desktop;"));
        let removed = mimeapps_added(&added, "text/csv", "k.desktop", false);
        assert!(removed.contains("text/csv=libreoffice-calc.desktop;"));
    }
}
