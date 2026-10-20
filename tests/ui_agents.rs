//! Agent session tracking end to end: fake "claude.exe" processes (a copy of node.exe
//! named claude.exe running a script) in real local terminals, reporting through the
//! real `tether hook` binary and OSC sequences. Screenshots land in `target/ui-shots/`.
//!
//! Needs Windows, node on PATH and a GPU adapter:
//! `cargo test --test ui_agents -- --ignored --nocapture`

#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use eframe::egui::{self, Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use tether::app::TetherApp;
use tether::layout::TabPosition;

fn shot(h: &mut Harness<'_, TetherApp>, name: &str) {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/ui-shots");
    std::fs::create_dir_all(&dir).unwrap();
    h.render().expect("render").save(dir.join(format!("{name}.png"))).unwrap();
}

/// Steps in real time (the pty, the pipe and the monitor run on their own threads).
fn run_for(h: &mut Harness<'_, TetherApp>, secs: f32) {
    let deadline = Instant::now() + Duration::from_secs_f32(secs);
    while Instant::now() < deadline {
        h.step();
        std::thread::sleep(Duration::from_millis(30));
    }
}

/// A script for the fake agent: a busy Claude-style title, then a hook event.
fn script(dir: &Path, name: &str, title: &str, hook: &str, payload: &str, after_ms: u32, final_title: &str) -> PathBuf {
    // A shared target dir can hold another branch's build: allow pointing at a copy.
    let tether = std::env::var("TETHER_HOOK_EXE").unwrap_or_else(|_| env!("CARGO_BIN_EXE_tether").to_string()).replace('\\', "/");
    let body = format!(
        r#"const out = s => process.stdout.write(s);
out('\x1b]0;\u2810 {title}\x07');
const tick = setInterval(() => out('.'), 150);
setTimeout(() => {{
  clearInterval(tick);
  require('child_process').spawnSync('{tether}', ['hook', 'claude', '{hook}'], {{ input: JSON.stringify({payload}) }});
  if ('{final_title}') out('\x1b]0;{final_title}\x07');
  out('\r\n> ');
}}, {after_ms});
setTimeout(() => {{}}, 600000);
"#
    );
    let path = dir.join(name);
    std::fs::write(&path, body).unwrap();
    path
}

fn type_line(h: &mut Harness<'_, TetherApp>, line: &str) {
    h.get_by_label("Terminal").click();
    h.run_steps(2);
    h.get_by_label("Terminal").type_text(line);
    h.key_press(Key::Enter);
}

#[test]
#[ignore]
fn agent_sessions() {
    let Some(node) = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).map(|d| d.join("node.exe")).find(|p| p.is_file()) else {
        eprintln!("node.exe not on PATH; skipping");
        return;
    };
    let config = tempfile::tempdir().unwrap();
    std::fs::write(config.path().join("settings.json"), r#"{"terminal":{"default_shell":"cmd"}}"#).unwrap();
    // SAFETY: the only test in this binary.
    unsafe { std::env::set_var("TETHER_CONFIG_DIR", config.path()) };
    let work = tempfile::tempdir().unwrap();
    let fake = work.path().join("claude.exe");
    std::fs::copy(&node, &fake).unwrap();
    let perm = script(
        work.path(),
        "perm.js",
        "Refactor auth module",
        "Notification",
        r#"{hook_event_name:'Notification', message:'Claude needs your permission to use Bash', notification_type:'permission_prompt', cwd:'D:/work/api-server', session_id:'s1'}"#,
        3000,
        "",
    );
    let done = script(
        work.path(),
        "done.js",
        "Write release notes",
        "Stop",
        r#"{hook_event_name:'Stop', cwd:'D:/work/docs-site', session_id:'s2', last_assistant_message:'Release notes for 0.2 are in CHANGELOG.md.'}"#,
        2500,
        "\u{2733} Write release notes",
    );
    let busy = script(
        work.path(),
        "busy.js",
        "Migrate the database",
        "SessionStart",
        r#"{hook_event_name:'SessionStart', cwd:'D:/work/billing', session_id:'s3'}"#,
        400_000,
        "",
    );

    // A session that has not named its task yet: "Claude Code · <folder>".
    let fresh = script(
        work.path(),
        "fresh.js",
        "Claude Code",
        "SessionStart",
        r#"{hook_event_name:'SessionStart', cwd:'D:/work/website', session_id:'s4'}"#,
        800,
        "\u{2733} Claude Code",
    );

    let mut h = Harness::builder().with_size(egui::vec2(1360.0, 820.0)).with_max_steps(100_000).wgpu().build_eframe(|cc| TetherApp::new(cc).expect("app init"));
    h.run_steps(10);
    for js in [&perm, &done, &busy, &fresh] {
        h.key_press_modifiers(Modifiers::CTRL | Modifiers::SHIFT, Key::T);
        run_for(&mut h, 1.5);
        type_line(&mut h, &format!("\"{}\" \"{}\"", fake.display(), js.display()));
        run_for(&mut h, 0.5);
    }
    // A plain shell: no agent identity, usage only on hover.
    h.key_press_modifiers(Modifiers::CTRL | Modifiers::SHIFT, Key::T);
    run_for(&mut h, 5.0);
    shot(&mut h, "agents-01-top-tabs");
    h.get_by_label_contains("Agents ·");

    // Vertical rows, then the popover of the session asking for permission.
    layout(|l| l.tab_position = TabPosition::Left);
    run_for(&mut h, 1.0);
    shot(&mut h, "agents-02-vertical-rows");
    h.get_all_by_label("Agent status Permission").next().expect("permission pill").click();
    run_for(&mut h, 0.6);
    shot(&mut h, "agents-03-popover");
    h.get_by_label("Jump to tab").click();
    run_for(&mut h, 0.6);
    shot(&mut h, "agents-03b-pane-pill");

    h.key_press_modifiers(Modifiers::CTRL | Modifiers::SHIFT, Key::J);
    run_for(&mut h, 1.0);
    shot(&mut h, "agents-04-panel");
    h.key_press_modifiers(Modifiers::CTRL | Modifiers::SHIFT, Key::J);

    // Top tabs in a light theme.
    layout(|l| l.tab_position = TabPosition::Top);
    tether::themes::apply(&h.ctx, "Tether Day");
    run_for(&mut h, 1.0);
    shot(&mut h, "agents-05-light");

    // Narrow window: the palette pill becomes a button; the app menu holds the rest.
    h.set_size(egui::vec2(700.0, 820.0));
    run_for(&mut h, 0.6);
    shot(&mut h, "agents-06-narrow");
    h.get_by_label("App menu").click();
    run_for(&mut h, 0.6);
    shot(&mut h, "agents-07-app-menu");
    h.key_press(Key::Escape);
    tether::themes::apply(&h.ctx, "Tether Night");
    h.set_size(egui::vec2(1360.0, 820.0));
    run_for(&mut h, 0.6);

    // The top-bar badge jumps to the most urgent session.
    h.get_by_label_contains("Agents ·").click();
    run_for(&mut h, 1.0);
    shot(&mut h, "agents-08-jump");
}

fn layout(f: impl FnOnce(&mut tether::layout::LayoutSettings)) {
    tether::settings::update(|s| f(&mut s.layout)).unwrap();
}
