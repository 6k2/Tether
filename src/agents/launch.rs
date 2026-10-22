//! "New → Claude Code / Codex": agents found on `PATH`, started by typing their command
//! into a fresh default shell (so the user lands in the shell when the agent exits).

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use super::AgentKind;

/// Definite missing-input cases, without rewriting the command the user entered.
pub fn needs_initial_prompt(command: &str) -> bool {
    let Ok(words) = shell_words::split(command) else { return false };
    let Some(program) = words.first() else { return false };
    let program = program.rsplit(['/', '\\']).next().unwrap_or(program).trim_end_matches(['.', ' ']).to_ascii_lowercase();
    let claude = ["claude", "claude.exe", "claude.cmd"].contains(&program.as_str());
    let codex = ["codex", "codex.exe", "codex.cmd"].contains(&program.as_str());
    if words.iter().any(|s| s == "--help" || s == "-h" || s == "--version") {
        return false;
    }
    if !(claude && words.iter().any(|s| s == "-p" || s == "--print") || codex && words.get(1).is_some_and(|s| s == "exec")) {
        return false;
    }
    let mut skip = false;
    for word in words.iter().skip(if codex { 2 } else { 1 }) {
        if skip {
            skip = false;
            continue;
        }
        if [
            "--model",
            "-m",
            "--output-format",
            "--input-format",
            "--permission-mode",
            "--allowedTools",
            "--disallowedTools",
            "--add-dir",
            "--system-prompt",
            "--append-system-prompt",
            "--max-turns",
            "--max-budget-usd",
            "--resume",
            "-r",
            "--cwd",
            "-C",
            "--config",
            "-c",
            "--profile",
            "--sandbox",
            "--output-last-message",
            "-o",
        ]
        .contains(&word.as_str())
        {
            skip = true;
            continue;
        }
        if !word.starts_with('-') && !word.is_empty() {
            return false;
        }
    }
    true
}

/// Literal shell argument for the optional initial prompt. Custom commands themselves
/// intentionally remain user-authored shell syntax.
pub fn append_initial_prompt(command: &str, prompt: &str, powershell: bool) -> String {
    if prompt.is_empty() {
        return command.trim().into();
    }
    let quoted = if powershell { format!("'{}'", prompt.replace('\'', "''")) } else { shell_words::quote(prompt).into_owned() };
    format!("{} {quoted}", command.trim())
}

#[derive(Debug, Clone)]
pub struct Launcher {
    pub kind: AgentKind,
    /// What is typed into the shell.
    pub command: &'static str,
    pub path: PathBuf,
}

const CANDIDATES: [(AgentKind, &str); 4] =
    [(AgentKind::Claude, "claude"), (AgentKind::Codex, "codex"), (AgentKind::Gemini, "gemini"), (AgentKind::OpenCode, "opencode")];

static FOUND: OnceLock<Vec<Launcher>> = OnceLock::new();

/// Agents installed on this machine (searched once).
pub fn available() -> &'static [Launcher] {
    FOUND.get_or_init(|| {
        let path = std::env::var_os("PATH").unwrap_or_default();
        let dirs: Vec<PathBuf> = std::env::split_paths(&path).collect();
        let exts = executable_extensions();
        CANDIDATES.iter().filter_map(|(kind, cmd)| find(&dirs, cmd, &exts).map(|path| Launcher { kind: *kind, command: cmd, path })).collect()
    })
}

/// The agents found, or `None` while the `PATH` scan is still running. Never blocks.
pub fn found() -> Option<&'static [Launcher]> {
    FOUND.get().map(Vec::as_slice)
}

/// Searches `PATH` on a background thread so the first "+" menu does not wait.
pub fn warm_up() {
    let _ = std::thread::Builder::new().name("tether-agent-scan".into()).spawn(|| {
        available();
    });
}

fn executable_extensions() -> Vec<String> {
    if !cfg!(windows) {
        return vec![String::new()];
    }
    let pathext = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
    pathext.split(';').filter(|e| !e.is_empty()).map(|e| e.to_ascii_lowercase()).collect()
}

pub fn find(dirs: &[PathBuf], name: &str, exts: &[String]) -> Option<PathBuf> {
    dirs.iter().flat_map(|d| exts.iter().map(move |e| d.join(format!("{name}{e}")))).find(|p| is_file(p))
}

fn is_file(p: &Path) -> bool {
    p.metadata().is_ok_and(|m| m.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn print_modes_require_real_input_but_interactive_commands_do_not() {
        for command in ["claude -p", "claude --print --model sonnet", "claude -p --dangerously-skip-permissions", "codex exec"] {
            assert!(needs_initial_prompt(command), "{command}");
        }
        for command in
            ["claude", "claude --dangerously-skip-permissions", "claude -p 'hello'", "claude -p --model sonnet 'hello'", "codex", "codex exec --help"]
        {
            assert!(!needs_initial_prompt(command), "{command}");
        }
        assert_eq!(append_initial_prompt("claude -p", "Don't run $(anything)", true), "claude -p 'Don''t run $(anything)'");
        let command = append_initial_prompt("claude -p", "hello 'quoted' world", false);
        assert_eq!(shell_words::split(&command).unwrap().last().unwrap(), "hello 'quoted' world");
    }

    #[test]
    fn finds_commands_with_extensions_in_path_order() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        std::fs::write(b.path().join("claude.cmd"), "").unwrap();
        std::fs::write(a.path().join("codex"), "").unwrap();
        std::fs::create_dir(a.path().join("claude.exe")).unwrap();
        let dirs = vec![a.path().to_path_buf(), b.path().to_path_buf()];
        let exts = vec![".exe".to_string(), ".cmd".to_string()];
        assert_eq!(find(&dirs, "claude", &exts), Some(b.path().join("claude.cmd")), "a folder named claude.exe is not a program");
        assert_eq!(find(&dirs, "codex", &[String::new()]), Some(a.path().join("codex")));
        assert_eq!(find(&dirs, "gemini", &exts), None);
    }
}
