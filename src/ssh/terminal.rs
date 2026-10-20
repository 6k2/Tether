//! Interactive shell channel feeding a vt100 screen model.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use russh::ChannelMsg;
use tokio::sync::mpsc;

use super::connection::{Connection, Stage};
use crate::repaint::RepaintThrottle;

pub const TERM_NAME: &str = "xterm-256color";
/// Fallback scrollback; the live value comes from `settings.terminal.scrollback`.
pub const SCROLLBACK_LINES: usize = 10_000;

/// Scrollback length for a new terminal, from settings.
pub fn scrollback_lines() -> usize {
    crate::settings::get().terminal.scrollback()
}

#[derive(Debug, Clone, PartialEq)]
pub enum LinkState {
    Connecting,
    Connected,
    Closed(String),
    Failed(String),
}

impl LinkState {
    pub fn is_live(&self) -> bool {
        matches!(self, LinkState::Connected)
    }
}

/// Captures OSC title changes and bells from the remote.
#[derive(Default)]
pub struct Events {
    pub title: Option<String>,
    pub bell_count: u64,
    /// Working directory reported by the shell via OSC 7 (`file://host/path`), if it does.
    pub cwd: Option<String>,
    /// Desktop notifications the program asked for (OSC 9 / OSC 777 / OSC 99): how many,
    /// and the latest text. `agents::hub` turns them into session states.
    pub notice_count: u64,
    pub notice: Option<String>,
}

impl vt100::Callbacks for Events {
    fn audible_bell(&mut self, _: &mut vt100::Screen) {
        self.bell_count += 1;
    }

    fn set_window_title(&mut self, _: &mut vt100::Screen, title: &[u8]) {
        self.title = Some(String::from_utf8_lossy(title).into_owned());
    }

    fn unhandled_osc(&mut self, _: &mut vt100::Screen, params: &[&[u8]]) {
        match params {
            [b"7", uri, ..] => self.cwd = osc7_path(uri),
            // A title containing ';' is split into more params than vt100 matches.
            [b"0" | b"2", rest @ ..] if !rest.is_empty() => self.title = Some(join_params(rest)),
            _ => {
                if let Some(text) = osc_notification(params) {
                    self.notice_count += 1;
                    self.notice = Some(text);
                }
            }
        }
    }
}

fn join_params(parts: &[&[u8]]) -> String {
    String::from_utf8_lossy(&parts.join(&b';')).into_owned()
}

/// The text of a desktop-notification OSC: `9;<msg>` (iTerm2 / Windows Terminal; `9;4;..`
/// is ConEmu's progress bar, not a message), `777;notify;<title>;<body>` (urxvt / Ghostty)
/// and `99;<meta>;<payload>` (kitty). vt100's parser (vte) reassembles sequences split
/// across reads, so this sees whole sequences only.
pub fn osc_notification(params: &[&[u8]]) -> Option<String> {
    let text = match params {
        [b"9", first, rest @ ..] => {
            if !first.is_empty() && first.iter().all(u8::is_ascii_digit) {
                return None;
            }
            let mut all = vec![*first];
            all.extend_from_slice(rest);
            join_params(&all)
        }
        [b"777", b"notify", title, body @ ..] => {
            let (title, body) = (String::from_utf8_lossy(title), join_params(body));
            match (title.trim().is_empty(), body.trim().is_empty()) {
                (_, true) => title.into_owned(),
                (true, false) => body,
                (false, false) => format!("{title}: {body}"),
            }
        }
        [b"99", _meta, payload @ ..] => join_params(payload),
        _ => return None,
    };
    let text = text.trim().to_string();
    (!text.is_empty()).then_some(text)
}

/// `file://host/some%20dir` -> `/some dir`.
pub fn osc7_path(uri: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(uri).ok()?;
    let rest = text.strip_prefix("file://")?;
    let path = &rest[rest.find('/')?..];
    let bytes = path.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = (bytes[i] == b'%' && i + 2 < bytes.len())
            .then(|| std::str::from_utf8(&bytes[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()))
            .flatten();
        if let Some(b) = hex {
            out.push(b);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).ok()
}

pub struct TermShared {
    pub parser: vt100::Parser<Events>,
    pub state: LinkState,
    /// Bumped on every parser change (each output chunk). Views cache painted rows keyed
    /// on it, so anything that feeds `parser` must bump it too.
    pub generation: u64,
    /// Connect progress shown while `state` is `Connecting`.
    pub stage: Stage,
    /// When the shell channel was opened (matches the remote shell's start time).
    pub opened_at: Option<std::time::Instant>,
    /// OS process id of a local shell (`pty::local`); `None` for SSH sessions.
    pub pid: Option<u32>,
    /// When the last output chunk arrived (local shells), for activity detection.
    pub last_output: Option<std::time::Instant>,
}

pub enum TermCmd {
    Input(Vec<u8>),
    Resize { rows: u16, cols: u16 },
    Close,
}

#[derive(Clone)]
pub struct TerminalHandle {
    pub shared: Arc<Mutex<TermShared>>,
    /// Random id exported to the remote shell as `LC_TETHER_ID` (local shells get it as
    /// `TETHER_SESSION`) so we can find its process (and working directory) later.
    /// Hex only: safe to embed in commands.
    pub tag: String,
    /// Coalesces UI wake-ups from the output reader (see [`RepaintThrottle`]).
    pub repaint: Arc<RepaintThrottle>,
    tx: mpsc::UnboundedSender<TermCmd>,
}

impl TerminalHandle {
    pub fn new(rows: u16, cols: u16) -> (Self, mpsc::UnboundedReceiver<TermCmd>) {
        let shared = TermShared {
            parser: vt100::Parser::new_with_callbacks(rows, cols, scrollback_lines(), Events::default()),
            state: LinkState::Connecting,
            generation: 0,
            stage: Stage::Connecting,
            opened_at: None,
            pid: None,
            last_output: None,
        };
        let (tx, rx) = mpsc::unbounded_channel();
        let tag = uuid::Uuid::new_v4().simple().to_string();
        (Self { shared: Arc::new(Mutex::new(shared)), tag, repaint: Arc::new(RepaintThrottle::new()), tx }, rx)
    }

    pub fn send(&self, bytes: impl Into<Vec<u8>>) {
        let _ = self.tx.send(TermCmd::Input(bytes.into()));
    }

    pub fn resize(&self, rows: u16, cols: u16) {
        let _ = self.tx.send(TermCmd::Resize { rows, cols });
    }

    pub fn close(&self) {
        let _ = self.tx.send(TermCmd::Close);
    }

    pub fn set_state(&self, state: LinkState) {
        if let Ok(mut s) = self.shared.lock() {
            s.state = state;
        }
    }

    pub fn set_stage(&self, stage: Stage) {
        if let Ok(mut s) = self.shared.lock() {
            s.stage = stage;
        }
    }

    /// Client-side facts used to find this shell on the server (see `ssh::cwd`).
    pub fn probe_facts(&self) -> (f64, u16, u16) {
        self.shared
            .lock()
            .map(|s| {
                let (rows, cols) = s.parser.screen().size();
                (s.opened_at.map_or(0.0, |t| t.elapsed().as_secs_f64()), rows, cols)
            })
            .unwrap_or((0.0, 0, 0))
    }
}

/// Drives the shell channel until it closes. `repaint` wakes the UI on state changes;
/// output wakes it through the handle's [`RepaintThrottle`] (coalesced, visibility-aware).
pub async fn run_shell(
    conn: Arc<Connection>,
    term: TerminalHandle,
    mut rx: mpsc::UnboundedReceiver<TermCmd>,
    repaint: impl Fn() + Send + 'static,
) -> Result<()> {
    let (rows, cols) = term.shared.lock().map(|s| s.parser.screen().size()).unwrap_or((24, 80));
    term.set_stage(Stage::Opening);
    repaint();
    let channel = conn.handle.channel_open_session().await?;
    channel.request_pty(false, TERM_NAME, cols.into(), rows.into(), 0, 0, &[]).await?;
    // Most servers accept LC_* (Debian/Ubuntu `AcceptEnv LANG LC_*`); harmless if refused.
    let _ = channel.set_env(false, "LC_TETHER_ID", term.tag.clone()).await;
    channel.request_shell(false).await?;
    if let Ok(mut s) = term.shared.lock() {
        s.opened_at = Some(std::time::Instant::now());
        s.state = LinkState::Connected;
    }
    repaint();

    let (mut read, write) = channel.split();
    // Writes run on their own task so a large paste waiting on the remote window can't
    // stop us from draining output (which is what would free that window).
    let closed_by_user = Arc::new(AtomicBool::new(false));
    let writer = {
        let closed_by_user = closed_by_user.clone();
        tokio::spawn(async move {
            while let Some(cmd) = rx.recv().await {
                let sent = match cmd {
                    TermCmd::Input(bytes) => write.data_bytes(bytes).await,
                    TermCmd::Resize { rows, cols } => write.window_change(cols.into(), rows.into(), 0, 0).await,
                    TermCmd::Close => {
                        closed_by_user.store(true, Ordering::Relaxed);
                        let _ = write.close().await;
                        return;
                    }
                };
                if sent.is_err() {
                    return;
                }
            }
        })
    };

    // Keep reading after ExitStatus/Eof: trailing output may still arrive before Close.
    let mut exit_code = None;
    while let Some(msg) = read.wait().await {
        match msg {
            ChannelMsg::Data { data } | ChannelMsg::ExtendedData { data, .. } => {
                if let Ok(mut s) = term.shared.lock() {
                    s.parser.process(&data);
                    s.generation = s.generation.wrapping_add(1);
                }
                term.repaint.notify();
            }
            ChannelMsg::ExitStatus { exit_status } => exit_code = Some(exit_status),
            ChannelMsg::Close => break,
            _ => {}
        }
    }
    writer.abort();
    let reason = match exit_code {
        _ if closed_by_user.load(Ordering::Relaxed) => "Closed".to_string(),
        Some(code) => format!("Process exited with code {code}"),
        None => "Connection closed".to_string(),
    };
    term.set_state(LinkState::Closed(reason));
    repaint();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parser() -> vt100::Parser<Events> {
        vt100::Parser::new_with_callbacks(24, 80, 0, Events::default())
    }

    #[test]
    fn notifications_are_extracted_even_when_split_across_chunks() {
        let stream: &[u8] = b"hi\x1b]777;notify;Claude Code;Needs your permission; to use Bash\x07\x1b]9;Task done\x1b\\\x1b]9;4;3;50\x07";
        // Every split point, including inside the ESC ] introducer and the ST terminator.
        for cut in 0..=stream.len() {
            let mut p = parser();
            p.process(&stream[..cut]);
            p.process(&stream[cut..]);
            assert_eq!(p.callbacks().notice_count, 2, "cut at {cut}");
            assert_eq!(p.callbacks().notice.as_deref(), Some("Task done"), "cut at {cut}");
        }
        // Byte by byte.
        let mut p = parser();
        for b in stream {
            p.process(std::slice::from_ref(b));
        }
        assert_eq!(p.callbacks().notice_count, 2);
        let mut p = parser();
        p.process(&stream[..stream.len() - 26]);
        assert_eq!(p.callbacks().notice.as_deref(), Some("Claude Code: Needs your permission; to use Bash"));
    }

    #[test]
    fn osc_notification_forms() {
        assert_eq!(osc_notification(&[b"9", b"hello"]).as_deref(), Some("hello"));
        assert_eq!(osc_notification(&[b"9", b"4", b"1", b"20"]), None, "ConEmu progress");
        assert_eq!(osc_notification(&[b"777", b"notify", b"", b"body"]).as_deref(), Some("body"));
        assert_eq!(osc_notification(&[b"777", b"notify", b"Title"]).as_deref(), Some("Title"));
        assert_eq!(osc_notification(&[b"99", b"i=1:d=0", b"Build finished"]).as_deref(), Some("Build finished"));
        assert_eq!(osc_notification(&[b"8", b"", b"http://x"]), None);
        assert_eq!(osc_notification(&[b"9", b"  "]), None);
    }

    #[test]
    fn titles_with_semicolons_and_cwd() {
        let mut p = parser();
        p.process("\x1b]0;\u{2810} fix; then test\x07".as_bytes());
        assert_eq!(p.callbacks().title.as_deref(), Some("\u{2810} fix; then test"));
        p.process(b"\x1b]2;plain\x07\x1b]7;file://host/home/me%20x\x07");
        assert_eq!(p.callbacks().title.as_deref(), Some("plain"));
        assert_eq!(p.callbacks().cwd.as_deref(), Some("/home/me x"));
        assert_eq!(p.callbacks().notice_count, 0);
    }
}
