//! What the command palette lists and how the app runs a chosen entry.
//! A child module of `app`. New built-in entries go in `build_registry`; other features
//! can add theirs with `actions::register_provider`.

use eframe::egui;

use super::TetherApp;
use crate::actions::{Command, Entry, Registry};
use crate::keybindings::{Action, Group};
use crate::layout::{Direction, TabPosition};
use crate::settings;
use crate::ui::palette::{Palette, PaletteOutcome};
use crate::ui::settings_view::SettingsView;
use crate::ui::sidebar::OpenKind;

impl TetherApp {
    pub(super) fn open_palette(&mut self) {
        if self.palette.is_none() {
            self.palette = Some(Palette::new(self.build_registry()));
        }
    }

    fn build_registry(&mut self) -> Registry {
        let mut r = Registry::default();
        let l = settings::snapshot().layout.clone();
        // Open tabs first: "go to" is the most common use.
        for (path, tab) in self.dock.iter_all_tabs_mut() {
            if !path.surface.is_main() {
                continue;
            }
            let title = tab.display_title();
            r.add(Entry::new("Tab", title, Command::FocusTab(tab.uid)).detail(tab.subtitle()));
        }
        for shell in crate::pty::shells::available() {
            let keys = (crate::pty::shells::default_shell().is_some_and(|d| d.id == shell.id)).then(|| crate::keybindings::label(Action::NewLocalTerminal));
            r.add(Entry::new("Shell", format!("New {}", shell.name), Command::NewShell(shell.id.clone())).keys(keys));
        }
        let mut hosts: Vec<_> = self.store.profiles.iter().filter(|p| !p.archived).collect();
        hosts.sort_by_key(|p| std::cmp::Reverse(p.last_used));
        for p in &hosts {
            r.add(Entry::new("Host", format!("Connect to {}", p.display_name()), Command::Connect(p.id)).detail(p.address()));
        }
        for a in Action::ALL {
            if is_global(a) {
                r.add(Entry::action(category(a.group()), a));
            }
        }
        for p in TabPosition::ALL.into_iter().filter(|p| *p != l.tab_position) {
            let title = match p {
                TabPosition::Top => "Tabs on top of panes".to_string(),
                _ => format!("Vertical tabs on the {}", p.label().to_lowercase()),
            };
            r.add(Entry::new("Layout", title, Command::SetTabPosition(p)));
        }
        let other = l.sidebar_side.flip();
        r.add(Entry::new("Layout", format!("Sidebar on the {}", other.label().to_lowercase()), Command::SetSidebarSide(other)));
        let flag = |on: bool| if on { "Hide" } else { "Show" };
        r.add(Entry::new("Layout", format!("{} activity bar", flag(l.activity_bar)), Command::ToggleActivityBar));
        let toggle = |on: bool| if on { "off" } else { "on" };
        r.add(Entry::new("Layout", format!("Turn {} dim inactive panes", toggle(l.dim_inactive_panes)), Command::ToggleDimPanes));
        r.add(Entry::new("Layout", format!("Turn {} focus follows mouse", toggle(l.focus_follows_mouse)), Command::ToggleFocusFollowsMouse));
        r.add(Entry::new("Layout", "Layout settings…", Command::OpenLayoutSettings));
        for l in crate::agents::launch::available() {
            let (kind, command) = (l.kind, l.command);
            r.add(Entry::new("Agent", format!("Launch {}", kind.name()), Command::LaunchAgent { kind, command, pick_folder: false }).detail(command));
            r.add(Entry::new("Agent", format!("Launch {} in folder…", kind.name()), Command::LaunchAgent { kind, command, pick_folder: true }));
        }
        for name in self.workspaces.names() {
            r.add(Entry::new("Workspace", format!("Restore {name}"), Command::RestoreWorkspace(name)));
        }
        for p in &hosts {
            r.add(Entry::new("SFTP", format!("Browse files on {}", p.display_name()), Command::ConnectSftp(p.id)).detail(p.address()));
        }
        let current = settings::snapshot().appearance.theme.clone();
        for t in crate::themes::all().iter().filter(|t| !t.name.eq_ignore_ascii_case(&current)) {
            r.add(Entry::new("Theme", t.name.clone(), Command::SetTheme(t.name.clone())).detail(t.tagline.clone()));
        }
        r.add_providers();
        r
    }

    pub(super) fn palette_frame(&mut self, ctx: &egui::Context) {
        let Some(palette) = &mut self.palette else { return };
        match palette.show(ctx) {
            PaletteOutcome::Open => {}
            PaletteOutcome::Close => self.palette = None,
            PaletteOutcome::Run(cmd) => {
                self.palette = None;
                self.run_command(cmd, ctx);
            }
        }
    }

    fn run_command(&mut self, cmd: Command, ctx: &egui::Context) {
        match cmd {
            Command::App(a) => self.run_action(a, ctx),
            Command::NewShell(id) => self.open_local(crate::pty::shells::find(&id), ctx),
            Command::Connect(id) => {
                if let Some(p) = self.store.get(id).cloned() {
                    self.request_open(p, OpenKind::Terminal, ctx);
                }
            }
            Command::ConnectSftp(id) => {
                if let Some(p) = self.store.get(id).cloned() {
                    self.request_open(p, OpenKind::Sftp, ctx);
                }
            }
            Command::SetTheme(name) => {
                let _ = settings::update(|s| {
                    s.appearance.theme = name.clone();
                    s.appearance.sync_with_os = false;
                });
                crate::themes::apply_animated(ctx, &name);
            }
            Command::SetTabPosition(p) => super::layout_ui::set_tab_position(p),
            Command::SetSidebarSide(side) => super::layout_ui::set_sidebar_side(side),
            Command::ToggleActivityBar => {
                let _ = settings::update(|s| s.layout.activity_bar = !s.layout.activity_bar);
            }
            Command::ToggleDimPanes => {
                let _ = settings::update(|s| s.layout.dim_inactive_panes = !s.layout.dim_inactive_panes);
            }
            Command::ToggleFocusFollowsMouse => {
                let _ = settings::update(|s| s.layout.focus_follows_mouse = !s.layout.focus_follows_mouse);
            }
            Command::FocusTab(uid) => self.show_tab(uid),
            Command::RestoreWorkspace(name) => self.restore_workspace(&name, ctx),
            Command::OpenLayoutSettings => self.open_layout_settings(),
            Command::LaunchAgent { kind, command, pick_folder } => {
                let pick = crate::ui::agents_panel::LaunchPick { kind, command, pick_folder };
                let target = self.dock.focused_leaf().or_else(|| self.dock.iter_leaves().next().map(|(p, _)| p)).unwrap_or(egui_dock::NodePath {
                    surface: egui_dock::SurfaceIndex::main(),
                    node: egui_dock::NodeIndex::root(),
                });
                self.open_agents_in(vec![(target, pick)], ctx);
            }
            Command::Run(f) => f(ctx),
        }
    }

    /// Runs a shortcut action (from its key or the palette).
    pub(super) fn run_action(&mut self, a: Action, ctx: &egui::Context) {
        use egui_dock::Split;
        use crate::workspace::TabKind;
        match a {
            Action::CommandPalette => {
                if self.palette.take().is_none() {
                    self.open_palette();
                }
            }
            Action::ToggleSidebar => super::layout_ui::toggle_sidebar(),
            Action::OpenSettings => {
                self.settings_view.get_or_insert_with(SettingsView::new);
            }
            Action::KeyManager => {
                if self.key_manager.is_none() {
                    self.key_manager = Some(crate::ui::key_manager::KeyManager::new(&self.keys));
                }
            }
            Action::NewHost => {
                if self.editor.is_none() {
                    self.open_editor_new(None);
                }
            }
            Action::NewFolder => self.new_folder(None),
            Action::QuickConnect => {
                show_sidebar_view(crate::layout::SidebarSection::Hosts);
                self.sidebar.focus_quick_connect();
            }
            Action::SearchHosts => {
                show_sidebar_view(crate::layout::SidebarSection::Hosts);
                self.sidebar.focus_search();
            }
            Action::NewLocalTerminal => self.open_local(None, ctx),
            Action::ToggleBroadcast => self.broadcast = !self.broadcast,
            Action::SplitRight => self.duplicate_focused(Split::Right, TabKind::Terminal, ctx),
            Action::SplitDown => self.duplicate_focused(Split::Below, TabKind::Terminal, ctx),
            Action::OpenSftp => self.duplicate_focused(Split::Right, TabKind::Sftp, ctx),
            Action::CloseTab => self.close_focused(),
            Action::NextTab => self.cycle_tab(true),
            Action::PrevTab => self.cycle_tab(false),
            Action::FocusPaneLeft => self.focus_pane(Direction::Left),
            Action::FocusPaneRight => self.focus_pane(Direction::Right),
            Action::FocusPaneUp => self.focus_pane(Direction::Up),
            Action::FocusPaneDown => self.focus_pane(Direction::Down),
            Action::ZoomPane => self.toggle_zoom(),
            Action::CycleTabPosition => super::layout_ui::set_tab_position(settings::snapshot().layout.tab_position.next()),
            Action::MoveSidebar => super::layout_ui::set_sidebar_side(settings::snapshot().layout.sidebar_side.flip()),
            Action::RenameTab => {
                if let Some(uid) = self.active_uid() {
                    self.apply_tab_actions(vec![crate::ui::vertical_tabs::TabAction::StartRename(uid)], ctx);
                }
            }
            Action::NextAttention => self.focus_next_attention(ctx),
            Action::ToggleAgents => self.toggle_agents(),
            // Handled by the focused terminal / hovered SFTP pane themselves.
            Action::FontBigger | Action::FontSmaller | Action::FontReset | Action::SftpBack | Action::SftpForward | Action::SftpUp | Action::SftpRefresh => {}
        }
    }
}

/// Makes sure the sidebar is open on `kind` (with the activity bar, that view).
fn show_sidebar_view(kind: crate::layout::SidebarSection) {
    let _ = settings::update(|s| {
        s.layout.sidebar_visible = true;
        if s.layout.activity_bar {
            s.layout.active_view = kind;
        }
        s.layout.set_collapsed(kind, false);
    });
}

fn category(g: Group) -> &'static str {
    match g {
        Group::General => "General",
        Group::Sessions => "Tab",
        Group::Panes => "Pane",
        Group::Terminal => "Terminal",
        Group::Sftp => "SFTP",
    }
}

/// Actions handled globally (others are left to the widget that has focus or hover).
pub(super) fn is_global(a: Action) -> bool {
    !matches!(a, Action::FontBigger | Action::FontSmaller | Action::FontReset) && a.group() != Group::Sftp
}
