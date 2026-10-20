//! AI coding agents (Claude Code, Codex CLI, ...) running in local terminals: which
//! one runs where, what it is doing, and the opt-in hooks that tell us precisely.
//!
//! Two layers feed one state machine ([`state::Tracker`]):
//! - heuristics that need no setup: the process tree (`procmon`), window titles,
//!   output activity, bells and OSC 9 / OSC 777 notifications;
//! - hook events the agents send through `tether hook` over a local pipe (`ipc`).

pub mod hook;
pub mod hub;
pub mod install;
pub mod ipc;
pub mod launch;
pub mod notify;
pub mod state;
pub mod title;

pub use hub::{AgentHub, Badge, SessionInfo};
pub use state::AgentState;

use serde::{Deserialize, Serialize};

/// A CLI agent we recognise in a terminal's process tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AgentKind {
    Claude,
    Codex,
    Gemini,
    OpenCode,
    Aider,
    Copilot,
    Goose,
    Amp,
    CursorAgent,
}

impl AgentKind {
    pub const ALL: [AgentKind; 9] = [
        AgentKind::Claude,
        AgentKind::Codex,
        AgentKind::Gemini,
        AgentKind::OpenCode,
        AgentKind::Aider,
        AgentKind::Copilot,
        AgentKind::Goose,
        AgentKind::Amp,
        AgentKind::CursorAgent,
    ];

    pub fn name(self) -> &'static str {
        match self {
            AgentKind::Claude => "Claude Code",
            AgentKind::Codex => "Codex",
            AgentKind::Gemini => "Gemini CLI",
            AgentKind::OpenCode => "opencode",
            AgentKind::Aider => "aider",
            AgentKind::Copilot => "Copilot CLI",
            AgentKind::Goose => "goose",
            AgentKind::Amp => "Amp",
            AgentKind::CursorAgent => "Cursor Agent",
        }
    }

    /// One-glyph mark used in tabs and the agents panel.
    pub fn glyph(self) -> &'static str {
        match self {
            AgentKind::Claude => "✳",
            AgentKind::Codex => "◎",
            AgentKind::Gemini => "✦",
            _ => "◆",
        }
    }

    /// Hue (degrees) of the agent's accent colour.
    pub fn hue(self) -> f32 {
        match self {
            AgentKind::Claude => 22.0,
            AgentKind::Codex => 200.0,
            AgentKind::Gemini => 255.0,
            AgentKind::OpenCode => 140.0,
            _ => 300.0,
        }
    }

    /// The id used on the `tether hook <agent>` command line.
    pub fn id(self) -> &'static str {
        match self {
            AgentKind::Claude => "claude",
            AgentKind::Codex => "codex",
            AgentKind::Gemini => "gemini",
            AgentKind::OpenCode => "opencode",
            AgentKind::Aider => "aider",
            AgentKind::Copilot => "copilot",
            AgentKind::Goose => "goose",
            AgentKind::Amp => "amp",
            AgentKind::CursorAgent => "cursor-agent",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.id().eq_ignore_ascii_case(id))
    }

    /// Recognises an agent from a process name (`claude.exe`) and its command line.
    /// Node / Bun / Python launchers are matched on the script they run
    /// (`node .../@anthropic-ai/claude-code/cli.js`, `python -m aider`).
    pub fn detect(name: &str, cmd: &[String]) -> Option<Self> {
        let stem = exe_stem(name);
        let by_name = match stem.as_str() {
            "claude" => Some(AgentKind::Claude),
            "codex" => Some(AgentKind::Codex),
            "gemini" => Some(AgentKind::Gemini),
            "opencode" => Some(AgentKind::OpenCode),
            "aider" => Some(AgentKind::Aider),
            "copilot" => Some(AgentKind::Copilot),
            "goose" => Some(AgentKind::Goose),
            "amp" => Some(AgentKind::Amp),
            "cursor-agent" => Some(AgentKind::CursorAgent),
            _ => None,
        };
        if by_name.is_some() {
            return by_name;
        }
        if !matches!(stem.as_str(), "node" | "bun" | "deno" | "python" | "python3" | "py" | "pythonw" | "npx" | "pnpm" | "uv" | "uvx") {
            return None;
        }
        // Only the script / module part, not the interpreter's own path.
        cmd.iter().skip(1).take(4).find_map(|arg| detect_script(&arg.replace('\\', "/").to_ascii_lowercase()))
    }
}

fn detect_script(arg: &str) -> Option<AgentKind> {
    const PACKAGES: [(&str, AgentKind); 8] = [
        ("@anthropic-ai/claude-code", AgentKind::Claude),
        ("@openai/codex", AgentKind::Codex),
        ("@google/gemini-cli", AgentKind::Gemini),
        ("opencode-ai", AgentKind::OpenCode),
        ("@github/copilot", AgentKind::Copilot),
        ("@sourcegraph/amp", AgentKind::Amp),
        ("aider-chat", AgentKind::Aider),
        ("/aider/", AgentKind::Aider),
    ];
    if let Some((_, kind)) = PACKAGES.iter().find(|(p, _)| arg.contains(p)) {
        return Some(*kind);
    }
    let last = arg.rsplit('/').next().unwrap_or(arg);
    match exe_stem(last).as_str() {
        "claude" => Some(AgentKind::Claude),
        "codex" => Some(AgentKind::Codex),
        "gemini" => Some(AgentKind::Gemini),
        "opencode" => Some(AgentKind::OpenCode),
        "aider" => Some(AgentKind::Aider),
        _ => None,
    }
}

/// `C:\x\Claude.EXE` -> `claude`.
fn exe_stem(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name).to_ascii_lowercase();
    for ext in [".exe", ".cmd", ".bat", ".ps1", ".js", ".mjs", ".cjs"] {
        if let Some(s) = base.strip_suffix(ext) {
            return s.to_string();
        }
    }
    base
}

/// Cuts `text` to `max` characters on one line, with an ellipsis.
pub fn snippet(text: &str, max: usize) -> String {
    let flat: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        return flat;
    }
    let mut out: String = flat.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmd(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn detects_native_binaries_by_name() {
        assert_eq!(AgentKind::detect("claude.exe", &[]), Some(AgentKind::Claude));
        assert_eq!(AgentKind::detect("Codex.EXE", &[]), Some(AgentKind::Codex));
        assert_eq!(AgentKind::detect("/usr/local/bin/opencode", &[]), Some(AgentKind::OpenCode));
        assert_eq!(AgentKind::detect("pwsh.exe", &cmd(&["pwsh.exe", "-NoLogo"])), None);
    }

    #[test]
    fn detects_node_and_python_launchers_by_script() {
        let c = cmd(&["node.exe", r"C:\Users\me\AppData\Roaming\npm\node_modules\@anthropic-ai\claude-code\cli.js"]);
        assert_eq!(AgentKind::detect("node.exe", &c), Some(AgentKind::Claude));
        let c = cmd(&["node", "/usr/lib/node_modules/@openai/codex/bin/codex.js", "--full-auto"]);
        assert_eq!(AgentKind::detect("node", &c), Some(AgentKind::Codex));
        let c = cmd(&["python3", "/home/me/.local/bin/aider", "--model", "x"]);
        assert_eq!(AgentKind::detect("python3", &c), Some(AgentKind::Aider));
        // A random node program, and an agent name in a later argument, are not agents.
        assert_eq!(AgentKind::detect("node.exe", &cmd(&["node", "server.js"])), None);
        assert_eq!(AgentKind::detect("vim", &cmd(&["vim", "claude.md"])), None);
    }

    #[test]
    fn ids_round_trip_and_snippets_cut() {
        for k in AgentKind::ALL {
            assert_eq!(AgentKind::from_id(k.id()), Some(k));
        }
        assert_eq!(snippet("a\n  b   c", 10), "a b c");
        assert_eq!(snippet("abcdefghijkl", 5), "abcd…");
    }
}
