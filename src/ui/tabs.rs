//! Dock tabs (terminal / SFTP / editor) and the egui_dock TabViewer that renders them.

use std::sync::atomic::{AtomicU64, Ordering};

use eframe::egui::{self, Id, RichText, Ui, WidgetText};
use egui_dock::tab_viewer::OnCloseResponse;
use egui_dock::{DockState, Node, Split};

use super::editor_tab::EditorView;
use super::sftp_view::SftpView;
use super::terminal_view::TerminalView;
use super::vertical_tabs::TabAction;
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
    /// A name the user gave the tab; replaces the automatic title.
    pub custom_title: Option<String>,
}

impl Tab {
    pub fn new(profile: Profile, body: TabBody) -> Self {
        Self { uid: next_uid(), profile, body, born: None, local: None, custom_title: None }
    }

    pub fn new_local(spec: ShellSpec, view: TerminalView) -> Self {
        Self { local: Some(spec.clone()), ..Self::new(pty::local_profile(&spec), TabBody::Terminal(view)) }
    }

    pub fn is_local(&self) -> bool {
        self.local.is_some()
    }

    pub fn spec(&self) -> TabSpec {
        if let Some(shell) = &self.local {
            return TabSpec { title: self.custom_title.clone(), ..TabSpec::new(uuid::Uuid::nil(), TabKind::Local { shell_id: shell.id.clone() }) };
        }
        let kind = match self.body {
            TabBody::Terminal(_) => TabKind::Terminal,
            // Editors are skipped by workspaces (see `is_persistable`); reporting them as
            // SFTP keeps the host's connection counted as in use while a file is open.
            TabBody::Sftp(_) | TabBody::Editor(_) => TabKind::Sftp,
        };
        TabSpec { title: self.custom_title.clone(), ..TabSpec::new(self.profile.id, kind) }
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

    /// Sets or clears (`None` / blank) the user's name for the tab.
    pub fn rename(&mut self, title: Option<String>) {
        self.custom_title = title.map(|t| t.trim().to_string()).filter(|t| !t.is_empty());
    }

    /// Glyph shown before the title.
    pub fn icon(&self) -> &'static str {
        match (&self.body, &self.local) {
            (TabBody::Terminal(_), Some(shell)) => shell.icon_kind.glyph(),
            (TabBody::Terminal(_), None) => "⌨",
            (TabBody::Sftp(_), _) => "📁",
            (TabBody::Editor(_), _) => "📝",
        }
    }

    /// The title tabs show: the user's name, else what the shell / file reports, else
    /// the host's name. Refreshes the terminal's cached title, hence `&mut`.
    pub fn display_title(&mut self) -> String {
        let remote_title = match (&mut self.body, &self.local) {
            (TabBody::Terminal(t), Some(shell)) => {
                let title = t.remote_title();
                Some(pty::tab_title(shell, title.as_deref(), t.remote_cwd()))
            }
            (TabBody::Terminal(t), None) => t.remote_title(),
            (TabBody::Sftp(_), _) => None,
            (TabBody::Editor(e), _) => Some(if e.is_dirty() { format!("{} •", e.file_name()) } else { e.file_name().to_string() }),
        };
        if let Some(custom) = &self.custom_title {
            return custom.clone();
        }
        let name = remote_title.filter(|t| !t.trim().is_empty()).unwrap_or_else(|| self.profile.display_name().to_string());
        super::agent_badge::clean_title(&name).to_string()
    }

    /// Second line in the vertical tab list: folder for local shells, address for
    /// remote ones, the file path for editors.
    pub fn subtitle(&self) -> String {
        match (&self.body, &self.local) {
            (TabBody::Terminal(t), Some(shell)) => t.remote_cwd().map(short_path).unwrap_or_else(|| {
                let program = shell.program.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
                format!("Local shell · {program}")
            }),
            (TabBody::Terminal(_), None) => self.profile.address(),
            (TabBody::Sftp(_), _) => format!("SFTP · {}", self.profile.address()),
            (TabBody::Editor(e), _) => e.remote_path().to_string(),
        }
    }

    pub fn is_terminal(&self) -> bool {
        matches!(self.body, TabBody::Terminal(_))
    }

    pub fn status_color(&self) -> egui::Color32 {
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
    /// Pane behaviour for this frame (set by the app before showing the dock).
    pub panes: PaneFx,
    /// A tab the pointer rests on that should take focus (focus follows mouse).
    pub hover_focus: Option<u64>,
    /// Terminal widget ids seen this frame (the next frame's `PaneFx::terminal_ids`).
    pub terminal_ids: Vec<Id>,
    /// Picked from a dock tab's context menu (or a double-click on it: rename).
    pub tab_actions: Vec<TabAction>,
    /// Rendering the main window's dock (tab actions only apply there).
    pub in_main: bool,
}

/// Per-frame inputs for pane dimming, outlines and focus handling.
#[derive(Default, Clone)]
pub struct PaneFx {
    /// The active tab of the focused pane.
    pub active_uid: Option<u64>,
    /// More than one pane is visible (dimming and outlines only make sense then).
    pub multi_pane: bool,
    /// 0 = off; how much unfocused terminal panes fade.
    pub dim: f32,
    /// Outline the focused pane (tab bars are hidden, so nothing else marks it).
    pub outline: bool,
    pub focus_follows_mouse: bool,
    /// Terminal widget ids of the previous frame: hover-focus only moves focus away
    /// from a terminal (or from nothing), never out of a text field.
    pub terminal_ids: Vec<Id>,
}

impl Viewer {
    pub fn with_panes(panes: PaneFx) -> Self {
        Self { panes, in_main: true, ..Self::default() }
    }

    /// Dim overlay, focus outline and focus-follows-mouse for a pane body.
    fn pane_effects(&mut self, ui: &mut Ui, tab: &Tab, body: egui::Rect) {
        let fx = &self.panes;
        let active = fx.active_uid == Some(tab.uid);
        let ctx = ui.ctx().clone();
        if fx.multi_pane && fx.dim > 0.0 && tab.is_terminal() {
            let t = ctx.animate_bool_with_time(Id::new(("pane_dim", tab.uid)), !active, theme::anim_time(0.18));
            if t > 0.0 {
                // Fade towards the terminal background: text recedes, the pane keeps its colour.
                let shade = theme::palette().term_bg.gamma_multiply(fx.dim * t);
                ui.painter().rect_filled(body, egui::CornerRadius::same(theme::RADIUS), shade);
            }
        }
        if fx.multi_pane && fx.outline {
            let t = ctx.animate_bool_with_time(Id::new(("pane_outline", tab.uid)), active, theme::anim_time(0.18));
            if t > 0.0 {
                let stroke = egui::Stroke::new(1.0, theme::accent().gamma_multiply(0.55 * t));
                ui.painter().rect_stroke(body.shrink(0.5), egui::CornerRadius::same(theme::RADIUS), stroke, egui::StrokeKind::Inside);
            }
        }
        if fx.focus_follows_mouse && !active && fx.multi_pane && ui.rect_contains_pointer(body) {
            let (moved, idle_buttons) = ui.input(|i| (i.pointer.delta() != egui::Vec2::ZERO, !i.pointer.any_down()));
            let free = ui.memory(|m| m.focused().is_none_or(|f| fx.terminal_ids.contains(&f)));
            if moved && idle_buttons && free && !egui::DragAndDrop::has_any_payload(&ctx) {
                self.hover_focus = Some(tab.uid);
            }
        }
    }
}

impl egui_dock::TabViewer for Viewer {
    type Tab = Tab;

    fn id(&mut self, tab: &mut Tab) -> Id {
        Id::new(("tab", tab.uid))
    }

    fn title(&mut self, tab: &mut Tab) -> WidgetText {
        let icon = tab.icon();
        let name: String = tab.display_title().chars().take(32).collect();
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
        let body = ui.max_rect();
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
                let term_id = ui.id().with("terminal");
                self.terminal_ids.push(term_id);
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
                if ui.memory(|m| m.focused().is_some_and(|f| f == term_id)) {
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
        self.pane_effects(ui, tab, body);
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
        if self.in_main && response.double_clicked() {
            self.tab_actions.push(TabAction::StartRename(tab.uid));
        }
        if self.in_main && response.middle_clicked() {
            self.tab_actions.push(TabAction::Close(tab.uid));
        }
    }

    /// Added below egui_dock's own entries in a tab's right-click menu.
    fn context_menu(&mut self, ui: &mut Ui, tab: &mut Tab, _path: egui_dock::NodePath) {
        if !self.in_main {
            return;
        }
        if ui.button("✏  Rename…").clicked() {
            self.tab_actions.push(TabAction::StartRename(tab.uid));
            ui.close();
        }
        super::vertical_tabs::tab_menu_items(ui, tab.uid, &mut self.tab_actions);
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

/// `C:\Users\me\src\tether` -> `~\src\tether`; long paths keep their last parts.
pub fn short_path(path: &str) -> String {
    let home = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")).unwrap_or_default();
    shorten_path(path, &home)
}

fn shorten_path(path: &str, home: &str) -> String {
    const KEEP: usize = 3;
    let trimmed = path.trim_end_matches(['/', '\\']);
    let home = home.trim_end_matches(['/', '\\']);
    let shown = match trimmed.strip_prefix(home) {
        Some(rest) if !home.is_empty() && (rest.is_empty() || rest.starts_with(['/', '\\'])) => format!("~{rest}"),
        _ => trimmed.to_string(),
    };
    if shown.is_empty() {
        return "/".into();
    }
    let sep = if shown.contains('\\') { '\\' } else { '/' };
    let parts: Vec<&str> = shown.split(['/', '\\']).filter(|p| !p.is_empty()).collect();
    if parts.len() > KEEP + 1 { format!("…{sep}{}", parts[parts.len() - KEEP..].join(&sep.to_string())) } else { shown }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_paths_use_tilde_and_keep_the_tail() {
        assert_eq!(shorten_path("/srv/app/", ""), "/srv/app");
        assert_eq!(shorten_path("/a/b/c/d/e", ""), "…/c/d/e");
        assert_eq!(shorten_path("/", ""), "/");
        assert_eq!(shorten_path(r"C:\Users\me\src", r"C:\Users\me"), r"~\src");
        assert_eq!(shorten_path(r"C:\Users\me", r"C:\Users\me\"), "~");
        assert_eq!(shorten_path(r"C:\Users\meow", r"C:\Users\me"), r"C:\Users\meow");
    }
}
