//! `tether hook <agent> [event]`: the command agents' hooks run. It turns the hook's
//! JSON (stdin for Claude Code, the last argument for Codex' `notify`) into one compact
//! [`IpcMessage`] line and hands it to the tether window that owns the terminal.
//!
//! It must never get in the agent's way: it does nothing unless `TETHER_SESSION` and
//! `TETHER_IPC` are set (so sessions outside tether are unaffected), prints nothing,
//! gives up after [`DEADLINE`] and always exits 0.

use std::io::Read;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::snippet;

pub const DEADLINE: Duration = Duration::from_millis(300);
/// Hook payloads beyond this are cut (a prompt can be long; we only keep a snippet).
const MAX_STDIN: u64 = 1 << 20;
const SNIPPET_CHARS: usize = 200;

/// One status update, as sent over the pipe (one JSON object per line).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IpcMessage {
    pub v: u8,
    /// The terminal's `TETHER_SESSION` tag.
    pub session: String,
    /// `claude`, `codex`, ...
    pub agent: String,
    /// Hook event name (`Stop`, `Notification`, `agent-turn-complete`, ...).
    pub event: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notification_type: Option<String>,
    /// The notification text, prompt or last reply, cut to a snippet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// The agent's own session id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_session: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
}

/// A session tag is our own hex uuid: anything else is not from a tether terminal.
pub fn valid_session(tag: &str) -> bool {
    !tag.is_empty() && tag.len() <= 64 && tag.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// Whether `TETHER_IPC` names one of our endpoints (a `tether-` named pipe on Windows,
/// a unix socket elsewhere): the hook opens it for writing, so it must never be a path
/// to a regular file someone planted in the environment.
pub fn valid_endpoint(endpoint: &str) -> bool {
    #[cfg(windows)]
    {
        endpoint
            .strip_prefix(r"\\.\pipe\tether-")
            .is_some_and(|rest| !rest.is_empty() && rest.len() <= 128 && rest.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'))
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;
        std::fs::symlink_metadata(endpoint).is_ok_and(|m| m.file_type().is_socket())
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = endpoint;
        false
    }
}

fn text(v: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|k| v.get(*k).and_then(Value::as_str)).map(str::to_string).filter(|s| !s.trim().is_empty())
}

/// Builds the message for `agent`'s hook payload. `event` (the CLI argument) wins over
/// the event named in the payload. `None` when no event can be told.
pub fn build_message(agent: &str, event: Option<&str>, payload: &Value, session: &str) -> Option<IpcMessage> {
    let event = event.map(str::to_string).or_else(|| text(payload, &["hook_event_name", "type"]))?;
    let mut message = text(payload, &["message", "prompt", "last-assistant-message", "last_assistant_message"]);
    if message.is_none()
        && let Some(inputs) = payload.get("input-messages").and_then(Value::as_array)
    {
        message = inputs.last().and_then(Value::as_str).map(str::to_string);
    }
    Some(IpcMessage {
        v: 1,
        session: session.to_string(),
        agent: agent.to_ascii_lowercase(),
        event,
        notification_type: text(payload, &["notification_type"]),
        message: message.map(|m| snippet(&m, SNIPPET_CHARS)),
        cwd: text(payload, &["cwd"]),
        agent_session: text(payload, &["session_id", "thread-id", "turn-id"]),
        tool: text(payload, &["tool_name"]),
    })
}

/// Entry point of `tether hook ...` (`args` are the words after `hook`).
pub fn run(args: &[String]) {
    let (Ok(session), Ok(endpoint)) = (std::env::var("TETHER_SESSION"), std::env::var("TETHER_IPC")) else { return };
    if !valid_session(&session) || !valid_endpoint(&endpoint) {
        return;
    }
    let args = args.to_vec();
    let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
    let started = std::thread::Builder::new().name("tether-hook".into()).spawn(move || {
        let deadline = Instant::now() + DEADLINE;
        let agent = args.first().cloned().unwrap_or_else(|| "claude".into());
        // Codex passes the JSON as the last argument; Claude Code writes it to stdin.
        let inline = args.last().filter(|a| a.trim_start().starts_with('{')).cloned();
        let event = args.get(1).filter(|a| !a.trim_start().starts_with('{')).cloned();
        let raw = inline.unwrap_or_else(|| {
            let mut s = String::new();
            let _ = std::io::stdin().take(MAX_STDIN).read_to_string(&mut s);
            s
        });
        let payload: Value = serde_json::from_str(&raw).unwrap_or(Value::Null);
        if let Some(msg) = build_message(&agent, event.as_deref(), &payload, &session)
            && let Ok(mut line) = serde_json::to_string(&msg)
        {
            line.push('\n');
            let _ = send(&endpoint, line.as_bytes(), deadline);
        }
        let _ = done_tx.send(());
    });
    if started.is_ok() {
        let _ = done_rx.recv_timeout(DEADLINE);
    }
}

#[cfg(test)]
pub(crate) fn send_for_test(endpoint: &str, line: &[u8]) -> std::io::Result<()> {
    send(endpoint, line, Instant::now() + DEADLINE)
}

#[cfg(windows)]
fn send(endpoint: &str, line: &[u8], deadline: Instant) -> std::io::Result<()> {
    use std::io::Write;
    const ERROR_PIPE_BUSY: i32 = 231;
    loop {
        match std::fs::OpenOptions::new().write(true).open(endpoint) {
            Ok(mut pipe) => return pipe.write_all(line),
            Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY) && Instant::now() < deadline => std::thread::sleep(Duration::from_millis(5)),
            Err(e) => return Err(e),
        }
    }
}

#[cfg(unix)]
fn send(endpoint: &str, line: &[u8], deadline: Instant) -> std::io::Result<()> {
    use std::io::Write;
    let mut sock = std::os::unix::net::UnixStream::connect(endpoint)?;
    sock.set_write_timeout(Some(deadline.saturating_duration_since(Instant::now()).max(Duration::from_millis(1))))?;
    sock.write_all(line)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn claude_notification_becomes_a_message() {
        let payload = json!({
            "session_id": "abc-123",
            "transcript_path": "/x.jsonl",
            "cwd": "D:\\Projects\\tether",
            "hook_event_name": "Notification",
            "message": "Claude needs your permission to use Bash",
            "notification_type": "permission_prompt"
        });
        let m = build_message("claude", Some("Notification"), &payload, "deadbeef").unwrap();
        assert_eq!(m.session, "deadbeef");
        assert_eq!(m.event, "Notification");
        assert_eq!(m.notification_type.as_deref(), Some("permission_prompt"));
        assert_eq!(m.message.as_deref(), Some("Claude needs your permission to use Bash"));
        assert_eq!(m.cwd.as_deref(), Some("D:\\Projects\\tether"));
        assert_eq!(m.agent_session.as_deref(), Some("abc-123"));
        let line = serde_json::to_string(&m).unwrap();
        assert!(!line.contains('\n') && !line.contains("tool"), "compact, optional fields skipped: {line}");
        assert_eq!(serde_json::from_str::<IpcMessage>(&line).unwrap(), m);
    }

    #[test]
    fn prompt_is_cut_and_event_falls_back_to_payload() {
        let long = "x ".repeat(500);
        let payload = json!({"hook_event_name": "UserPromptSubmit", "prompt": long, "session_id": "s"});
        let m = build_message("claude", None, &payload, "ab").unwrap();
        assert_eq!(m.event, "UserPromptSubmit");
        assert_eq!(m.message.as_ref().unwrap().chars().count(), SNIPPET_CHARS);
        assert!(build_message("claude", None, &Value::Null, "ab").is_none());
        // A payload that is not JSON still reports the event named on the command line.
        let m = build_message("claude", Some("Stop"), &Value::Null, "ab").unwrap();
        assert_eq!((m.event.as_str(), m.message), ("Stop", None));
    }

    #[test]
    fn codex_notify_payload() {
        let payload = json!({
            "type": "agent-turn-complete",
            "turn-id": "t1",
            "cwd": "/home/me/app",
            "input-messages": ["rename the module"],
            "last-assistant-message": "Renamed `foo` to `bar`."
        });
        let m = build_message("Codex", None, &payload, "ab").unwrap();
        assert_eq!(m.agent, "codex");
        assert_eq!(m.event, "agent-turn-complete");
        assert_eq!(m.message.as_deref(), Some("Renamed `foo` to `bar`."));
        assert_eq!(m.agent_session.as_deref(), Some("t1"));
    }

    #[test]
    fn session_tags_are_validated() {
        assert!(valid_session("0123abcdef"));
        assert!(!valid_session(""));
        assert!(!valid_session("../../etc"));
        assert!(!valid_session(&"a".repeat(65)));
    }

    #[test]
    fn without_tether_env_the_hook_does_nothing_and_returns_fast() {
        // The test runner is not a tether terminal unless someone runs it inside one.
        if std::env::var_os("TETHER_SESSION").is_some() {
            return;
        }
        let t = Instant::now();
        run(&["claude".into(), "Stop".into()]);
        assert!(t.elapsed() < Duration::from_millis(200), "returns well before the {DEADLINE:?} send deadline");
    }
}
