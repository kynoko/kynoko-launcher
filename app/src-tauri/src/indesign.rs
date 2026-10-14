//! The InDesign installed on this computer, to turn an InDesign document
//! into IDML for an app in a Kynoko window (docs/SPEC.md, section 10).
//!
//! WHY THE LAUNCHER. An InDesign document (.indd) is InDesign's own
//! database: closed, undocumented, different in every version. Only InDesign
//! reads it. Its IDML export, an open format, is what a layout app imports.
//! A page cannot run a program; the launcher, which runs on the computer,
//! can drive the user's own InDesign, with the user's own licence, and the
//! document never leaves the computer.
//!
//! WHAT IS GIVEN. `indesign_versions`: the InDesign versions installed,
//! newest first (major version and release name), so that the page can tell
//! whether one can open the document it holds (an InDesign opens the
//! documents of its own version and older ones). `indesign_convert`: the
//! bytes of a document, sent as the request's raw body, give the bytes of
//! its IDML. The page names no file and sends no code: what InDesign runs is
//! this module's script, fixed. It opens the document without a window,
//! exports it as IDML, closes it without saving, and holds InDesign's dialogs
//! back meanwhile. The newest InDesign does it. An InDesign that was not
//! running is quit afterwards; one that was keeps the user's documents as
//! they were.
//!
//! HOW. Windows: InDesign's COM server (the ProgID InDesign.Application.<release>),
//! driven by Windows PowerShell (DoScript, in JavaScript). macOS: InDesign's
//! scripting, from osascript in JavaScript for Automation; macOS asks the
//! user once whether Kynoko Launcher may control InDesign (the bundle's
//! NSAppleEventsUsageDescription, tools/mac-info-plist.mjs). Linux has no
//! InDesign.
//!
//! One conversion at a time. The files live in a folder of their own in the
//! system's temporary directory, removed afterwards.

use serde::Serialize;
use std::fs;
use std::path::Path;
use std::process::Child;
use std::sync::{Mutex, TryLockError};
use std::time::{Duration, Instant};

/// The largest document taken: an InDesign document with its pictures
/// embedded weighs hundreds of megabytes at most.
pub const MAX_DOCUMENT: usize = 1 << 30;

/// Starting InDesign, then opening and exporting a large document: minutes,
/// never this long.
const TIMEOUT: Duration = Duration::from_secs(600);

/// What InDesign runs: the document at arguments[0] exported as IDML to
/// arguments[1], without a window, InDesign's dialogs held back meanwhile.
const EXPORT_SCRIPT: &str = r#"var level = app.scriptPreferences.userInteractionLevel;
app.scriptPreferences.userInteractionLevel = UserInteractionLevels.NEVER_INTERACT;
var doc = null;
try {
  doc = app.open(File(arguments[0]), false);
  doc.exportFile(ExportFormat.INDESIGN_MARKUP, File(arguments[1]));
} finally {
  if (doc !== null) {
    try {
      doc.close(SaveOptions.NO);
    } catch (e) {}
  }
  app.scriptPreferences.userInteractionLevel = level;
}
"#;

/// One InDesign of this computer.
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct InDesign {
    /// The major version: 14 for InDesign CC 2019, 19 for InDesign 2024.
    pub major: u32,
    /// The release as it was sold: "CC 2019", "2024".
    pub release: String,
    /// How to reach it: its ProgID on Windows, its application's path on macOS.
    #[serde(skip)]
    handle: String,
}

/// Why a conversion did not happen, as the page is told (it says it in its
/// own words): `none` (no InDesign here), `busy` (another conversion under
/// way), `too-large`, `unavailable` (InDesign did not start: not signed in,
/// not licensed), `denied` (macOS: the user refused that the launcher control
/// InDesign), `unreadable` (InDesign could not open or export the document:
/// saved by a later version, or damaged), `timeout`, `failed`.
pub type Refusal = &'static str;

static CONVERTING: Mutex<()> = Mutex::new(());

/// The InDesign versions of this computer, newest first, one per major version.
pub fn installed() -> Vec<InDesign> {
    let mut found = find();
    found.sort_by(|a, b| b.major.cmp(&a.major));
    found.dedup_by_key(|i| i.major);
    found
}

/// The IDML of an InDesign document, exported by the newest InDesign here.
pub fn convert(document: &[u8]) -> Result<Vec<u8>, Refusal> {
    if !accepts(document.len()) {
        return Err("too-large");
    }
    let Some(app) = installed().into_iter().next() else {
        return Err("none");
    };
    // A conversion that panicked leaves the lock poisoned, not held: the next one goes on.
    let _one = match CONVERTING.try_lock() {
        Ok(guard) => guard,
        Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
        Err(TryLockError::WouldBlock) => return Err("busy"),
    };
    let dir = std::env::temp_dir().join(format!("kynoko-indesign-{}", random_name()));
    fs::create_dir_all(&dir).map_err(|_| "failed")?;
    let result = (|| {
        let script = dir.join("export.jsx");
        let input = dir.join("document.indd");
        let output = dir.join("document.idml");
        fs::write(&script, EXPORT_SCRIPT).map_err(|_| "failed")?;
        fs::write(&input, document).map_err(|_| "failed")?;
        run(&app, &script, &input, &output)?;
        match fs::read(&output) {
            // An IDML is a zip: anything else is not one.
            Ok(bytes) if bytes.starts_with(b"PK") => Ok(bytes),
            _ => Err("failed"),
        }
    })();
    let _ = fs::remove_dir_all(&dir);
    result
}

fn accepts(size: usize) -> bool {
    size <= MAX_DOCUMENT
}

/// The release of a major version, as Adobe named it.
fn release(major: u32) -> String {
    match major {
        5 => "CS3".into(),
        6 => "CS4".into(),
        7 => "CS5".into(),
        8 => "CS6".into(),
        9 => "CC".into(),
        10 => "CC 2014".into(),
        11 => "CC 2015".into(),
        12 => "CC 2017".into(),
        13 => "CC 2018".into(),
        14 => "CC 2019".into(),
        // From version 15, a year: 2020, then one a version.
        m => (2005 + m).to_string(),
    }
}

/// The major version of a release's year: 2019 is 14 (CC 2019), 2024 is 19.
fn major_of_year(year: u32) -> Option<u32> {
    match year {
        2014 => Some(10),
        2015 => Some(11),
        2017 => Some(12),
        2018 => Some(13),
        2019 => Some(14),
        y if (2020..2100).contains(&y) => Some(y - 2005),
        _ => None,
    }
}

/// The major version a ProgID's suffix names: "CC.2019", "2024", "CC", "CS6".
fn major_of_progid(suffix: &str) -> Option<u32> {
    let s = suffix.trim();
    if let Some(cs) = s.strip_prefix("CS") {
        return cs.parse::<u32>().ok().filter(|n| (3..=6).contains(n)).map(|n| n + 2);
    }
    if s == "CC" {
        return Some(9);
    }
    let year = s.strip_prefix("CC.").unwrap_or(s);
    year.parse::<u32>().ok().and_then(major_of_year)
}

/// The program of a COM server's LocalServer32 value: quoted, or up to ".exe", arguments left out.
#[cfg_attr(not(windows), allow(dead_code))]
fn exe_of(server: &str) -> &str {
    let s = server.trim();
    if let Some(rest) = s.strip_prefix('"') {
        return rest.split('"').next().unwrap_or(rest);
    }
    match s.to_ascii_lowercase().find(".exe") {
        Some(i) => &s[..i + 4],
        None => s,
    }
}

fn random_name() -> String {
    let mut bytes = [0u8; 8];
    getrandom::getrandom(&mut bytes).expect("os randomness");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Waits for the program driving InDesign: its exit code, or `timeout`
/// (it is then stopped; an InDesign it started may stay open).
#[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
fn finish(mut child: Child) -> Result<(i32, String), Refusal> {
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let mut err = String::new();
                if let Some(mut pipe) = child.stderr.take() {
                    let _ = std::io::Read::read_to_string(&mut pipe, &mut err);
                }
                return Ok((status.code().unwrap_or(-1), err));
            }
            Ok(None) => {}
            Err(_) => return Err("failed"),
        }
        if start.elapsed() > TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            return Err("timeout");
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/* --------------------------------- Windows --------------------------------- */

/// Drives InDesign's COM server. Exits 3 when InDesign does not start, 4 when
/// it cannot open or export the document. The paths and the ProgID come in
/// the environment, never in the code.
#[cfg(windows)]
const DRIVER_POWERSHELL: &str = r#"$ErrorActionPreference = 'Stop'
# Started for this conversion: an InDesign process that was not there before.
# (One still closing after a previous conversion is not the one COM answers with.)
$before = @(Get-Process -Name 'InDesign' -ErrorAction SilentlyContinue | ForEach-Object { $_.Id })
try {
  $app = New-Object -ComObject $env:KYNOKO_INDESIGN_PROGID
} catch {
  exit 3
}
$started = @(Get-Process -Name 'InDesign' -ErrorAction SilentlyContinue | Where-Object { $before -notcontains $_.Id }).Count -gt 0
$failed = 0
try {
  $code = [IO.File]::ReadAllText($env:KYNOKO_INDESIGN_SCRIPT)
  # 1246973031: ScriptLanguage.JAVASCRIPT.
  $null = $app.DoScript($code, 1246973031, [object[]]@($env:KYNOKO_INDESIGN_IN, $env:KYNOKO_INDESIGN_OUT))
} catch {
  $failed = 4
} finally {
  # 1852776480: SaveOptions.NO. An InDesign started for this conversion is quit.
  if ($started) {
    try { $app.Quit(1852776480) } catch {}
  }
}
exit $failed
"#;

#[cfg(windows)]
fn find() -> Vec<InDesign> {
    use winreg::enums::HKEY_CLASSES_ROOT;
    use winreg::RegKey;
    let root = RegKey::predef(HKEY_CLASSES_ROOT);
    let mut out = Vec::new();
    for name in root.enum_keys().flatten() {
        let Some(suffix) = name.strip_prefix("InDesign.Application.") else { continue };
        let Some(major) = major_of_progid(suffix) else { continue };
        // The server must still be on disk: an uninstalled InDesign may leave its keys behind.
        let Ok(clsid) = root.open_subkey(format!("{name}\\CLSID")).and_then(|k| k.get_value::<String, _>("")) else { continue };
        let Ok(server) = root.open_subkey(format!("CLSID\\{clsid}\\LocalServer32")).and_then(|k| k.get_value::<String, _>("")) else { continue };
        if !Path::new(exe_of(&server)).is_file() {
            continue;
        }
        out.push(InDesign { major, release: release(major), handle: name });
    }
    out
}

#[cfg(windows)]
fn run(app: &InDesign, script: &Path, input: &Path, output: &Path) -> Result<(), Refusal> {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    // Windows PowerShell by its full path: never one found earlier on the PATH.
    let root = std::env::var_os("SystemRoot").map(std::path::PathBuf::from).unwrap_or_else(|| std::path::PathBuf::from(r"C:\Windows"));
    let shell = root.join(r"System32\WindowsPowerShell\v1.0\powershell.exe");
    let child = Command::new(shell)
        .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-EncodedCommand", &utf16_base64(DRIVER_POWERSHELL)])
        .env("KYNOKO_INDESIGN_PROGID", &app.handle)
        .env("KYNOKO_INDESIGN_SCRIPT", script)
        .env("KYNOKO_INDESIGN_IN", input)
        .env("KYNOKO_INDESIGN_OUT", output)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|_| "failed")?;
    match finish(child)? {
        (0, _) => Ok(()),
        (3, _) => Err("unavailable"),
        (4, _) => Err("unreadable"),
        (_, err) => {
            eprintln!("kynoko-launcher: InDesign conversion failed: {}", err.trim());
            Err("failed")
        }
    }
}

/// PowerShell's -EncodedCommand: the script in UTF-16LE, in base64.
#[cfg_attr(not(windows), allow(dead_code))]
fn utf16_base64(text: &str) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes: Vec<u8> = text.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (chunk[0] as u32) << 16 | (*chunk.get(1).unwrap_or(&0) as u32) << 8 | *chunk.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/* ---------------------------------- macOS ---------------------------------- */

/// Drives InDesign's scripting from JavaScript for Automation: the
/// application (argv[0]) runs the export script (argv[1]) on the document
/// (argv[2]) towards the IDML (argv[3]). Fails with "denied" when the user
/// refused the control of InDesign, "unreadable" when the document could not
/// be opened or exported.
#[cfg(target_os = "macos")]
const DRIVER_JXA: &str = r#"function run(argv) {
  var inDesign = Application(argv[0]);
  var wasRunning = inDesign.running();
  var failure = null;
  try {
    inDesign.doScript(Path(argv[1]), { language: 'javascript', withArguments: [argv[2], argv[3]] });
  } catch (e) {
    failure = /-1743|not allowed|not authori[sz]ed/i.test(String(e)) ? 'denied' : 'unreadable';
  }
  if (!wasRunning && failure !== 'denied') {
    try {
      inDesign.quit({ saving: 'no' });
    } catch (e) {}
  }
  if (failure) throw new Error('kynoko:' + failure);
}"#;

#[cfg(target_os = "macos")]
fn find() -> Vec<InDesign> {
    // Adobe installs "/Applications/Adobe InDesign <year>/Adobe InDesign <year>.app".
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir("/Applications") else { return out };
    for folder in entries.flatten() {
        if !folder.file_name().to_string_lossy().starts_with("Adobe InDesign") {
            continue;
        }
        let Ok(inside) = fs::read_dir(folder.path()) else { continue };
        for app in inside.flatten() {
            let name = app.file_name().to_string_lossy().to_string();
            if !(name.starts_with("Adobe InDesign") && name.ends_with(".app")) {
                continue;
            }
            let Ok(info) = plist::Value::from_file(app.path().join("Contents/Info.plist")) else { continue };
            let Some(dict) = info.as_dictionary() else { continue };
            if dict.get("CFBundleIdentifier").and_then(|v| v.as_string()) != Some("com.adobe.InDesign") {
                continue;
            }
            let version = dict.get("CFBundleShortVersionString").and_then(|v| v.as_string()).unwrap_or("");
            let Some(major) = version.split('.').next().and_then(|m| m.trim().parse::<u32>().ok()) else { continue };
            out.push(InDesign { major, release: release(major), handle: app.path().to_string_lossy().to_string() });
        }
    }
    out
}

#[cfg(target_os = "macos")]
fn run(app: &InDesign, script: &Path, input: &Path, output: &Path) -> Result<(), Refusal> {
    use std::process::{Command, Stdio};
    let child = Command::new("/usr/bin/osascript")
        .args(["-l", "JavaScript", "-e", DRIVER_JXA])
        .arg(&app.handle)
        .arg(script)
        .arg(input)
        .arg(output)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|_| "failed")?;
    match finish(child)? {
        (0, _) => Ok(()),
        (_, err) if err.contains("kynoko:denied") => Err("denied"),
        (_, err) if err.contains("kynoko:unreadable") => Err("unreadable"),
        (_, err) => {
            eprintln!("kynoko-launcher: InDesign conversion failed: {}", err.trim());
            Err("failed")
        }
    }
}

/* ---------------------------------- Linux ---------------------------------- */

#[cfg(not(any(windows, target_os = "macos")))]
fn find() -> Vec<InDesign> {
    Vec::new()
}

#[cfg(not(any(windows, target_os = "macos")))]
fn run(_: &InDesign, _: &Path, _: &Path, _: &Path) -> Result<(), Refusal> {
    Err("none")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_the_releases() {
        assert_eq!(release(8), "CS6");
        assert_eq!(release(14), "CC 2019");
        assert_eq!(release(15), "2020");
        assert_eq!(release(19), "2024");
    }

    #[test]
    fn reads_a_progid_suffix() {
        assert_eq!(major_of_progid("CC.2019"), Some(14));
        assert_eq!(major_of_progid("CC.2018"), Some(13));
        assert_eq!(major_of_progid("2024"), Some(19));
        assert_eq!(major_of_progid("CC"), Some(9));
        assert_eq!(major_of_progid("CS6"), Some(8));
        assert_eq!(major_of_progid("Server"), None);
        assert_eq!(major_of_progid("2016"), None);
    }

    #[test]
    fn finds_the_program_of_a_com_server() {
        assert_eq!(exe_of(r"D:\Program Files\Adobe\Adobe InDesign CC 2019\InDesign.exe"), r"D:\Program Files\Adobe\Adobe InDesign CC 2019\InDesign.exe");
        assert_eq!(exe_of(r#""C:\Adobe\InDesign.exe" /automation"#), r"C:\Adobe\InDesign.exe");
        assert_eq!(exe_of(r"C:\Adobe\InDesign.EXE /automation"), r"C:\Adobe\InDesign.EXE");
    }

    #[test]
    fn encodes_a_powershell_command() {
        // "ab" in UTF-16LE is 61 00 62 00.
        assert_eq!(utf16_base64("ab"), "YQBiAA==");
        assert_eq!(utf16_base64("é"), "6QA=");
    }

    #[test]
    fn refuses_a_document_too_large() {
        assert!(accepts(MAX_DOCUMENT));
        assert!(!accepts(MAX_DOCUMENT + 1));
    }

    /// The real thing, on a computer with InDesign: KYNOKO_INDD_SAMPLE names an
    /// InDesign document this InDesign can open.
    ///   KYNOKO_INDD_SAMPLE=path/to/doc.indd cargo test -- --ignored converts_with
    #[test]
    #[ignore]
    fn converts_with_the_installed_indesign() {
        let sample = std::env::var("KYNOKO_INDD_SAMPLE").expect("KYNOKO_INDD_SAMPLE");
        assert!(!installed().is_empty(), "no InDesign on this computer");
        let idml = convert(&fs::read(sample).expect("sample")).expect("converted");
        assert!(idml.starts_with(b"PK"));
        // The package's first entry is its media type, stored.
        let head = String::from_utf8_lossy(&idml[..idml.len().min(200)]).to_string();
        assert!(head.contains("mimetype") && head.contains("application/vnd.adobe.indesign-idml-package"));
        // A document InDesign cannot read is refused as such.
        assert_eq!(convert(b"not an InDesign document"), Err("unreadable"));
    }
}
