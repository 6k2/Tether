//! Headless screenshots of the layout features (vertical tabs, sidebar sections,
//! activity bar, command palette, dimmed panes, maximized pane, welcome screen), with
//! real local shells. PNGs land in `target/ui-shots/`.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use eframe::egui::{self, Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use tether::app::TetherApp;
use tether::layout::{Side, SidebarSection, TabPosition};

const PROFILES: &str = r#"[
 {"id":"22222222-2222-4222-8222-222222222222","name":"api-prod-01","group":"Production","host":"10.0.4.21","port":22,"user":"deploy","auth":"Password","hue":210,"pipeline_kb":4096,"remote_start_dir":"","last_used":1760000000},
 {"id":"33333333-3333-4333-8333-333333333333","name":"db-replica","group":"Production","host":"10.0.4.40","port":22,"user":"postgres","auth":"Password","hue":30,"pipeline_kb":4096,"remote_start_dir":"","last_used":1750000000},
 {"id":"44444444-4444-4444-8444-444444444444","name":"build-box","group":"Lab","host":"192.168.1.20","port":22,"user":"ci","auth":"Password","hue":280,"pipeline_kb":4096,"remote_start_dir":"","last_used":0}
]"#;

fn harness(config: &std::path::Path) -> Harness<'static, TetherApp> {
    std::fs::write(config.join("profiles.json"), PROFILES).unwrap();
    // SAFETY: this file has a single #[test]; nothing else reads the env concurrently.
    unsafe { std::env::set_var("TETHER_CONFIG_DIR", config) };
    Harness::builder().with_size(egui::vec2(1360.0, 820.0)).with_max_steps(10_000).wgpu().build_eframe(|cc| TetherApp::new(cc).expect("app init"))
}

fn shot(h: &mut Harness<'_, TetherApp>, name: &str) {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/ui-shots");
    std::fs::create_dir_all(&dir).unwrap();
    h.render().expect("render").save(dir.join(format!("{name}.png"))).unwrap();
}

/// Steps in real time (shells start on their own threads).
fn settle(h: &mut Harness<'_, TetherApp>, secs: f32) {
    let deadline = Instant::now() + Duration::from_secs_f32(secs);
    while Instant::now() < deadline {
        h.step();
        std::thread::sleep(Duration::from_millis(30));
    }
    h.run_steps(10);
}

fn keys(h: &mut Harness<'_, TetherApp>, m: Modifiers, k: Key) {
    h.key_press_modifiers(m, k);
    h.run_steps(4);
}

fn layout(f: impl FnOnce(&mut tether::layout::LayoutSettings)) {
    tether::settings::update(|s| f(&mut s.layout)).unwrap();
}

const CS: Modifiers = Modifiers { ctrl: true, shift: true, alt: false, mac_cmd: false, command: true };

#[test]
fn layout_screens() {
    let config = tempfile::tempdir().unwrap();
    let mut h = harness(config.path());
    h.run_steps(30);
    shot(&mut h, "20-welcome");
    if tether::pty::shells::default_shell().is_none() {
        eprintln!("no local shell: skipping the terminal layouts");
        return;
    }

    // Three local shells: two tabs in one pane, one split to the right.
    keys(&mut h, CS, Key::T);
    keys(&mut h, CS, Key::T);
    settle(&mut h, 1.0);
    h.get_all_by_label("Terminal").last().unwrap().click();
    h.run_steps(4);
    keys(&mut h, CS, Key::D);
    settle(&mut h, 2.5);

    // Vertical tabs on the left, grouped by pane.
    layout(|l| l.tab_position = TabPosition::Left);
    settle(&mut h, 0.6);
    shot(&mut h, "21-vertical-tabs-left");
    h.get_all_by_label_contains("Tab ").nth(1).unwrap().hover();
    h.run_steps(10);
    shot(&mut h, "21b-vertical-tabs-hover");

    // Tabs on the right, sidebar on the right with an activity bar, compact rows.
    layout(|l| {
        l.tab_position = TabPosition::Right;
        l.sidebar_side = Side::Left;
        l.activity_bar = true;
        l.active_view = SidebarSection::Hosts;
        l.compact_tabs = true;
    });
    settle(&mut h, 0.6);
    shot(&mut h, "22-tabs-right-activity-bar");
    layout(|l| {
        l.compact_tabs = false;
        l.activity_bar = false;
        l.sidebar_side = Side::Right;
        l.tab_position = TabPosition::Left;
    });
    settle(&mut h, 0.6);
    shot(&mut h, "22b-sidebar-right");

    // Tabs on top, sidebar sections reordered with one collapsed, panes dimmed.
    layout(|l| {
        l.tab_position = TabPosition::Top;
        l.sidebar_side = Side::Left;
        l.dim_inactive_panes = true;
        l.move_section(2, 0);
        l.set_collapsed(SidebarSection::Hosts, true);
    });
    settle(&mut h, 0.6);
    shot(&mut h, "24-dim-inactive-panes");

    // Command palette with a query.
    keys(&mut h, CS, Key::P);
    h.run_steps(10);
    shot(&mut h, "23a-palette-open");
    h.event(egui::Event::Text("split".into()));
    h.run_steps(10);
    shot(&mut h, "23-palette");
    h.key_press(Key::Escape);
    h.run_steps(10);
    assert!(h.query_by_label_contains("Palette ").is_none(), "Esc closes the palette");

    // Maximize the focused pane.
    layout(|l| l.set_collapsed(SidebarSection::Hosts, false));
    keys(&mut h, CS, Key::Enter);
    settle(&mut h, 0.6);
    h.get_by_label_contains("Maximized");
    shot(&mut h, "25-zoomed-pane");
    keys(&mut h, CS, Key::Enter);
    h.run_steps(10);
}
