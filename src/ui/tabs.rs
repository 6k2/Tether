//! Dock tabs (terminal / SFTP / editor) and the egui_dock TabViewer that renders them.

use std::sync::atomic::{AtomicU64, Ordering};

use eframe::egui::{self, Id, RichText, Ui, WidgetText};
use egui_dock::tab_viewer::OnCloseResponse;
use egui_dock::{DockState, Node, Split};

use super::editor_tab::EditorView;
use super::sftp_view::SftpView;
use super::terminal_view::TerminalView;
use crate::commands::{self, AppCommand, EditorDoc};
use crate::profile::Profile;
use crate::pty::{self, ShellSpec};
use crate::ssh::terminal::LinkState;
use crate::theme;
use crate::workspace::{TabKind, TabSpec};

static NEXT_UID: AtomicU64 = AtomicU64::new(1);
const APPEAR_SECS: f64 = 0.28;

pub fn next_uid() -> u64 {
    NEXT_UID.fetch_add(1, Ordering::Relaxed)
}

pub enum TabBody {
    Terminal(TerminalView),
    Sftp(Box<SftpView>),
    /// A remote file open in the built-in editor. Not saved in workspaces.
    Editor(Box<EditorView>),
}

pub struct Tab {
    pub uid: u64,
    /// The host; for a local shell, a stand-in (`pty::local_profile`) with a nil id.
    pub profile: Profile,
    pub body: TabBody,
    pub born: Option<f64>,
    /// Set for local shells: SSH-only features (SFTP links, cwd probes, `nano` hand-off)
    /// skip these tabs, and reconnecting respawns the shell.
    pub local: Option<ShellSpec>,
}

impl Tab {
    pub fn new(profile: Profile, body: TabBody) -> Self {
        Self { uid: next_uid(), profile, body, born: None, local: None }
    }

    pub fn new_local(spec: ShellSpec, view: TerminalView) -> Self {
        Self { local: Some(spec.clone()), ..Self::new(pty::local_profile(&spec), TabBody::Terminal(view)) }
    }

    pub fn is_local(&self) -> bool {
        self.local.is_some()
    }

    pub fn spec(&self) -> TabSpec {
        if let Some(shell) = &self.local {
            return TabSpec { profile_id: uuid::Uuid::nil(), kind: TabKind::Local { shell_id: shell.id.clone() } };
        }
        let kind = match self.body {
            TabBody::Terminal(_) => TabKind::Terminal,
            // Editors are skipped by workspaces (see `is_persistable`); reporting them as
            // SFTP keeps the host's connection counted as in use while a file is open.
            TabBody::Sftp(_) | TabBody::Editor(_) => TabKind::Sftp,
        };
        TabSpec { profile_id: self.profile.id, kind }
    }

    /// Whether the tab belongs in a saved workspace layout (editor tabs do not).
    pub fn is_persistable(&self) -> bool {
        !matches!(self.body, TabBody::Editor(_))
    }

    pub fn is_editor(&self) -> bool {
        matches!(self.body, TabBody::Editor(_))
    }

    /// True for the editor tab of `remote_path` on host `profile_id`.
    pub fn edits(&self, profile_id: uuid::Uuid, remote_path: &str) -> bool {
        matches!(&self.body, TabBody::Editor(e) if e.edits(profile_id, remote_path))
    }

    fn status_color(&self) -> egui::Color32 {
        match &self.body {
            TabBody::Terminal(t) => match t.link_state() {
                LinkState::Connected => theme::accent(),
                LinkState::Connecting => theme::warn(),
                _ => theme::danger(),
            },
            TabBody::Sftp(s) if s.is_ready() => theme::blue(),
            TabBody::Sftp(_) => theme::warn(),
            TabBody::Editor(e) => e.status_color(),
        }
    }
}

/// Applies the queued [`AppCommand`]s to the dock. Call once per frame.
pub fn apply_commands(dock: &mut DockState<Tab>, rt: &tokio::runtime::Handle, ctx: &egui::Context) {
    for cmd in commands::drain() {
        match cmd {
            AppCommand::OpenEditor(doc) => place_editor(dock, *doc, rt, ctx),
            AppCommand::CloseTab(uid) => {
                if let Some(path) = dock.find_tab_from(|t| t.uid == uid) {
                    dock.remove_tab(path);
                }
            }
        }
    }
}

/// Focuses the editor already showing the file, else joins the leaf holding other editors,
/// else opens it below the whole workspace when already split (or to the right of a single pane).
fn place_editor(dock: &mut DockState<Tab>, doc: EditorDoc, rt: &tokio::runtime::Handle, ctx: &egui::Context) {
    if let Some(path) = dock.find_tab_from(|t| t.edits(doc.profile.id, &doc.remote_path)) {
        let _ = dock.set_active_tab(path);
        dock.set_focused_node_and_surface(path.node_path());
        return;
    }
    let tab = Tab::new(doc.profile.clone(), TabBody::Editor(Box::new(EditorView::new(doc, rt.clone(), ctx.clone()))));
    if let Some(path) = dock.find_tab_from(Tab::is_editor)
        && let Ok(leaf) = dock.leaf_mut(path.node_path())
    {
        leaf.append_tab(tab);
        dock.set_focused_node_and_surface(path.node_path());
        return;
    }
    if dock.main_surface().root_node().is_some_and(Node::is_parent) {
        let [_, new] = dock.main_surface_mut().split_below(egui_dock::NodeIndex::root(), 0.5, vec![tab]);
        dock.set_focused_node_and_surface(egui_dock::NodePath { surface: egui_dock::SurfaceIndex::main(), node: new });
        return;
    }
    match dock.focused_leaf() {
        Some(path) => {
            dock.split(path, Split::Right, 0.5, Node::leaf_with(vec![tab]));
        }
        None => dock.push_to_focused_leaf(tab),
    }
}

/// Collected during rendering, applied by the app afterwards.
#[derive(Default)]
pub struct Viewer {
    pub inputs: Vec<(u64, Vec<u8>)>,
    pub reconnect: Vec<u64>,
    pub open_files: Vec<(u64, Vec<String>)>,
    pub focused_uid: Option<u64>,
    /// Shells picked from a pane's "+" menu, opened in that pane.
    pub new_local: Vec<(egui_dock::NodePath, &'static ShellSpec)>,
    /// Agents picked from a pane's "+" menu (`agents_panel::launch_menu`).
    pub new_agent: Vec<(egui_dock::NodePath, super::agents_panel::LaunchPick)>,
    /// Frame time, for animated tab badges.
    pub time: f64,
}

impl egui_dock::TabViewer for Viewer {
    type Tab = Tab;

    fn id(&mut self, tab: &mut Tab) -> Id {
        Id::new(("tab", tab.uid))
    }

    fn title(&mut self, tab: &mut Tab) -> WidgetText {
        let icon = match (&tab.body, &tab.local) {
            (TabBody::Terminal(_), Some(shell)) => shell.icon_kind.glyph(),
            (TabBody::Terminal(_), None) => "⌨",
            (TabBody::Sftp(_), _) => "📁",
            (TabBody::Editor(_), _) => "📝",
        };
        let remote_title = match (&mut tab.body, &tab.local) {
            (TabBody::Terminal(t), Some(shell)) => {
                let title = t.remote_title();
                Some(pty::tab_title(shell, title.as_deref(), t.remote_cwd()))
            }
            (TabBody::Terminal(t), None) => t.remote_title(),
            (TabBody::Sftp(_), _) => None,
            (TabBody::Editor(e), _) => Some(if e.is_dirty() { format!("{} •", e.file_name()) } else { e.file_name().to_string() }),
        };
        let name = remote_title.filter(|t| !t.trim().is_empty()).unwrap_or_else(|| tab.profile.display_name().to_string());
        let name: String = super::agent_badge::clean_title(&name).chars().take(32).collect();
        let mut job = egui::text::LayoutJob::default();
        let time = self.time;
        super::agent_badge::append_title_mark(&mut job, tab.uid, tab.status_color(), time);
        RichText::new(format!("{icon}  {name}")).size(12.5).append_to(&mut job, &egui::Style::default(), egui::FontSelection::Default, egui::Align::Center);
        if tab.is_local() {
            super::usage_chip::append_to_title(&mut job, tab.uid);
        }
        job.into()
    }

    fn ui(&mut self, ui: &mut Ui, tab: &mut Tab) {
        // Fade + rise when a tab first appears.
        let now = ui.input(|i| i.time);
        let born = *tab.born.get_or_insert(now);
        let t = ((now - born) / APPEAR_SECS).clamp(0.0, 1.0) as f32;
        let eased = 1.0 - (1.0 - t).powi(3);
        if t < 1.0 {
            ui.ctx().request_repaint();
            ui.set_opacity(eased);
            ui.add_space((1.0 - eased) * 10.0);
        }
        match &mut tab.body {
            TabBody::Terminal(view) => {
                let out = view.ui(ui, &tab.profile);
                if !out.input.is_empty() {
                    self.inputs.push((tab.uid, out.input));
                }
                if !out.open_files.is_empty() {
                    self.open_files.push((tab.uid, out.open_files));
                }
                if out.reconnect {
                    self.reconnect.push(tab.uid);
                }
                if ui.memory(|m| m.focused().is_some_and(|f| f == ui.id().with("terminal"))) {
                    self.focused_uid = Some(tab.uid);
                }
            }
            TabBody::Sftp(view) => {
                let out = view.ui(ui);
                if out.reconnect {
                    self.reconnect.push(tab.uid);
                }
                if out.close {
                    commands::push(AppCommand::CloseTab(tab.uid));
                }
            }
            TabBody::Editor(view) => {
                if view.ui(ui).close {
                    commands::push(AppCommand::CloseTab(tab.uid));
                }
            }
        }
    }

    fn on_close(&mut self, tab: &mut Tab) -> OnCloseResponse {
        match &mut tab.body {
            TabBody::Terminal(t) => t.handle.close(),
            // Unsaved changes: keep the tab and let the editor ask first.
            TabBody::Editor(e) => {
                if !e.request_close() {
                    return OnCloseResponse::Focus;
                }
            }
            TabBody::Sftp(_) => {}
        }
        OnCloseResponse::Close
    }

    fn on_tab_button(&mut self, tab: &mut Tab, response: &egui::Response) {
        if tab.is_local() {
            super::usage_chip::tab_tooltip(tab.uid, response);
        }
        if let TabBody::Editor(e) = &tab.body {
            response.clone().on_hover_text(format!("{}:{}", tab.profile.display_name(), e.remote_path()));
        }
    }

    /// The pane's "+" button: every detected local shell.
    fn add_popup(&mut self, ui: &mut Ui, path: egui_dock::NodePath) {
        ui.set_min_width(200.0);
        let shells = pty::shells::available();
        if shells.is_empty() {
            ui.label(RichText::new("No local shells found").color(theme::text_dim()));
        }
        for shell in shells {
            if ui.button(format!("{}  New {}", shell.icon_kind.glyph(), shell.name)).clicked() {
                self.new_local.push((path, shell));
                ui.close();
            }
        }
        if let Some(pick) = super::agents_panel::launch_menu(ui) {
            self.new_agent.push((path, pick));
        }
    }

    /// With a translucent window, terminals paint their own (translucent) background.
    fn clear_background(&self, tab: &Tab) -> bool {
        !(crate::themes::see_through() && matches!(tab.body, TabBody::Terminal(_)))
    }

    fn scroll_bars(&self, _tab: &Tab) -> [bool; 2] {
        [false, false]
    }
}
