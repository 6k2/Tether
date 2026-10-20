//! Application state and the frame loop: sidebar, dock workspace, modals, toasts.
//! Sidebar actions, the host editor and the key manager live in `app_actions.rs`.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use eframe::egui::{self, Key, RichText};
use egui_dock::{DockArea, DockState, Node, Split};
use uuid::Uuid;

#[path = "app_actions.rs"]
mod actions;
#[path = "app_local.rs"]
mod local;

use crate::folders::FolderStore;
use crate::keybindings::{self, Action};
use crate::keystore::KeyStore;
use crate::known_hosts::KnownHosts;
use crate::profile::{AuthMethod, Profile, ProfileStore, secret};
use crate::ssh::connection::Stage;
use crate::ssh::pool::Pool;
use crate::ssh::terminal::{LinkState, TerminalHandle, run_shell};
use crate::theme;
use crate::ui::detach::Detacher;
use crate::ui::editor::ProfileEditor;
use crate::ui::import_view::{ImportDialog, ImportOutcome};
use crate::ui::key_manager::KeyManager;
use crate::ui::settings_view::SettingsView;
use crate::ui::settings_widgets::{Icon, icon_button};
use crate::ui::sftp_view::SftpView;
use crate::ui::sidebar::{OpenKind, Sidebar, SidebarData, WorkspaceInfo};
use crate::ui::tabs::{Tab, TabBody, Viewer};
use crate::ui::terminal_view::TerminalView;
use crate::ui::toast::Toasts;
use crate::workspace::{self, LAST_SESSION, TabKind, Workspaces};

/// Password / passphrase asked before connecting (nothing stored, no live connection).
struct Prompt {
    profile: Profile,
    action: PromptAction,
    secret: String,
    remember: bool,
}

enum PromptAction {
    Open(OpenKind),
    Retry(u64),
}

pub struct TetherApp {
    rt: tokio::runtime::Runtime,
    pool: Pool,
    store: ProfileStore,
    folders: FolderStore,
    keys: KeyStore,
    workspaces: Workspaces,
    dock: DockState<Tab>,
    sidebar: Sidebar,
    sidebar_open: bool,
    broadcast: bool,
    editor: Option<ProfileEditor>,
    import: Option<ImportDialog>,
    settings_view: Option<SettingsView>,
    key_manager: Option<KeyManager>,
    prompt: Option<Prompt>,
    confirm_delete: Option<Uuid>,
    toasts: Toasts,
    last_layout: Vec<(Uuid, TabKind)>,
    live_profiles: HashSet<Uuid>,
    /// Profiles with open tabs, shared with pending disconnect tasks (see `track_layout`).
    in_use: Arc<Mutex<HashSet<Uuid>>>,
    focused_terminal: Option<u64>,
    /// Tabs dragged out into their own native windows (see `ui::detach`).
    detached: Detacher,
}

/// `TETHER_CONFIG_DIR` overrides the location (portable installs, tests).
pub fn config_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("TETHER_CONFIG_DIR") {
        return PathBuf::from(dir);
    }
    directories::ProjectDirs::from("dev", "tether", "tether").map(|d| d.config_dir().to_path_buf()).unwrap_or_else(|| PathBuf::from(".tether"))
}

impl TetherApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> anyhow::Result<Self> {
        let dir = config_dir();
        crate::settings::init(dir.join("settings.json"));
        crate::themes::apply_saved(&cc.egui_ctx);
        crate::updater::check_on_startup(&cc.egui_ctx);
        crate::pty::shells::warm_up();
        let toasts = Toasts::default();
        let mut store = ProfileStore::load(dir.join("profiles.json")).unwrap_or_else(|e| {
            toasts.error(format!("Could not read profiles: {e:#}"));
            ProfileStore::default()
        });
        let mut folders = FolderStore::load(dir.join("folders.json")).unwrap_or_else(|e| {
            toasts.error(format!("Could not read folders: {e:#}"));
            FolderStore::default()
        });
        // Older versions grouped hosts by a free-text name: turn those into folders.
        if folders.migrate_groups(&mut store.profiles) && (folders.save().is_err() || store.save().is_err()) {
            toasts.error("Could not save the migrated host folders");
        }
        let keys = KeyStore::load(crate::keystore::default_dir()).unwrap_or_else(|e| {
            toasts.error(format!("Could not read the key manager index: {e:#}"));
            KeyStore::default()
        });
        let rt = tokio::runtime::Builder::new_multi_thread().enable_all().thread_name("tether-io").build()?;
        Ok(Self {
            rt,
            pool: Pool::new(KnownHosts::load(dir.join("known_hosts.json"))),
            store,
            folders,
            keys,
            workspaces: Workspaces::load(dir.join("workspaces.json")),
            dock: DockState::new(Vec::new()),
            sidebar: Sidebar::default(),
            sidebar_open: true,
            broadcast: false,
            editor: None,
            import: None,
            settings_view: None,
            key_manager: None,
            prompt: None,
            confirm_delete: None,
            toasts,
            last_layout: Vec::new(),
            live_profiles: HashSet::new(),
            in_use: Arc::default(),
            focused_terminal: None,
            detached: Detacher::default(),
        })
    }

    // ---- opening sessions -------------------------------------------------------

    /// Whether connecting needs a password / passphrase the user hasn't stored.
    fn needs_secret(&self, p: &Profile) -> bool {
        if self.pool.has_live(p.id) {
            return false;
        }
        match &p.auth {
            AuthMethod::Password => secret::get(p.id).is_none(),
            AuthMethod::Managed { key } => self.keys.get(*key).is_some_and(|k| k.encrypted) && secret::get(*key).is_none(),
            AuthMethod::Key { .. } => false,
        }
    }

    fn request_open(&mut self, profile: Profile, kind: OpenKind, ctx: &egui::Context) {
        if self.needs_secret(&profile) {
            self.prompt = Some(Prompt { profile, action: PromptAction::Open(kind), secret: String::new(), remember: true });
            return;
        }
        let secret = secret::get(profile.secret_id());
        self.open_now(profile, kind, secret, ctx);
    }

    fn open_now(&mut self, profile: Profile, kind: OpenKind, secret: Option<String>, ctx: &egui::Context) {
        self.store.touch(profile.id);
        let _ = self.store.save();
        match kind {
            OpenKind::Terminal => {
                let tab = self.terminal_tab(&profile, secret, ctx);
                self.dock.push_to_focused_leaf(tab);
            }
            OpenKind::Sftp => {
                let tab = self.sftp_tab(&profile, secret, ctx);
                self.dock.push_to_focused_leaf(tab);
            }
            OpenKind::Both => {
                let term = self.terminal_tab(&profile, secret.clone(), ctx);
                let files = self.sftp_tab(&profile, secret, ctx);
                self.dock.push_to_focused_leaf(term);
                self.split_focused(files, Split::Right);
            }
        }
    }

    fn split_focused(&mut self, tab: Tab, split: Split) {
        match self.dock.focused_leaf() {
            Some(path) => {
                self.dock.split(path, split, 0.5, Node::leaf_with(vec![tab]));
            }
            None => self.dock.push_to_focused_leaf(tab),
        }
    }

    fn spawn_terminal(&self, profile: &Profile, secret: Option<String>, ctx: &egui::Context) -> TerminalView {
        let (handle, rx) = TerminalHandle::new(24, 80);
        let (pool, profile, h, ctx, toasts) = (self.pool.clone(), profile.clone(), handle.clone(), ctx.clone(), self.toasts.clone());
        self.rt.spawn(async move {
            let repaint = {
                let ctx = ctx.clone();
                move || ctx.request_repaint()
            };
            let on_stage = {
                let (h, ctx) = (h.clone(), ctx.clone());
                move |stage: Stage| {
                    h.set_stage(stage);
                    ctx.request_repaint();
                }
            };
            match pool.get_staged(&profile, secret, &on_stage).await {
                Ok(conn) => {
                    if let Some(fp) = conn.learned_fingerprint.lock().ok().and_then(|mut f| f.take()) {
                        toasts.info(format!("New host key trusted for {}\n{fp}", profile.host));
                    }
                    if let Err(e) = run_shell(conn, h.clone(), rx, repaint).await {
                        h.set_state(LinkState::Failed(format!("{e:#}")));
                    }
                }
                Err(e) => h.set_state(LinkState::Failed(format!("{e:#}"))),
            }
            ctx.request_repaint();
        });
        TerminalView::new(handle)
    }

    fn terminal_tab(&self, profile: &Profile, secret: Option<String>, ctx: &egui::Context) -> Tab {
        Tab::new(profile.clone(), TabBody::Terminal(self.spawn_terminal(profile, secret, ctx)))
    }

    fn sftp_tab(&self, profile: &Profile, secret: Option<String>, ctx: &egui::Context) -> Tab {
        let view = SftpView::new(profile.clone(), self.rt.handle().clone(), ctx.clone());
        view.connect(self.pool.clone(), secret, self.toasts.clone());
        Tab::new(profile.clone(), TabBody::Sftp(Box::new(view)))
    }

    fn reconnect(&mut self, uid: u64, secret_override: Option<String>, ctx: &egui::Context) {
        if self.all_tabs().any(|t| t.uid == uid && t.is_local()) {
            self.respawn_local(uid, ctx);
            return;
        }
        let Some((profile, is_terminal)) = self.all_tabs().find(|t| t.uid == uid).map(|t| (t.profile.clone(), matches!(t.body, TabBody::Terminal(_)))) else {
            return;
        };
        let profile = self.store.get(profile.id).cloned().unwrap_or(profile);
        if secret_override.is_none() && self.needs_secret(&profile) {
            self.prompt = Some(Prompt { profile, action: PromptAction::Retry(uid), secret: String::new(), remember: true });
            return;
        }
        let secret = secret_override.or_else(|| secret::get(profile.secret_id()));
        let new_view = is_terminal.then(|| self.spawn_terminal(&profile, secret.clone(), ctx));
        let Some(tab) = self.dock.iter_all_tabs_mut().map(|(_, t)| t).chain(self.detached.tabs_mut()).find(|t| t.uid == uid) else { return };
        tab.profile = profile;
        match (&mut tab.body, new_view) {
            (TabBody::Terminal(view), Some(v)) => *view = v,
            (TabBody::Sftp(view), _) => {
                view.profile = tab.profile.clone();
                view.connect(self.pool.clone(), secret, self.toasts.clone());
            }
            _ => {}
        }
    }

    fn duplicate_focused(&mut self, split: Split, kind: TabKind, ctx: &egui::Context) {
        let Some((_, tab)) = self.dock.find_active_focused() else { return };
        if let Some((spec, cwd)) = local::origin(tab) {
            // A local shell has no files pane; splits run the same shell again.
            if kind == TabKind::Terminal {
                let tab = self.local_tab(&spec, cwd, ctx);
                self.split_focused(tab, split);
            }
            return;
        }
        let profile = tab.profile.clone();
        let secret = secret::get(profile.secret_id());
        let tab = match kind {
            TabKind::Sftp => self.sftp_tab(&profile, secret, ctx),
            TabKind::Terminal | TabKind::Local { .. } => self.terminal_tab(&profile, secret, ctx),
        };
        self.split_focused(tab, split);
    }

    /// Closes the focused tab the way its close button would (editors may refuse).
    fn close_focused(&mut self) {
        let Some((_, tab)) = self.dock.find_active_focused() else { return };
        let close = match &mut tab.body {
            TabBody::Terminal(t) => {
                t.handle.close();
                true
            }
            TabBody::Editor(e) => e.request_close(),
            TabBody::Sftp(_) => true,
        };
        let uid = tab.uid;
        if close && let Some(path) = self.dock.find_tab_from(|t| t.uid == uid) {
            self.dock.remove_tab(path);
        }
    }

    /// Activates the next (or previous) tab of the focused pane, wrapping around.
    fn cycle_tab(&mut self, forward: bool) {
        let Some(path) = self.dock.focused_leaf() else { return };
        let Ok(leaf) = self.dock.leaf_mut(path) else { return };
        let n = leaf.tabs.len();
        if n < 2 {
            return;
        }
        let next = if forward { (leaf.active.0 + 1) % n } else { (leaf.active.0 + n - 1) % n };
        let _ = leaf.set_active_tab(next);
    }

    // ---- detached windows ---------------------------------------------------------

    /// Tabs of the main dock and of every detached window. Anything that must see
    /// every open tab (connection tracking, counts, lookups) iterates this.
    fn all_tabs(&self) -> impl Iterator<Item = &Tab> {
        self.dock.iter_all_tabs().map(|(_, t)| t).chain(self.detached.tabs())
    }

    /// Shows the native windows of detached tabs and applies their tab output like
    /// `workspace_area` does for the main dock.
    fn detached_windows(&mut self, ctx: &egui::Context) {
        let mut viewer = Viewer::default();
        self.detached.frame(ctx, &mut self.dock, &mut viewer);
        if viewer.focused_uid.is_some() {
            self.focused_terminal = viewer.focused_uid;
        }
        self.route_input(viewer.inputs);
        for uid in viewer.reconnect {
            self.reconnect(uid, None, ctx);
        }
        for (uid, files) in viewer.open_files {
            self.open_terminal_files(uid, files, ctx);
        }
    }

    // ---- workspaces -------------------------------------------------------------

    fn layout_specs(&self) -> Option<workspace::Layout<workspace::TabSpec>> {
        // Editor tabs hold unsaved buffers, not layout: never restore them as panes.
        workspace::capture(&self.dock, |t: &Tab| t.is_persistable().then(|| t.spec()))
    }

    /// Saved layouts as the sidebar shows them (tab count and host names).
    fn workspace_infos(&self) -> Vec<WorkspaceInfo> {
        self.workspaces
            .names()
            .into_iter()
            .filter_map(|name| {
                let tabs = self.workspaces.get(&name)?.tabs();
                let mut hosts: Vec<String> = Vec::new();
                for spec in &tabs {
                    let name = match &spec.kind {
                        TabKind::Local { shell_id } => crate::pty::shells::find(shell_id).map(|s| s.name.clone()),
                        _ => self.store.get(spec.profile_id).map(|p| p.display_name().to_string()),
                    };
                    if let Some(name) = name
                        && !hosts.contains(&name)
                    {
                        hosts.push(name);
                    }
                }
                Some(WorkspaceInfo { name, tabs: tabs.len(), hosts })
            })
            .collect()
    }

    fn restore_workspace(&mut self, name: &str, ctx: &egui::Context) {
        let Some(layout) = self.workspaces.get(name).cloned() else { return };
        let restored = workspace::rebuild(&layout, &mut |spec| {
            if let TabKind::Local { shell_id } = &spec.kind {
                return crate::pty::shells::find(shell_id).map(|shell| self.local_tab(shell, None, ctx));
            }
            let profile = self.store.get(spec.profile_id)?.clone();
            let secret = secret::get(profile.secret_id());
            Some(match spec.kind {
                TabKind::Sftp => self.sftp_tab(&profile, secret, ctx),
                TabKind::Terminal | TabKind::Local { .. } => self.terminal_tab(&profile, secret, ctx),
            })
        });
        for (_, tab) in self.dock.iter_all_tabs() {
            if let TabBody::Terminal(t) = &tab.body {
                t.handle.close();
            }
        }
        self.dock = restored;
        self.toasts.success(format!("Workspace “{name}” restored"));
    }

    /// Keeps "Last session" current and closes transports nobody uses any more.
    fn track_layout(&mut self) {
        let layout: Vec<(Uuid, TabKind)> = self
            .all_tabs()
            .map(|t| {
                let s = t.spec();
                (s.profile_id, s.kind)
            })
            .collect();
        if layout != self.last_layout {
            if !layout.is_empty() {
                let _ = self.workspaces.put(LAST_SESSION.into(), self.layout_specs());
            }
            self.last_layout = layout;
        }
        // Local shells (nil id) hold no pooled connection.
        let in_use: HashSet<Uuid> = self.last_layout.iter().map(|(id, _)| *id).filter(|id| !id.is_nil()).collect();
        if let Ok(mut shared) = self.in_use.lock() {
            shared.clone_from(&in_use);
        }
        for gone in self.live_profiles.difference(&in_use) {
            let (pool, id, shared) = (self.pool.clone(), *gone, self.in_use.clone());
            // Re-checked under the pool slot lock: the profile may be reopened before this runs.
            self.rt.spawn(async move { pool.drop_connection(id, || shared.lock().map(|s| !s.contains(&id)).unwrap_or(false)).await });
        }
        self.live_profiles = in_use;
    }

    // ---- frame --------------------------------------------------------------------

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        let pressed = |a: Action| keybindings::consume(ctx, a);
        if pressed(Action::ToggleSidebar) {
            self.sidebar_open = !self.sidebar_open;
        }
        if pressed(Action::OpenSettings) {
            self.settings_view.get_or_insert_with(SettingsView::new);
        }
        if pressed(Action::KeyManager) && self.key_manager.is_none() {
            self.key_manager = Some(KeyManager::new(&self.keys));
        }
        if pressed(Action::NewHost) && self.editor.is_none() {
            self.open_editor_new(None);
        }
        if pressed(Action::NewFolder) {
            self.new_folder(None);
        }
        if pressed(Action::QuickConnect) {
            self.sidebar_open = true;
            self.sidebar.focus_quick_connect();
        }
        if pressed(Action::SearchHosts) {
            self.sidebar_open = true;
            self.sidebar.focus_search();
        }
        if pressed(Action::NewLocalTerminal) {
            self.open_local(None, ctx);
        }
        if pressed(Action::ToggleBroadcast) {
            self.broadcast = !self.broadcast;
        }
        if pressed(Action::SplitRight) {
            self.duplicate_focused(Split::Right, TabKind::Terminal, ctx);
        }
        if pressed(Action::SplitDown) {
            self.duplicate_focused(Split::Below, TabKind::Terminal, ctx);
        }
        if pressed(Action::OpenSftp) {
            self.duplicate_focused(Split::Right, TabKind::Sftp, ctx);
        }
        if pressed(Action::CloseTab) {
            self.close_focused();
        }
        if pressed(Action::NextTab) {
            self.cycle_tab(true);
        }
        if pressed(Action::PrevTab) {
            self.cycle_tab(false);
        }
    }

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top("top").exact_size(38.0).frame(egui::Frame::new().fill(theme::bg()).inner_margin(egui::Margin::symmetric(10, 6))).show(ui, |ui| {
            ui.horizontal_centered(|ui| {
                if ui
                    .add(egui::Button::new(RichText::new("☰").size(15.0)).frame(false))
                    .on_hover_text(keybindings::hint("Toggle sidebar", Action::ToggleSidebar))
                    .clicked()
                {
                    self.sidebar_open = !self.sidebar_open;
                }
                if ui
                    .add(egui::Button::new(RichText::new("⬇ Import").size(12.0)).frame(false))
                    .on_hover_text("Import hosts from Bitvise or an SSH config")
                    .clicked()
                {
                    self.import = Some(ImportDialog::new(&self.store));
                }
                if ui
                    .add(egui::Button::new(RichText::new("🔑 Keys").size(12.0)).frame(false))
                    .on_hover_text(keybindings::hint("Key manager", Action::KeyManager))
                    .clicked()
                {
                    self.key_manager = Some(KeyManager::new(&self.keys));
                }
                let tabs = self.all_tabs().count();
                ui.label(RichText::new(format!("{tabs} session{}", if tabs == 1 { "" } else { "s" })).size(11.5).color(theme::text_faint()));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if icon_button(ui, Icon::Gear, 24.0).on_hover_text(keybindings::hint("Settings", Action::OpenSettings)).clicked() {
                        self.settings_view.get_or_insert_with(SettingsView::new);
                    }
                    let glow = ui.ctx().animate_bool_with_time(egui::Id::new("bc_glow"), self.broadcast, 0.2);
                    let fill = theme::danger().gamma_multiply(0.25 * glow);
                    let text = RichText::new("📡 Broadcast").size(12.0).color(if self.broadcast { theme::danger() } else { theme::text_dim() });
                    if ui
                        .add(egui::Button::new(text).fill(fill))
                        .on_hover_text(keybindings::hint("Type into every terminal at once", Action::ToggleBroadcast))
                        .clicked()
                    {
                        self.broadcast = !self.broadcast;
                    }
                    crate::ui::update_banner::top_bar(ui);
                    let hint = [(Action::SplitRight, "split"), (Action::SplitDown, "split down"), (Action::OpenSftp, "files")]
                        .iter()
                        .filter_map(|(a, what)| keybindings::binding(*a).map(|b| format!("{} {what}", b.label())))
                        .collect::<Vec<_>>()
                        .join(" · ");
                    ui.label(RichText::new(hint).size(11.0).color(theme::text_faint()));
                });
            });
        });
    }

    fn side_panel(&mut self, ui: &mut egui::Ui) {
        let frame = egui::Frame::new().fill(theme::bg()).inner_margin(egui::Margin { left: 12, right: 8, top: 4, bottom: 10 });
        let workspaces = self.workspace_infos();
        let pool = self.pool.clone();
        let is_live = move |id| pool.has_live(id);
        let has_tabs = self.layout_specs().is_some();
        let shells = crate::pty::shells::available();
        let data = SidebarData { store: &self.store, folders: &self.folders, workspaces: &workspaces, has_tabs, is_live: &is_live, shells };
        let sidebar = &mut self.sidebar;
        let actions = egui::Panel::left("sidebar")
            .resizable(true)
            .default_size(260.0)
            .size_range(200.0..=420.0)
            .frame(frame)
            .show_separator_line(false)
            .show_collapsible(ui, &mut self.sidebar_open, |ui| sidebar.ui(ui, &data))
            .map(|r| r.inner)
            .unwrap_or_default();
        for a in actions {
            self.apply_sidebar(a, ui.ctx());
        }
    }

    fn workspace_area(&mut self, ui: &mut egui::Ui) {
        // Translucent window: the panel must not stack its fill under the terminals.
        let see_through = crate::themes::see_through() && self.dock.iter_all_tabs().next().is_some();
        let fill = if see_through { egui::Color32::TRANSPARENT } else { theme::bg() };
        let frame = egui::Frame::new().fill(fill).inner_margin(egui::Margin { left: 0, right: 6, top: 0, bottom: 6 });
        egui::CentralPanel::default().frame(frame).show(ui, |ui| {
            if self.dock.iter_all_tabs().next().is_none() {
                self.welcome(ui);
                return;
            }
            let mut style = theme::dock_style(ui.style());
            if see_through {
                style.tab.tab_body.bg_fill = theme::surface().gamma_multiply(crate::themes::window_opacity());
                crate::ui::backdrop::paint_frame(ui, egui::vec2(6.0, 6.0));
            }
            let mut viewer = Viewer::default();
            crate::ui::tabs::apply_commands(&mut self.dock, self.rt.handle(), ui.ctx());
            DockArea::new(&mut self.dock)
                .style(style)
                // Floating windows may leave the app: `Detacher` turns them into native windows.
                .window_bounds(crate::ui::detach_geom::floating_bounds(ui.ctx().content_rect()))
                // Each pane's "+" lists the local shells (`Viewer::add_popup`).
                .show_add_buttons(true)
                .show_add_popup(true)
                .show_leaf_collapse_buttons(false)
                .show_leaf_close_all_buttons(false)
                .show_inside(ui, &mut viewer);
            self.open_local_in(std::mem::take(&mut viewer.new_local), ui.ctx());
            if viewer.focused_uid.is_some() {
                self.focused_terminal = viewer.focused_uid;
            }
            self.route_input(viewer.inputs);
            for uid in viewer.reconnect {
                self.reconnect(uid, None, ui.ctx());
            }
            for (uid, files) in viewer.open_files {
                self.open_terminal_files(uid, files, ui.ctx());
            }
        });
    }

    fn route_input(&mut self, inputs: Vec<(u64, Vec<u8>)>) {
        for (uid, bytes) in inputs {
            for tab in self.dock.iter_all_tabs().map(|(_, t)| t).chain(self.detached.tabs()) {
                if let TabBody::Terminal(t) = &tab.body
                    && (tab.uid == uid || self.broadcast)
                {
                    t.handle.send(bytes.clone());
                }
            }
        }
    }

    /// Opens files requested from a terminal (`nano <file>`) in the built-in editor.
    fn open_terminal_files(&mut self, uid: u64, files: Vec<String>, ctx: &egui::Context) {
        // Local shells edit files with their own tools: only remote ones go through SFTP.
        let Some((profile, handle)) = self.all_tabs().find_map(|t| match &t.body {
            TabBody::Terminal(term) if t.uid == uid && !t.is_local() => Some((t.profile.clone(), term.handle.clone())),
            _ => None,
        }) else {
            return;
        };
        let profile = self.store.get(profile.id).cloned().unwrap_or(profile);
        let (pool, toasts, ctx) = (self.pool.clone(), self.toasts.clone(), ctx.clone());
        let osc_cwd = handle.shared.lock().ok().and_then(|s| s.parser.callbacks().cwd.clone());
        let (age_secs, rows, cols) = handle.probe_facts();
        self.rt.spawn(async move {
            let conn = match pool.get(&profile, secret::get(profile.secret_id())).await {
                Ok(c) => c,
                Err(e) => {
                    toasts.error(format!("Could not open file in editor: {e:#}"));
                    ctx.request_repaint();
                    return;
                }
            };
            let sftp = match crate::ssh::sftp::Sftp::open(&conn).await {
                Ok(s) => s,
                Err(e) => {
                    toasts.error(format!("Could not open SFTP for editor: {e:#}"));
                    ctx.request_repaint();
                    return;
                }
            };
            let home = sftp.canonicalize(".").await.unwrap_or_else(|_| "/".into());
            let cwd = match osc_cwd {
                Some(c) => c,
                None => {
                    let probe = crate::ssh::cwd::Probe { tag: &handle.tag, age_secs, rows, cols };
                    crate::ssh::cwd::terminal_cwd(&conn, probe).await.unwrap_or_else(|_| home.clone())
                }
            };
            for file in files {
                let raw_path = if file == "~" {
                    home.clone()
                } else if let Some(rest) = file.strip_prefix("~/") {
                    crate::ssh::sftp::join(&home, rest)
                } else if file.starts_with('/') {
                    file
                } else {
                    crate::ssh::sftp::join(&cwd, &file)
                };
                let remote_path = crate::ui::path_complete::normalize_remote(&raw_path);
                let read = match crate::remote_edit::read_capped(&sftp, &remote_path, crate::remote_edit::MAX_EDITOR_BYTES).await {
                    Ok(bytes) => Ok(bytes),
                    Err(e) if is_no_such_file(&e) => Ok(Vec::new()),
                    Err(e) => Err(e),
                };
                match read {
                    Ok(bytes) => match crate::remote_edit::decode(&bytes) {
                        Ok(_) => {
                            let pipeline_kb = profile.pipeline_kb;
                            crate::commands::push(crate::commands::AppCommand::OpenEditor(Box::new(crate::commands::EditorDoc {
                                profile: profile.clone(),
                                remote_path,
                                bytes,
                                sftp: sftp.clone(),
                                pipeline_kb,
                            })));
                        }
                        Err(r) => toasts.error(format!("Cannot open {remote_path}: {r}")),
                    },
                    Err(e) => toasts.error(format!("Cannot open {remote_path}: {e:#}")),
                }
            }
            ctx.request_repaint();
        });
    }

    fn welcome(&mut self, ui: &mut egui::Ui) {
        let t = ui.ctx().animate_bool_with_time(egui::Id::new("welcome"), true, 0.4);
        ui.set_opacity(t);
        ui.vertical_centered(|ui| {
            ui.add_space(ui.available_height() * 0.22);
            crate::ui::sidebar::logo(ui, 56.0);
            ui.label(RichText::new("tether").size(28.0).strong());
            ui.label(RichText::new("Local and SSH terminals, pipelined SFTP, side by side.").color(theme::text_dim()));
            ui.add_space(22.0);
            let mut shell = None;
            if let Some(default) = crate::pty::shells::default_shell() {
                let text = RichText::new(format!("{}  New {}", default.icon_kind.glyph(), default.name)).size(13.0);
                let hint = keybindings::hint("Local terminal", Action::NewLocalTerminal);
                if ui.add(egui::Button::new(text).min_size(egui::vec2(320.0, 34.0)).fill(theme::accent_soft())).on_hover_text(hint).clicked() {
                    shell = Some(default);
                }
                ui.horizontal_wrapped(|ui| {
                    ui.set_max_width(320.0);
                    for s in crate::pty::shells::available().iter().filter(|s| s.id != default.id) {
                        if ui.add(egui::Button::new(RichText::new(&s.name).size(11.5)).frame(false)).clicked() {
                            shell = Some(s);
                        }
                    }
                });
                ui.add_space(10.0);
            }
            let mut recent: Vec<&Profile> = self.store.profiles.iter().filter(|p| p.last_used > 0 && !p.archived).collect();
            recent.sort_by_key(|p| std::cmp::Reverse(p.last_used));
            let mut open = None;
            for p in recent.into_iter().take(4) {
                let text = RichText::new(format!("⌨  {}   ·   {}", p.display_name(), p.address())).size(13.0);
                if ui.add(egui::Button::new(text).min_size(egui::vec2(320.0, 34.0))).clicked() {
                    open = Some(p.clone());
                }
            }
            if ui.add(egui::Button::new(RichText::new("+  New host").size(13.0)).min_size(egui::vec2(320.0, 34.0)).fill(theme::accent_soft())).clicked() {
                self.open_editor_new(None);
            }
            if ui.add(egui::Button::new(RichText::new("⬇  Import from Bitvise / SSH config").size(13.0)).min_size(egui::vec2(320.0, 34.0))).clicked() {
                self.import = Some(ImportDialog::new(&self.store));
            }
            ui.add_space(18.0);
            ui.label(RichText::new("Drag tabs to split · drop files onto a remote pane to upload").size(11.5).color(theme::text_faint()));
            if let Some(p) = open {
                self.request_open(p, OpenKind::Terminal, ui.ctx());
            }
            if let Some(s) = shell {
                self.open_local(Some(s), ui.ctx());
            }
        });
    }

    fn modals(&mut self, ctx: &egui::Context) {
        self.editor_modal(ctx);
        self.key_manager_modal(ctx);
        self.import_modal(ctx);
        self.prompt_modal(ctx);
        self.delete_modal(ctx);
    }

    fn import_modal(&mut self, ctx: &egui::Context) {
        let Some(dialog) = &mut self.import else { return };
        match dialog.show(ctx, &self.store) {
            ImportOutcome::Open => {}
            ImportOutcome::Cancel => self.import = None,
            ImportOutcome::Import(profiles) => {
                let n = profiles.len();
                for p in profiles {
                    self.store.upsert(p);
                }
                // Importers tag hosts with a group ("Bitvise", "SSH config"): file them.
                if self.folders.migrate_groups(&mut self.store.profiles) {
                    let _ = self.folders.save();
                }
                match self.store.save() {
                    Ok(()) => self.toasts.success(format!("Imported {n} host{}", if n == 1 { "" } else { "s" })),
                    Err(e) => self.toasts.error(format!("Could not save profiles: {e:#}")),
                }
                self.import = None;
            }
        }
    }

    fn prompt_modal(&mut self, ctx: &egui::Context) {
        let Some(prompt) = &mut self.prompt else { return };
        let (mut submit, mut cancel) = (false, false);
        let heading = match &prompt.profile.auth {
            AuthMethod::Managed { key } => {
                let name = self.keys.get(*key).map(|k| k.name.clone()).unwrap_or_else(|| "key".into());
                format!("Passphrase for “{name}”")
            }
            _ => format!("Password for {}", prompt.profile.address()),
        };
        let modal = egui::Modal::new(egui::Id::new("pw_prompt")).show(ctx, |ui| {
            ui.set_width(340.0);
            ui.heading(heading);
            ui.add_space(8.0);
            let r = ui.add(egui::TextEdit::singleline(&mut prompt.secret).password(true).desired_width(f32::INFINITY));
            r.request_focus();
            submit |= r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
            ui.checkbox(&mut prompt.remember, "Remember in Windows Credential Manager");
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                submit |= ui.add(egui::Button::new("Connect").fill(theme::accent_soft())).clicked();
                cancel |= ui.button("Cancel").clicked();
            });
        });
        if modal.should_close() || cancel {
            self.prompt = None;
            return;
        }
        if !submit {
            return;
        }
        let Some(p) = self.prompt.take() else { return };
        if p.remember
            && let Err(e) = secret::set(p.profile.secret_id(), &p.secret)
        {
            self.toasts.error(format!("{e:#}"));
        }
        match p.action {
            PromptAction::Open(kind) => self.open_now(p.profile, kind, Some(p.secret), ctx),
            PromptAction::Retry(uid) => self.reconnect(uid, Some(p.secret), ctx),
        }
    }

    fn delete_modal(&mut self, ctx: &egui::Context) {
        let Some(id) = self.confirm_delete else { return };
        let name = self.store.get(id).map(|p| p.display_name().to_string()).unwrap_or_default();
        let mut close = false;
        let modal = egui::Modal::new(egui::Id::new("del_host")).show(ctx, |ui| {
            ui.set_width(300.0);
            ui.heading(format!("Delete “{name}”?"));
            ui.label(RichText::new("The saved password is removed too. Archive it instead to keep it out of the way.").color(theme::text_dim()));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.add(egui::Button::new(RichText::new("Delete").color(egui::Color32::WHITE)).fill(theme::danger().gamma_multiply(0.7))).clicked() {
                    self.store.remove(id);
                    let _ = self.store.save();
                    close = true;
                }
                close |= ui.button("Cancel").clicked();
            });
        });
        if close || modal.should_close() {
            self.confirm_delete = None;
        }
    }
}

impl eframe::App for TetherApp {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        // Panels cover the window; transparent so window opacity can reach the desktop.
        [0.0; 4]
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        crate::themes::follow_system(&ctx);
        self.handle_shortcuts(&ctx);
        self.top_bar(ui);
        self.side_panel(ui);
        {
            let mut docks: Vec<&mut DockState<Tab>> = std::iter::once(&mut self.dock).chain(self.detached.docks_mut()).collect();
            link_terminals(&mut docks, self.focused_terminal);
        }
        self.workspace_area(ui);
        self.detached_windows(&ctx);
        self.modals(&ctx);
        if let Some(view) = &mut self.settings_view
            && !view.show(&ctx)
        {
            self.settings_view = None;
        }
        self.track_layout();
        self.toasts.show(&ctx);
        if self.broadcast {
            broadcast_frame(&ctx);
        }
    }
}

/// A terminal tab as seen by [`link_terminals`].
struct TermRef {
    uid: u64,
    dock: usize,
    leaf: egui_dock::NodePath,
    /// The active tab of its pane (visible).
    visible: bool,
    profile: Uuid,
    handle: TerminalHandle,
}

/// Gives each SFTP pane a terminal of the same host, which enables its "go to terminal
/// folder" button: the last focused one, else one visible in another pane (or window),
/// else any terminal of that host.
fn link_terminals(docks: &mut [&mut DockState<Tab>], focused: Option<u64>) {
    let mut terms = Vec::new();
    for (d, dock) in docks.iter().enumerate() {
        for (path, leaf) in dock.iter_leaves() {
            for (i, tab) in leaf.tabs.iter().enumerate() {
                if let TabBody::Terminal(t) = &tab.body
                    && !tab.is_local()
                {
                    terms.push(TermRef { uid: tab.uid, dock: d, leaf: path, visible: i == leaf.active.0, profile: tab.profile.id, handle: t.handle.clone() });
                }
            }
        }
    }
    for (d, dock) in docks.iter_mut().enumerate() {
        for (path, leaf) in dock.iter_leaves_mut() {
            for tab in leaf.tabs.iter_mut() {
                if let TabBody::Sftp(view) = &mut tab.body {
                    let same_host = |t: &&TermRef| t.profile == tab.profile.id;
                    let elsewhere = |t: &&TermRef| t.dock != d || t.leaf != path;
                    let linked = terms
                        .iter()
                        .filter(same_host)
                        .max_by_key(|t| (Some(t.uid) == focused && t.visible, t.visible && elsewhere(t), elsewhere(t)))
                        .map(|t| t.handle.clone());
                    view.set_linked_terminal(linked);
                }
            }
        }
    }
}

/// Red hairline around the window while broadcast mode is on.
fn broadcast_frame(ctx: &egui::Context) {
    let rect = ctx.content_rect().shrink(1.0);
    let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("bc_frame")));
    let pulse = ((ctx.input(|i| i.time) * 3.0).sin() * 0.5 + 0.5) as f32;
    painter.rect_stroke(rect, 0.0, egui::Stroke::new(1.5, theme::danger().gamma_multiply(0.4 + 0.4 * pulse)), egui::StrokeKind::Inside);
    ctx.request_repaint_after(std::time::Duration::from_millis(33));
}

fn is_no_such_file(err: &anyhow::Error) -> bool {
    err.chain().any(|c| {
        matches!(
            c.downcast_ref::<russh_sftp::client::error::Error>(),
            Some(russh_sftp::client::error::Error::Status(s)) if s.status_code == russh_sftp::protocol::StatusCode::NoSuchFile
        )
    })
}

