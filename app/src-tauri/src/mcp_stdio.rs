//! `kynoko-launcher mcp`: what an assistant starts (Claude Desktop, Codex,
//! Qwen Code, LM Studio...), its MCP server over standard input and output.
//!
//! It is a pipe, nothing more: each line the assistant writes goes, as it
//! is, to the launcher's agent over the loopback (automation.rs), and each
//! answer comes back as a line. The protocol is the agent's (mcp.rs), so
//! that several assistants share one set of app windows, and the agent goes
//! on running between them. No agent yet: this starts one (`agent`), and
//! waits for it to say where it listens (`automation.json`, in the state
//! folder, rewritten at each start).
//!
//! It runs before anything else in `run()`, without Tauri: no window, no
//! single-instance hand-over, nothing registered. What goes wrong is said on
//! standard error, which assistants keep in their logs.

use std::io::{BufRead, ErrorKind, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tungstenite::{Message, WebSocket};

pub const STATE_FILE: &str = "automation.json";
/// How long an agent started here has to come up.
const AGENT_START: Duration = Duration::from_secs(25);
const POLL: Duration = Duration::from_millis(40);

/// Where the agent listens for assistants, for this run.
#[derive(Serialize, Deserialize, Default, Clone, PartialEq, Debug)]
pub struct State {
    pub port: u16,
    pub token: String,
}

/// Written by the agent as its relay starts: readable by this user only.
pub fn write_state(state: &State) -> std::io::Result<()> {
    crate::settings::write_named(STATE_FILE, state)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(state_path(), std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

fn state_path() -> PathBuf {
    crate::settings::dir().join(STATE_FILE)
}

fn read_state() -> Option<State> {
    let state: State = crate::settings::read_named(STATE_FILE);
    (state.port != 0 && state.token.len() == 64).then_some(state)
}

pub fn run() -> i32 {
    let ws = match connect_now() {
        Some(ws) => ws,
        None => {
            if let Err(e) = start_agent() {
                eprintln!("kynoko-launcher mcp: cannot start Kynoko Launcher: {e}");
                return 1;
            }
            let deadline = Instant::now() + AGENT_START;
            loop {
                if let Some(ws) = connect_now() {
                    break ws;
                }
                if Instant::now() > deadline {
                    eprintln!("kynoko-launcher mcp: Kynoko Launcher did not start (state in {})", state_path().display());
                    return 1;
                }
                thread::sleep(Duration::from_millis(250));
            }
        }
    };
    pipe(ws)
}

fn connect_now() -> Option<WebSocket<TcpStream>> {
    let state = read_state()?;
    let stream = TcpStream::connect_timeout(&([127, 0, 0, 1], state.port).into(), Duration::from_secs(2)).ok()?;
    let url = format!("ws://127.0.0.1:{}/c/{}", state.port, state.token);
    tungstenite::client(url.as_str(), stream).ok().map(|(ws, _)| ws)
}

/// Starts the agent apart from this process: it outlives the assistant's
/// session, as the launcher does. A running launcher gets the request
/// through its single-instance hand-over and starts its relay.
fn start_agent() -> std::io::Result<()> {
    let exe = std::env::current_exe()?;
    let mut command = std::process::Command::new(exe);
    command.arg("agent").stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
        // Out of the assistant's job when it allows it, so that closing the
        // assistant does not take the launcher with it.
        command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_BREAKAWAY_FROM_JOB);
        if command.spawn().is_ok() {
            return Ok(());
        }
        command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    command.spawn().map(|_| ())
}

/// Lines in, messages out, and back, until either side ends.
fn pipe(mut ws: WebSocket<TcpStream>) -> i32 {
    let (tx, rx) = mpsc::channel::<Option<String>>();
    thread::spawn(move || {
        let stdin = std::io::stdin();
        for line in stdin.lock().lines() {
            match line {
                Ok(l) if l.trim().is_empty() => {}
                Ok(l) => {
                    if tx.send(Some(l)).is_err() {
                        return;
                    }
                }
                Err(_) => break,
            }
        }
        let _ = tx.send(None);
    });
    let _ = ws.get_mut().set_read_timeout(Some(POLL));
    let stdout = std::io::stdout();
    loop {
        match ws.read() {
            Ok(Message::Text(text)) => {
                let mut out = stdout.lock();
                if writeln!(out, "{}", text.as_str()).and_then(|_| out.flush()).is_err() {
                    return 0;
                }
            }
            Ok(Message::Close(_)) => {
                eprintln!("kynoko-launcher mcp: Kynoko Launcher closed the connection");
                return 1;
            }
            Ok(_) => {}
            Err(tungstenite::Error::Io(e)) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(e) => {
                eprintln!("kynoko-launcher mcp: connection to Kynoko Launcher lost: {e}");
                return 1;
            }
        }
        loop {
            match rx.try_recv() {
                Ok(Some(line)) => {
                    if ws.send(Message::text(line)).is_err() {
                        return 1;
                    }
                }
                Ok(None) | Err(mpsc::TryRecvError::Disconnected) => {
                    let _ = ws.close(None);
                    let _ = ws.flush();
                    return 0;
                }
                Err(mpsc::TryRecvError::Empty) => break,
            }
        }
    }
}
