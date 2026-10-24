//! The ffmpeg installed on this computer, to turn a film an app made in WebM
//! into an MP4 in H.264 (docs/SPEC.md, section 8, "App sessions").
//!
//! WHY THE LAUNCHER. A Kynoko app encodes video with the browser's own
//! encoders, under the browser maker's licences. H.264 is the one format a
//! browser may lack: Firefox on Linux encodes it with the system's ffmpeg
//! libraries, and fails without them, or when the page's process reads
//! numbers with a decimal comma (its libx264 then refuses the level "3.1").
//! The app then writes a WebM, which plays everywhere a browser does but not
//! everywhere a film is watched. The launcher, which runs on the computer,
//! hands that WebM to the computer's own ffmpeg, with the user's own codec
//! libraries: nothing is bundled, and the film never leaves the computer.
//!
//! WHAT IS GIVEN. A WebM, sent as a request's raw body, becomes an MP4: H.264
//! High picture, AAC sound, its index at the front. ffmpeg reads nothing but
//! that one local file, through the Matroska/WebM demuxer and the VP8, VP9,
//! Opus and Vorbis decoders (its own whitelists): a page cannot make it open
//! another file, another protocol or another decoder. It runs with the C
//! locale, whatever the user's, so that no option is read with a decimal
//! comma.
//!
//! One conversion at a time, on its own thread. The files live in a folder of
//! their own in the system's temporary directory, removed with the job.

use std::ffi::OsString;
use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// The largest film taken: two hours of 1080p at the apps' generous rates.
pub const MAX_INPUT: u64 = 16 << 30;

/// Asking ffmpeg what it encodes: a second, never this long.
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// How the picture is encoded. veryfast at CRF 18: measured on a 1080p30
/// slideshow (photos, Ken Burns zooms, dissolves) made by an app in VP8 at
/// 0.25 bit per pixel, it keeps 43.6 dB of PSNR against that WebM, as the
/// medium preset does, 1.6 times as fast (146 against 92 frames/s on four
/// cores).
const PRESET: &str = "veryfast";
const CRF: &str = "18";
const AUDIO_RATE: &str = "192k";

/// Why a conversion did not happen or did not finish, as the page is told
/// (it says it in its own words): `none` (no ffmpeg able to write H.264 and
/// AAC here), `busy` (another conversion under way), `too-large`,
/// `unreadable` (ffmpeg could not read the film: not a WebM it takes),
/// `cancelled`, `failed`.
pub type Refusal = &'static str;

/// One conversion under way, anywhere in the launcher.
static CONVERTING: AtomicBool = AtomicBool::new(false);

/// Holds CONVERTING for as long as it lives.
struct Turn;

impl Turn {
    fn take() -> Option<Turn> {
        CONVERTING.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).ok().map(|_| Turn)
    }
}

impl Drop for Turn {
    fn drop(&mut self) {
        CONVERTING.store(false, Ordering::SeqCst);
    }
}

/// The ffmpeg this launcher would run, if one writes H.264 and AAC: found on
/// the PATH, or where package managers put it (an application started from
/// the macOS Dock gets a PATH without Homebrew's folders). Asked once.
pub fn ffmpeg() -> Option<&'static Path> {
    static FOUND: OnceLock<Option<PathBuf>> = OnceLock::new();
    FOUND.get_or_init(|| candidates().into_iter().find(|p| p.is_file() && encodes_h264(p))).as_deref()
}

/// Whether this computer can convert a film at all (the page's `meta`).
pub fn available() -> bool {
    ffmpeg().is_some()
}

fn candidates() -> Vec<PathBuf> {
    let name = if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" };
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    if !cfg!(windows) {
        for extra in ["/usr/bin", "/usr/local/bin", "/opt/homebrew/bin", "/opt/local/bin", "/snap/bin"] {
            dirs.push(PathBuf::from(extra));
        }
    }
    let mut seen = Vec::new();
    for dir in dirs {
        let candidate = dir.join(name);
        if !seen.contains(&candidate) {
            seen.push(candidate);
        }
    }
    seen
}

/// `ffmpeg -encoders` names libx264 and aac.
fn encodes_h264(ffmpeg: &Path) -> bool {
    let Ok(child) = command(ffmpeg).args(["-hide_banner", "-encoders"]).stdout(Stdio::piped()).stderr(Stdio::null()).spawn() else {
        return false;
    };
    finish_with_output(child, PROBE_TIMEOUT).is_some_and(|listing| lists_encoders(&listing))
}

/// In an `ffmpeg -encoders` listing, the two encoders a conversion needs.
fn lists_encoders(listing: &str) -> bool {
    let has = |name: &str| listing.lines().any(|l| l.split_whitespace().nth(1) == Some(name));
    has("libx264") && has("aac")
}

/// Waits for a short-lived ffmpeg and hands back what it wrote, or None (it
/// failed, or ran past `timeout` and was stopped).
fn finish_with_output(mut child: Child, timeout: Duration) -> Option<String> {
    let out = child.stdout.take();
    // Read while ffmpeg runs, so that a full pipe never holds it up.
    let reader = std::thread::spawn(move || {
        let mut text = String::new();
        if let Some(mut out) = out {
            let _ = out.read_to_string(&mut text);
        }
        text
    });
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break,
            Ok(Some(_)) | Err(_) => return None,
            Ok(None) if start.elapsed() > timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
        }
    }
    reader.join().ok()
}

/// ffmpeg, set up the same way for every run: the C locale (numbers with a
/// point), no window on Windows, and on Linux none of the library paths an
/// AppImage sets for itself, which would make the system's ffmpeg load the
/// launcher's libraries.
fn command(ffmpeg: &Path) -> Command {
    let mut cmd = Command::new(ffmpeg);
    cmd.env("LC_ALL", "C").env("LANG", "C").stdin(Stdio::null());
    if std::env::var_os("APPIMAGE").is_some() {
        cmd.env_remove("LD_LIBRARY_PATH").env_remove("LD_PRELOAD");
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// The arguments of one conversion: `input` (the page's WebM) to `output`.
fn arguments(input: &Path, output: &Path) -> Vec<OsString> {
    let mut args: Vec<OsString> = [
        "-hide_banner", "-nostdin", "-nostats", "-loglevel", "error", "-y",
        "-progress", "pipe:1",
        // What ffmpeg may read: one local file, a Matroska/WebM, its picture
        // and its sound in the codecs a browser writes there. Nothing else.
        "-protocol_whitelist", "file",
        "-format_whitelist", "matroska,webm",
        "-codec_whitelist", "vp8,vp9,libvpx,libvpx-vp9,opus,libopus,vorbis,libvorbis",
        "-f", "matroska",
        "-i",
    ]
    .iter()
    .map(OsString::from)
    .collect();
    args.push(input.as_os_str().to_owned());
    for a in [
        "-map", "0:v:0", "-map", "0:a:0?", "-map_metadata", "0",
        "-c:v", "libx264", "-preset", PRESET, "-crf", CRF, "-profile:v", "high", "-pix_fmt", "yuv420p",
        "-c:a", "aac", "-b:a", AUDIO_RATE,
        "-movflags", "+faststart",
        "-f", "mp4",
    ] {
        args.push(OsString::from(a));
    }
    args.push(output.as_os_str().to_owned());
    args
}

/// From one line of ffmpeg's `-progress` report, how far the conversion is
/// (0 to 1) for a film of `duration` seconds; None for the other lines.
fn progress_of(line: &str, duration: f64) -> Option<f64> {
    let (key, value) = line.trim().split_once('=')?;
    match key {
        "out_time_us" | "out_time_ms" => {
            // Despite its name, out_time_ms is in microseconds too.
            let us: f64 = value.trim().parse().ok()?;
            (duration > 0.0).then(|| (us / 1e6 / duration).clamp(0.0, 1.0))
        }
        "progress" if value.trim() == "end" => Some(1.0),
        _ => None,
    }
}

/// Where a conversion stands, as the page reads it.
#[derive(Clone, Debug, PartialEq)]
pub enum State {
    /// The film is being received or converted; how far (0 to 1, or None
    /// while ffmpeg has not said).
    Running(Option<f64>),
    Done,
    Failed(Refusal),
}

/// One conversion: its state, its ffmpeg (to stop it), its folder.
pub struct Job {
    state: Arc<Mutex<State>>,
    child: Arc<Mutex<Option<Child>>>,
    cancelled: Arc<AtomicBool>,
    dir: PathBuf,
}

impl Job {
    /// Takes the launcher's one turn and a folder of its own, or says why not.
    pub fn new() -> Result<(Job, Starter), Refusal> {
        let Some(ffmpeg) = ffmpeg() else { return Err("none") };
        let turn = Turn::take().ok_or("busy")?;
        let dir = std::env::temp_dir().join(format!("kynoko-h264-{}", random_name()));
        fs::create_dir_all(&dir).map_err(|_| "failed")?;
        let job = Job {
            state: Arc::new(Mutex::new(State::Running(None))),
            child: Arc::new(Mutex::new(None)),
            cancelled: Arc::new(AtomicBool::new(false)),
            dir: dir.clone(),
        };
        let starter = Starter {
            ffmpeg: ffmpeg.to_path_buf(),
            dir,
            state: job.state.clone(),
            child: job.child.clone(),
            cancelled: job.cancelled.clone(),
            _turn: turn,
        };
        Ok((job, starter))
    }

    pub fn state(&self) -> State {
        self.state.lock().expect("job state").clone()
    }

    /// The MP4, once done.
    pub fn result(&self) -> Option<PathBuf> {
        (self.state() == State::Done).then(|| self.dir.join("film.mp4")).filter(|p| p.is_file())
    }

    /// Stops ffmpeg if it runs; the folder goes when the job is dropped.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        if let Some(child) = self.child.lock().expect("job child").as_mut() {
            let _ = child.kill();
        }
        let mut state = self.state.lock().expect("job state");
        if matches!(*state, State::Running(_)) {
            *state = State::Failed("cancelled");
        }
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        self.cancel();
        // ffmpeg may take a moment to let go of its files (Windows).
        let dir = self.dir.clone();
        std::thread::spawn(move || {
            for _ in 0..20 {
                if fs::remove_dir_all(&dir).is_ok() || !dir.exists() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(250));
            }
        });
    }
}

/// The half of a job that does the work, on the thread that received the
/// film; holds the launcher's turn until it is done.
pub struct Starter {
    ffmpeg: PathBuf,
    dir: PathBuf,
    state: Arc<Mutex<State>>,
    child: Arc<Mutex<Option<Child>>>,
    cancelled: Arc<AtomicBool>,
    _turn: Turn,
}

impl Starter {
    /// The film's bytes, received from `body`, to the job's folder; at most
    /// MAX_INPUT. Answers how many bytes.
    pub fn receive(&self, body: &mut dyn Read) -> Result<u64, Refusal> {
        let mut out = fs::File::create(self.dir.join("film.webm")).map_err(|_| "failed")?;
        let mut limited = body.take(MAX_INPUT + 1);
        let copied = std::io::copy(&mut limited, &mut out).map_err(|_| "failed")?;
        if copied > MAX_INPUT {
            return Err("too-large");
        }
        Ok(copied)
    }

    /// Converts the received film, `duration` seconds long (for the
    /// progress), and records how it ended. Blocks until ffmpeg is done.
    pub fn convert(self, duration: f64) {
        let ended = self.run(duration);
        let mut state = self.state.lock().expect("job state");
        *state = match ended {
            _ if self.cancelled.load(Ordering::SeqCst) => State::Failed("cancelled"),
            Ok(()) => State::Done,
            Err(why) => State::Failed(why),
        };
    }

    /// Records a refusal before anything ran (the film was not received).
    pub fn refuse(self, why: Refusal) {
        *self.state.lock().expect("job state") = State::Failed(why);
    }

    fn run(&self, duration: f64) -> Result<(), Refusal> {
        let input = self.dir.join("film.webm");
        let output = self.dir.join("film.mp4");
        let mut child = command(&self.ffmpeg)
            .args(arguments(&input, &output))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|_| "failed")?;
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        *self.child.lock().expect("job child") = Some(child);
        if self.cancelled.load(Ordering::SeqCst) {
            return Err("cancelled");
        }
        // stderr read on the side, so that a chatty ffmpeg never fills its
        // pipe and stops; only its last lines are kept, for the log.
        let errors = std::thread::spawn(move || {
            let mut tail: Vec<String> = Vec::new();
            if let Some(err) = stderr {
                for line in BufReader::new(err).lines().map_while(Result::ok) {
                    tail.push(line);
                    if tail.len() > 20 {
                        tail.remove(0);
                    }
                }
            }
            tail
        });
        if let Some(out) = stdout {
            for line in BufReader::new(out).lines().map_while(Result::ok) {
                if let Some(p) = progress_of(&line, duration) {
                    *self.state.lock().expect("job state") = State::Running(Some(p.min(0.999)));
                }
            }
        }
        let status = self.child.lock().expect("job child").as_mut().map(|c| c.wait());
        let tail = errors.join().unwrap_or_default();
        match status {
            Some(Ok(s)) if s.success() && output.is_file() => Ok(()),
            _ if self.cancelled.load(Ordering::SeqCst) => Err("cancelled"),
            _ => {
                eprintln!("kynoko-launcher: H.264 conversion failed: {}", tail.join(" | "));
                // What ffmpeg says when the input is not one it takes.
                let unreadable = tail.iter().any(|l| {
                    l.contains("Invalid data found") || l.contains("not on whitelist") || l.contains("Decoder") || l.contains("No such")
                });
                Err(if unreadable { "unreadable" } else { "failed" })
            }
        }
    }
}

fn random_name() -> String {
    let mut bytes = [0u8; 8];
    getrandom::getrandom(&mut bytes).expect("os randomness");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ffmpeg_reads_only_a_local_webm_in_a_browser_codec() {
        let args: Vec<String> =
            arguments(Path::new("in.webm"), Path::new("out.mp4")).iter().map(|a| a.to_string_lossy().into_owned()).collect();
        let after = |flag: &str| args.iter().position(|a| a == flag).map(|i| args[i + 1].clone());
        assert_eq!(after("-protocol_whitelist").as_deref(), Some("file"));
        assert_eq!(after("-format_whitelist").as_deref(), Some("matroska,webm"));
        let codecs = after("-codec_whitelist").unwrap();
        for allowed in ["vp8", "vp9", "opus", "vorbis"] {
            assert!(codecs.split(',').any(|c| c == allowed), "{codecs}");
        }
        assert!(!codecs.contains("h264") && !codecs.contains("hevc"), "{codecs}");
        // The input comes right after its whitelists, the output last.
        assert_eq!(after("-i").as_deref(), Some("in.webm"));
        assert_eq!(args.last().map(String::as_str), Some("out.mp4"));
        assert_eq!(after("-c:v").as_deref(), Some("libx264"));
        assert_eq!(after("-c:a").as_deref(), Some("aac"));
    }

    #[test]
    fn progress_is_read_from_ffmpeg_report() {
        assert_eq!(progress_of("out_time_us=5000000", 10.0), Some(0.5));
        assert_eq!(progress_of("out_time_ms=12000000", 10.0), Some(1.0));
        assert_eq!(progress_of("progress=end", 10.0), Some(1.0));
        assert_eq!(progress_of("progress=continue", 10.0), None);
        assert_eq!(progress_of("frame=42", 10.0), None);
        assert_eq!(progress_of("out_time_us=N/A", 10.0), None);
        assert_eq!(progress_of("out_time_us=5000000", 0.0), None);
    }

    #[test]
    fn the_encoders_are_found_in_a_listing() {
        let listing = " V....D libx264              libx264 H.264 / AVC\n A....D aac                  AAC (Advanced Audio Coding)\n";
        assert!(lists_encoders(listing));
        assert!(!lists_encoders(" V....D libx264rgb  libx264 RGB\n A....D aac  AAC\n"));
        assert!(!lists_encoders(" V....D libx264  libx264\n"));
    }
}
