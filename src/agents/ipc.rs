//! The local endpoint `tether hook` reports to: a named pipe on Windows, a unix socket
//! elsewhere. Every local terminal gets its path as `TETHER_IPC`. Messages are queued in
//! an inbox the UI drains once per frame; each one wakes the UI.

use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use eframe::egui;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, BufReader};

use super::hook::{IpcMessage, valid_session};

/// A client may send at most this much, within [`CLIENT_TIMEOUT`].
const MAX_CLIENT_BYTES: u64 = 64 * 1024;
const CLIENT_TIMEOUT: Duration = Duration::from_secs(2);
/// Messages kept while the UI is not draining (it drains every frame).
const MAX_QUEUED: usize = 1024;

static ENDPOINT: OnceLock<String> = OnceLock::new();

/// The path exported as `TETHER_IPC`, once the server runs.
pub fn endpoint() -> Option<&'static str> {
    ENDPOINT.get().map(String::as_str)
}

/// Environment for a new local terminal: `TETHER_IPC` and `TETHER_EXE` (empty until
/// the server runs). `TETHER_SESSION` is set by the pty itself.
pub fn session_env() -> Vec<(&'static str, String)> {
    let Some(ep) = endpoint() else { return Vec::new() };
    let mut env = vec![("TETHER_IPC", ep.to_string())];
    if let Ok(exe) = std::env::current_exe() {
        env.push(("TETHER_EXE", exe.to_string_lossy().into_owned()));
    }
    env
}

#[derive(Clone, Default)]
pub struct Inbox(Arc<Mutex<Vec<IpcMessage>>>);

impl Inbox {
    pub fn drain(&self) -> Vec<IpcMessage> {
        self.0.lock().map(|mut q| std::mem::take(&mut *q)).unwrap_or_default()
    }

    fn push(&self, msg: IpcMessage) {
        if let Ok(mut q) = self.0.lock()
            && q.len() < MAX_QUEUED
        {
            q.push(msg);
        }
    }
}

/// Starts the server on `rt` (once per process; later calls return a fresh, unused inbox).
pub fn start(rt: &tokio::runtime::Handle, ctx: &egui::Context) -> Inbox {
    let inbox = Inbox::default();
    if ENDPOINT.get().is_some() {
        return inbox;
    }
    let name = endpoint_name();
    let (inbox2, ctx) = (inbox.clone(), ctx.clone());
    let _guard = rt.enter();
    let started = platform::listen(&name, inbox2, ctx);
    if started.is_ok() {
        let _ = ENDPOINT.set(name);
    }
    inbox
}

fn endpoint_name() -> String {
    let id = uuid::Uuid::new_v4().simple().to_string();
    if cfg!(windows) {
        format!(r"\\.\pipe\tether-{}-{}", std::process::id(), &id[..12])
    } else {
        std::env::temp_dir().join(format!("tether-{}-{}.sock", std::process::id(), &id[..12])).to_string_lossy().into_owned()
    }
}

/// Reads one client's lines into the inbox.
async fn serve_client(stream: impl AsyncRead + Unpin, inbox: Inbox, ctx: egui::Context) {
    let mut lines = BufReader::new(stream.take(MAX_CLIENT_BYTES)).lines();
    let read = async {
        while let Ok(Some(line)) = lines.next_line().await {
            if let Some(msg) = parse_line(&line) {
                inbox.push(msg);
                ctx.request_repaint();
            }
        }
    };
    let _ = tokio::time::timeout(CLIENT_TIMEOUT, read).await;
}

/// A valid message from one line, or `None`.
pub fn parse_line(line: &str) -> Option<IpcMessage> {
    let msg: IpcMessage = serde_json::from_str(line.trim()).ok()?;
    (msg.v == 1 && valid_session(&msg.session)).then_some(msg)
}

#[cfg(windows)]
mod platform {
    use tokio::net::windows::named_pipe::ServerOptions;

    use super::*;

    pub fn listen(name: &str, inbox: Inbox, ctx: egui::Context) -> std::io::Result<()> {
        // Remote clients are rejected by default; only this user's processes can write.
        let mut server = ServerOptions::new().first_pipe_instance(true).create(name)?;
        let name = name.to_string();
        tokio::spawn(async move {
            loop {
                if server.connect().await.is_ok() {
                    let next = match ServerOptions::new().create(&name) {
                        Ok(s) => s,
                        Err(_) => return,
                    };
                    let client = std::mem::replace(&mut server, next);
                    tokio::spawn(serve_client(client, inbox.clone(), ctx.clone()));
                } else {
                    server = match ServerOptions::new().create(&name) {
                        Ok(s) => s,
                        Err(_) => return,
                    };
                }
            }
        });
        Ok(())
    }
}

#[cfg(unix)]
mod platform {
    use std::os::unix::fs::PermissionsExt;

    use tokio::net::UnixListener;

    use super::*;

    pub fn listen(name: &str, inbox: Inbox, ctx: egui::Context) -> std::io::Result<()> {
        let _ = std::fs::remove_file(name);
        let listener = UnixListener::bind(name)?;
        std::fs::set_permissions(name, std::fs::Permissions::from_mode(0o600))?;
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                tokio::spawn(serve_client(stream, inbox.clone(), ctx.clone()));
            }
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_are_validated() {
        assert!(parse_line(r#"{"v":1,"session":"ab12","agent":"claude","event":"Stop"}"#).is_some());
        assert!(parse_line(r#"{"v":2,"session":"ab12","agent":"claude","event":"Stop"}"#).is_none());
        assert!(parse_line(r#"{"v":1,"session":"a/b","agent":"claude","event":"Stop"}"#).is_none());
        assert!(parse_line("not json").is_none());
    }

    /// The real round trip: `hook::send` from a thread into the running server.
    #[test]
    fn hook_messages_reach_the_inbox() {
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(1).enable_all().build().unwrap();
        let ctx = egui::Context::default();
        let inbox = start(rt.handle(), &ctx);
        let Some(ep) = endpoint() else { panic!("server did not start") };
        let msg = super::super::hook::build_message("claude", Some("Stop"), &serde_json::Value::Null, "cafe01").unwrap();
        let line = format!("{}\n", serde_json::to_string(&msg).unwrap());
        super::super::hook::send_for_test(ep, line.as_bytes()).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        let mut got = Vec::new();
        while got.is_empty() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
            got = inbox.drain();
        }
        assert_eq!(got, vec![msg]);
    }
}
