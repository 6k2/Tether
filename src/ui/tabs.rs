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
    pub profile: Profile,
    pub body: TabBody,
    pub born: Option<f64>,
}

impl Tab {
    pub fn new(profile: Profile, body: TabBody) -> Self {
        Self { uid: next_uid(), profile, body, born: None }
    }

    pub fn spec(&self) -> TabSpec {
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
            TabBody::Terminal(t) => match t.handle.shared.lock().map(|s| s.state.clone()) {
                Ok(LinkState::Connected) => theme::accent(),
                Ok(LinkState::Connecting) => theme::warn(),
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
/// else splits the focused leaf (the SFTP pane that opened it) to the right.
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
    pub focused_uid: Option<u64>,
}

impl egui_dock::TabViewer for Viewer {
    type Tab = Tab;

    fn id(&mut self, tab: &mut Tab) -> Id {
        Id::new(("tab", tab.uid))
    }

    fn title(&mut self, tab: &mut Tab) -> WidgetText {
        let icon = match tab.body {
            TabBody::Terminal(_) => "⌨",
            TabBody::Sftp(_) => "📁",
            TabBody::Editor(_) => "📝",
        };
        let remote_title = match &tab.body {
            TabBody::Terminal(t) => t.handle.shared.lock().ok().and_then(|s| s.parser.callbacks().title.clone()),
            TabBody::Sftp(_) => None,
            TabBody::Editor(e) => Some(if e.is_dirty() { format!("{} •", e.file_name()) } else { e.file_name().to_string() }),
        };
        let name = remote_title.filter(|t| !t.trim().is_empty()).unwrap_or_else(|| tab.profile.display_name().to_string());
        let name: String = name.chars().take(32).collect();
        let mut job = egui::text::LayoutJob::default();
        RichText::new("• ").color(tab.status_color()).size(16.0).append_to(
            &mut job,
            &egui::Style::default(),
            egui::FontSelection::Default,
            egui::Align::Center,
        );
        RichText::new(format!("{icon}  {name}")).size(12.5).append_to(&mut job, &egui::Style::default(), egui::FontSelection::Default, egui::Align::Center);
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
        if let TabBody::Editor(e) = &tab.body {
            response.clone().on_hover_text(format!("{}:{}", tab.profile.display_name(), e.remote_path()));
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
