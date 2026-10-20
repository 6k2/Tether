//! A local shell on a pseudo terminal (ConPTY on Windows, a unix pty elsewhere)
//! feeding the same [`TerminalHandle`] an SSH shell does, so `TerminalView` can't tell
//! them apart.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
use tokio::sync::mpsc;

use super::shells::ShellSpec;
use crate::ssh::terminal::{LinkState, TERM_NAME, TermCmd, TerminalHandle};

const READ_BUF: usize = 64 * 1024;
/// How long to wait for trailing output once the shell has exited.
const DRAIN_TIMEOUT: Duration = Duration::from_millis(500);

type SharedMaster = Arc<Mutex<Option<Box<dyn MasterPty + Send>>>>;

/// Runs `spec` until it exits, then marks the terminal closed with its exit code.
/// Blocking: call it from its own thread. `repaint(delay)` asks the UI for a frame
/// after `delay` (egui's `request_repaint_after`), which lets bursts coalesce.
/// Returns an error only when the shell could not be started.
pub fn run_local_pty(
    spec: &ShellSpec,
    cwd: Option<PathBuf>,
    term: TerminalHandle,
    rx: mpsc::UnboundedReceiver<TermCmd>,
    repaint: impl Fn(Duration) + Send + Sync + 'static,
) -> Result<()> {
    let repaint = Arc::new(repaint);
    let (rows, cols) = term.shared.lock().map(|s| s.parser.screen().size()).unwrap_or((24, 80));
    let pair = native_pty_system().openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 }).context("opening a pseudo terminal")?;
    let mut cmd = CommandBuilder::new(&spec.program);
    cmd.args(&spec.args);
    cmd.env("TERM", TERM_NAME);
    cmd.env("COLORTERM", "truecolor");
    cmd.env("TETHER_SESSION", &term.tag);
    cmd.env("TERM_PROGRAM", "tether");
    // Where `tether hook` reports agent status (see `agents::ipc`).
    for (key, value) in crate::agents::ipc::session_env() {
        cmd.env(key, value);
    }
    if let Some(dir) = cwd.filter(|d| d.is_dir()).or_else(home_dir) {
        cmd.cwd(dir);
    }
    let mut child = pair.slave.spawn_command(cmd).with_context(|| format!("starting {}", spec.program.display()))?;
    // Our copy of the slave end would keep the pty open after the shell exits.
    drop(pair.slave);
    let reader = pair.master.try_clone_reader()?;
    let writer = pair.master.take_writer()?;
    let master: SharedMaster = Arc::new(Mutex::new(Some(pair.master)));
    if let Ok(mut s) = term.shared.lock() {
        s.pid = child.process_id();
        s.opened_at = Some(Instant::now());
        s.state = LinkState::Connected;
    }
    repaint(Duration::ZERO);

    let (drained_tx, drained_rx) = std::sync::mpsc::channel::<()>();
    spawn_named("tether-pty-read", {
        let term = term.clone();
        move || {
            pump_output(reader, &term);
            let _ = drained_tx.send(());
        }
    })?;
    let closed_by_user = Arc::new(AtomicBool::new(false));
    spawn_named("tether-pty-write", {
        let (master, killer, closed_by_user) = (master.clone(), child.clone_killer(), closed_by_user.clone());
        move || handle_commands(rx, writer, &master, killer, &closed_by_user)
    })?;

    let status = child.wait();
    // Closing the pty is what ends the reader on Windows (ConPTY keeps its pipe open).
    drop(master.lock().ok().and_then(|mut m| m.take()));
    let _ = drained_rx.recv_timeout(DRAIN_TIMEOUT);
    let reason = match status {
        _ if closed_by_user.load(Ordering::Relaxed) => "Closed".to_string(),
        Ok(s) => format!("Process exited with code {}", s.exit_code()),
        Err(e) => format!("Process lost: {e}"),
    };
    term.set_state(LinkState::Closed(reason));
    repaint(Duration::ZERO);
    Ok(())
}

/// Feeds pty output into the screen model until EOF.
fn pump_output(mut reader: Box<dyn Read + Send>, term: &TerminalHandle) {
    let mut buf = vec![0u8; READ_BUF];
    loop {
        let n = match reader.read(&mut buf) {
            Ok(0) | Err(_) => return,
            Ok(n) => n,
        };
        let chunk = &buf[..n];
        let mut cursor_report = None;
        if let Ok(mut s) = term.shared.lock() {
            s.parser.process(chunk);
            s.generation = s.generation.wrapping_add(1);
            s.last_output = Some(Instant::now());
            if contains(chunk, CURSOR_QUERY) {
                let (row, col) = s.parser.screen().cursor_position();
                cursor_report = Some(format!("\x1b[{};{}R", row + 1, col + 1));
            }
        }
        // vt100 only models the screen; ConPTY blocks its first output until this query
        // (and TUI programs everywhere wait on it) is answered.
        if let Some(report) = cursor_report {
            term.send(report.into_bytes());
        }
        term.repaint.notify();
    }
}

fn handle_commands(
    mut rx: mpsc::UnboundedReceiver<TermCmd>,
    mut writer: Box<dyn Write + Send>,
    master: &SharedMaster,
    mut killer: Box<dyn portable_pty::ChildKiller + Send + Sync>,
    closed_by_user: &AtomicBool,
) {
    while let Some(cmd) = rx.blocking_recv() {
        match cmd {
            TermCmd::Input(bytes) => {
                if writer.write_all(&bytes).and_then(|()| writer.flush()).is_err() {
                    return;
                }
            }
            TermCmd::Resize { rows, cols } => {
                if let Ok(guard) = master.lock()
                    && let Some(m) = guard.as_ref()
                {
                    let _ = m.resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 });
                }
            }
            TermCmd::Close => {
                closed_by_user.store(true, Ordering::Relaxed);
                break;
            }
        }
    }
    // Closed, or every handle dropped: nobody can see this shell any more.
    let _ = killer.kill();
}

/// Device status report "where is the cursor?" (DSR 6).
const CURSOR_QUERY: &[u8] = b"\x1b[6n";

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

fn spawn_named(name: &str, f: impl FnOnce() + Send + 'static) -> Result<()> {
    std::thread::Builder::new().name(name.into()).spawn(f).map(drop).context("spawning a pty thread")
}

fn home_dir() -> Option<PathBuf> {
    directories::BaseDirs::new().map(|d| d.home_dir().to_path_buf())
}
