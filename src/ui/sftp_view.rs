//! SFTP browser: remote pane (plus an optional local pane) with drag-and-drop, Explorer
//! drops, per-host path history, live path suggestions, browser-style back / forward,
//! a per-host listing cache (stale-while-revalidate), a transfer queue, and opening
//! remote files in the built-in editor or the PC's default app (`sftp_open.rs`).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::Result;
use eframe::egui::{self, CornerRadius, Id, PointerButton, Rect, Sense, Stroke, Ui, Vec2};

#[path = "sftp_dialogs.rs"]
mod dialogs;
#[path = "sftp_open.rs"]
mod open;
#[path = "sftp_remote.rs"]
mod remote;
#[path = "sftp_session.rs"]
mod session;

use super::dir_cache::{self, SharedHostCache};
use super::file_list::{DragPayload, ListAction, ListState, Row, Side};
use super::loading::{self, ConnectInfo};
use super::nav_history::NavHistory;
use super::path_complete;
use super::path_field::{PathEvent, PathField, Suggest};
use super::sftp_chrome::{drop_overlay, icon_button, last_active_id, listing, loading_bar, nav_buttons, pane_title, pill, terminal_jump_button, toolbar};
use super::sftp_data::{self as data, HistoryRecorder, cached, candidates, learn, list_local, list_rows, set};
use super::sftp_status::{self as status, StatusAction};
use super::toast::Toasts;
use super::transfers_view;
use crate::keybindings::{self, Action};
use crate::profile::{Profile, unix_now};
use crate::remote_edit::Watcher;
use crate::settings;
use crate::sftp_history::{self, KnownDirs};
use crate::ssh::connection::Stage;
use crate::ssh::pool::Pool;
use crate::ssh::sftp::{self, Sftp};
use crate::ssh::terminal::TerminalHandle;
use crate::ssh::transfer::{self, Direction, Progress, Transfer, TransferState};
use crate::theme;
use dialogs::Dialog;
use session::{Link, RemoteState, Session};

const HEADER_H: f32 = 22.0;
const GAP: f32 = 8.0;
const PANE_ANIM: f32 = 0.22;
/// Room on the path row for the toolbar icons (and the upload button).
const PATH_ROW_RESERVE: f32 = 96.0;
const EXTRA_ICON_W: f32 = 28.0;
/// Panes narrower than this (mid-animation) are not drawn.
const MIN_PANE_W: f32 = 24.0;

/// Distinguishes several SFTP tabs (even of the same host) for ids and drop routing.
static NEXT_VIEW: AtomicU64 = AtomicU64::new(1);

/// How a remote navigation moves through the back / forward history.
enum NavOp {
    Visit(String),
    Back,
    Forward,
    Reload,
}

/// A pane's rect and its fade (0..1) during the show/hide animation.
type PaneSlot = (Rect, f32);

/// Directory listing fetched for Tab completion: (dir, entries or failure).
type CompletionSlot = Arc<Mutex<Option<(String, Option<Vec<path_complete::Candidate>>)>>>;

/// A finished background listing of the local pane: (generation, rows or error).
type LocalSlot = Arc<Mutex<Option<(u64, Result<Vec<Row>, String>)>>>;

/// Where a local navigation still loading came from: restored if it fails.
struct LocalUndo {
    cwd: PathBuf,
    nav: NavHistory<PathBuf>,
}

pub struct SftpView {
    pub profile: Profile,
    view_n: u64,
    remote: Arc<Mutex<RemoteState>>,
    cache: SharedHostCache,
    /// Directories being prefetched for suggestions (never fetched twice at once).
    prefetching: Arc<Mutex<HashSet<String>>>,
    local_cwd: PathBuf,
    local_rows: Vec<Row>,
    local_error: Option<String>,
    local_dirty: Arc<AtomicBool>,
    /// Local listings run on a blocking thread (big or slow folders never stall a frame).
    local_slot: LocalSlot,
    local_gen: u64,
    local_loading: bool,
    local_stale: bool,
    local_undo: Option<LocalUndo>,
    local_nav: NavHistory<PathBuf>,
    /// Local folders visited this session (suggestions); `local_version` bumps on change.
    local_known: KnownDirs,
    local_version: u64,
    local_list: ListState,
    remote_list: ListState,
    local_path: PathField,
    remote_path: PathField,
    show_local: bool,
    show_remote: bool,
    history: Arc<Mutex<Vec<String>>>,
    history_file: PathBuf,
    completion: CompletionSlot,
    transfers: Arc<Mutex<Vec<Transfer>>>,
    next_transfer: u64,
    dialog: Option<Dialog>,
    /// Terminal of the same host shown next to this pane (set by the app each frame).
    linked_terminal: Option<TerminalHandle>,
    /// Set by `connect`: what reconnects and "go to terminal folder" need.
    session: Arc<Mutex<Option<Session>>>,
    /// Result of a pending terminal-folder lookup, consumed on the UI thread.
    jump: Arc<Mutex<Option<String>>>,
    jumping: Arc<AtomicBool>,
    /// Files opened with the PC's default app, uploaded on every save.
    watcher: Watcher,
    /// A background "open in editor" that was refused: (file name, reason).
    open_refused: Arc<Mutex<Option<(String, String)>>>,
    /// Editor downloads in flight (drives the loading bar).
    opening: Arc<AtomicUsize>,
    /// UI time the current connect started (drives the connecting screen).
    connecting_since: Option<f64>,
    rt: tokio::runtime::Handle,
    ctx: egui::Context,
}

impl SftpView {
    pub fn new(profile: Profile, rt: tokio::runtime::Handle, ctx: egui::Context) -> Self {
        let home = directories::UserDirs::new().map(|d| d.home_dir().to_path_buf()).unwrap_or_default();
        let view_n = NEXT_VIEW.fetch_add(1, Ordering::Relaxed);
        let history_file = sftp_history::default_path();
        Self {
            remote: Arc::new(Mutex::new(RemoteState::default())),
            cache: dir_cache::host_cache(profile.id),
            prefetching: Arc::default(),
            local_cwd: home,
            local_rows: Vec::new(),
            local_error: None,
            // The first frame lists the local folder in the background.
            local_dirty: Arc::new(AtomicBool::new(true)),
            local_slot: LocalSlot::default(),
            local_gen: 0,
            local_loading: false,
            local_stale: false,
            local_undo: None,
            local_nav: NavHistory::default(),
            local_known: KnownDirs::default(),
            local_version: 0,
            local_list: ListState::default(),
            remote_list: ListState::default(),
            local_path: PathField::new(Id::new(("sftp_path", "local", view_n)), path_complete::LOCAL),
            remote_path: PathField::new(Id::new(("sftp_path", "remote", view_n)), path_complete::REMOTE),
            show_local: settings::get().sftp.start_with_local_pane,
            show_remote: true,
            // Loaded by `connect`, off the UI thread.
            history: Arc::default(),
            history_file,
            completion: CompletionSlot::default(),
            transfers: Arc::default(),
            next_transfer: 0,
            dialog: None,
            linked_terminal: None,
            session: Arc::default(),
            jump: Arc::default(),
            jumping: Arc::default(),
            watcher: Watcher::default(),
            open_refused: Arc::default(),
            opening: Arc::default(),
            connecting_since: None,
            rt,
            ctx,
            view_n,
            profile,
        }
    }

    pub fn connect(&self, pool: Pool, secret: Option<String>, toasts: Toasts) {
        let session = Session {
            pool,
            toasts,
            profile: self.profile.clone(),
            state: self.remote.clone(),
            cache: self.cache.clone(),
            history: self.history.clone(),
            recorder: Arc::new(self.recorder()),
            ctx: self.ctx.clone(),
            rt: self.rt.clone(),
        };
        session.start(secret);
        set(&self.session, |s| *s = Some(session));
    }

    /// The app links a visible terminal of the same host; `None` hides the jump button.
    pub fn set_linked_terminal(&mut self, terminal: Option<TerminalHandle>) {
        self.linked_terminal = terminal;
    }

    pub fn is_ready(&self) -> bool {
        self.remote.lock().map(|s| matches!(s.link, Link::Ready { .. })).unwrap_or(false)
    }

    fn sftp(&self) -> Option<Arc<Sftp>> {
        match &self.remote.lock().ok()?.link {
            Link::Ready { sftp, .. } => Some(sftp.clone()),
            _ => None,
        }
    }

    fn remote_cwd(&self) -> String {
        self.remote.lock().map(|s| s.cwd.clone()).unwrap_or_default()
    }

    fn base_id(&self) -> Id {
        Id::new(("sftp_view", self.view_n))
    }

    fn recorder(&self) -> HistoryRecorder {
        HistoryRecorder { file: self.history_file.clone(), host: self.profile.id, list: self.history.clone() }
    }

    // ---- local ops -------------------------------------------------------------

    /// Re-lists the current local folder in the background.
    fn refresh_local(&mut self) {
        self.local_dirty.store(false, Ordering::Relaxed);
        self.load_local(self.local_cwd.clone());
    }

    /// Lists `path` on a blocking thread; [`Self::poll_local`] applies the result.
    fn load_local(&mut self, path: PathBuf) {
        self.local_gen += 1;
        self.local_loading = true;
        let (generation, slot, ctx) = (self.local_gen, self.local_slot.clone(), self.ctx.clone());
        self.rt.spawn_blocking(move || {
            let listed = list_local(&path).map_err(|e| e.to_string());
            // Listings can finish out of order: never replace a newer result.
            set(&slot, |s| {
                if s.as_ref().is_none_or(|(g, _)| *g < generation) {
                    *s = Some((generation, listed));
                }
            });
            ctx.request_repaint();
        });
    }

    /// Applies a finished local listing; a failed navigation restores the previous folder.
    fn poll_local(&mut self) {
        let Some((generation, listed)) = self.local_slot.lock().ok().and_then(|mut s| s.take()) else { return };
        if generation != self.local_gen {
            return;
        }
        self.local_loading = false;
        self.local_stale = false;
        let undo = self.local_undo.take();
        match listed {
            Ok(rows) => {
                self.local_rows = rows;
                self.local_error = None;
                if undo.is_some() && !self.local_cwd.as_os_str().is_empty() {
                    self.local_known.visit(&self.local_cwd.display().to_string(), unix_now());
                    self.local_version += 1;
                }
            }
            Err(e) => {
                if let Some(u) = undo {
                    self.local_cwd = u.cwd;
                    self.local_nav = u.nav;
                }
                self.local_error = Some(e);
            }
        }
    }

    /// Shows `path` at once (the old rows dimmed) and lists it in the background.
    /// `nav_before` comes back together with the folder if the listing fails.
    fn local_switch(&mut self, path: PathBuf, nav_before: NavHistory<PathBuf>) {
        let previous = std::mem::replace(&mut self.local_cwd, path.clone());
        self.local_list.clear();
        self.local_stale = !self.local_rows.is_empty();
        // Quick successive navigations keep the oldest folder as the one to go back to.
        let undo = self.local_undo.take().unwrap_or(LocalUndo { cwd: previous, nav: nav_before });
        self.local_undo = Some(undo);
        self.load_local(path);
    }

    fn local_navigate(&mut self, path: PathBuf) {
        let path = if path.as_os_str().is_empty() || path.is_absolute() { path } else { self.local_cwd.join(path) };
        let before = self.local_nav.clone();
        self.local_nav.visit(self.local_cwd.clone(), &path);
        self.local_switch(path, before);
    }

    /// Back (`back`) or forward through the local pane's history.
    fn local_step(&mut self, back: bool) {
        let before = self.local_nav.clone();
        let current = self.local_cwd.clone();
        let target = if back { self.local_nav.back(current) } else { self.local_nav.forward(current) };
        if let Some(t) = target {
            self.local_switch(t, before);
        }
    }

    fn step(&mut self, side: Side, back: bool) {
        match side {
            Side::Local => self.local_step(back),
            Side::Remote => self.remote_go(if back { NavOp::Back } else { NavOp::Forward }),
        }
    }

    fn local_op(&mut self, op: impl FnOnce() -> std::io::Result<()>) {
        if let Err(e) = op() {
            self.local_error = Some(e.to_string());
            return;
        }
        self.refresh_local();
    }

    // ---- transfers -------------------------------------------------------------

    fn start_transfer(&mut self, direction: Direction, names: Vec<String>) {
        let Some(sftp) = self.sftp() else { return };
        if names.is_empty() {
            return;
        }
        // Without a visible local pane there is no implied destination: ask for one.
        if direction == Direction::Download && !self.show_local {
            let Some(dir) = rfd::FileDialog::new().set_directory(&self.local_cwd).pick_folder() else { return };
            self.local_navigate(dir);
        }
        let remote_cwd = self.remote_cwd();
        for name in names {
            if direction == Direction::Download && !transfer::is_safe_local_name(&name) {
                self.local_error = Some(format!("Refusing unsafe remote file name {name:?}"));
                continue;
            }
            let (local, remote) = (self.local_cwd.join(&name), sftp::join(&remote_cwd, &name));
            self.spawn_transfer(sftp.clone(), direction, local, remote, name);
        }
    }

    fn upload_paths(&mut self, paths: Vec<PathBuf>) {
        let Some(sftp) = self.sftp() else { return };
        let remote_cwd = self.remote_cwd();
        for path in paths {
            let Some(name) = path.file_name().map(|n| n.to_string_lossy().into_owned()) else { continue };
            let remote = sftp::join(&remote_cwd, &name);
            self.spawn_transfer(sftp.clone(), Direction::Upload, path, remote, name);
        }
    }

    fn spawn_transfer(&mut self, sftp: Arc<Sftp>, direction: Direction, local: PathBuf, remote: String, label: String) {
        self.next_transfer += 1;
        let progress = Arc::new(Progress::default());
        let state = Arc::new(Mutex::new(TransferState::Running));
        let finished = Arc::new(Mutex::new(None));
        if let Ok(mut list) = self.transfers.lock() {
            list.push(Transfer {
                id: self.next_transfer,
                direction,
                label,
                progress: progress.clone(),
                state: state.clone(),
                started: Instant::now(),
                finished: finished.clone(),
            });
        }
        let (remote_state, local_dirty, ctx, pipeline) = (self.remote.clone(), self.local_dirty.clone(), self.ctx.clone(), self.profile.pipeline_kb);
        let (cache, host) = (self.cache.clone(), self.profile.id);
        let (remote_cwd, session) = (self.remote_cwd(), self.session());
        self.rt.spawn(async move {
            let result = transfer::run(sftp.clone(), direction, local, remote, pipeline, progress.clone()).await;
            let final_state = match result {
                Ok(()) => TransferState::Done,
                Err(_) if progress.is_cancelled() => TransferState::Cancelled,
                Err(e) => match Self::triage(session.as_ref(), &sftp, e) {
                    Some(msg) => TransferState::Failed(msg),
                    None => TransferState::Failed("Connection lost — send it again once reconnected".into()),
                },
            };
            set(&state, |s| *s = final_state);
            set(&finished, |f| *f = Some(Instant::now()));
            match direction {
                Direction::Download => local_dirty.store(true, Ordering::Relaxed),
                Direction::Upload => {
                    if let Ok(rows) = list_rows(&sftp, &remote_cwd).await {
                        learn(&cache, host, &remote_cwd, &rows, false);
                        set(&remote_state, |s| {
                            if s.cwd == remote_cwd {
                                s.rows = rows
                            }
                        });
                    }
                }
            }
            ctx.request_repaint();
        });
    }

    // ---- UI ----------------------------------------------------------------------

    /// Draws the view; the returned action asks the app to reconnect or close the tab.
    pub fn ui(&mut self, ui: &mut Ui) -> StatusAction {
        self.poll_open();
        self.poll_local();
        if let Some(path) = self.jump.lock().ok().and_then(|mut j| j.take()) {
            self.remote_navigate(path);
        }
        if self.local_dirty.load(Ordering::Relaxed) {
            self.refresh_local();
        }
        let mut action = StatusAction::default();
        let base = self.base_id();
        let has_transfers = self.transfers.lock().map(|t| !t.is_empty()).unwrap_or(false);
        let t = ui.ctx().animate_bool_with_time(base.with("tx_panel"), has_transfers, 0.25);
        let transfers_h = 150.0 * t;
        let full = ui.available_rect_before_wrap().shrink(8.0);
        if ui.rect_contains_pointer(full) {
            ui.ctx().data_mut(|d| d.insert_temp(last_active_id(), self.view_n));
        }

        let header = Rect::from_min_size(full.min, Vec2::new(full.width(), HEADER_H));
        self.header(ui, header);
        let top = header.bottom() + 6.0;
        let panes_h = full.bottom() - top - transfers_h - if t > 0.0 { GAP } else { 0.0 };
        let (local_rect, remote_rect) = self.pane_rects(ui.ctx(), Rect::from_min_size(egui::pos2(full.left(), top), Vec2::new(full.width(), panes_h)));

        if let Some((rect, fade)) = local_rect {
            self.pane(ui, rect, Side::Local, fade);
        }
        if let Some((rect, fade)) = remote_rect {
            self.pane(ui, rect, Side::Remote, fade);
            self.connecting_screen(ui, rect);
            action = self.status_overlay(ui, rect);
        }

        if t > 0.0 {
            let rect = egui::Rect::from_min_size(egui::pos2(full.left(), top + panes_h + GAP), Vec2::new(full.width(), transfers_h));
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
            child.set_opacity(t);
            if let Ok(mut list) = self.transfers.lock() {
                transfers_view::show(&mut child, &mut list);
            }
        }
        self.dialogs(ui.ctx());
        ui.allocate_rect(full, Sense::hover());
        action
    }

    /// The reconnect banner or the failure card over the remote pane.
    fn status_overlay(&mut self, ui: &mut Ui, rect: Rect) -> StatusAction {
        let base = self.base_id();
        let (banner, failure) = self
            .remote
            .lock()
            .map(|s| match &s.link {
                Link::Reconnecting { attempt } => (Some((status::reconnect_text(*attempt, session::MAX_ATTEMPTS), s.stage)), None),
                // A manual reconnect over a listing keeps it visible under the banner.
                Link::Connecting if !s.rows.is_empty() => (Some(("Reconnecting…".to_string(), s.stage)), None),
                Link::Failed(f) => (None, Some(f.clone())),
                _ => (None, None),
            })
            .unwrap_or_default();
        let banner_t = ui.ctx().animate_bool_with_time(base.with("banner"), banner.is_some(), theme::anim_time(0.25));
        let card_t = ui.ctx().animate_bool_with_time(base.with("failure"), failure.is_some(), theme::anim_time(0.25));
        let mut action = StatusAction::default();
        if let Some((text, stage)) = &banner {
            action = status::reconnect_banner(ui, rect, text, &format!("{}…", stage.label()), banner_t);
        }
        if let Some(f) = &failure {
            // The next connect starts its own "connecting" timer.
            self.connecting_since = None;
            action = status::failure_card(ui, rect, f.title, &f.message, &f.detail, card_t);
        }
        action
    }

    /// Covers the remote pane while the session connects (only if that takes a moment),
    /// then fades away onto the listing.
    fn connecting_screen(&mut self, ui: &Ui, rect: Rect) {
        // With a listing on screen (manual reconnect) the slim banner shows progress instead.
        let (connecting, stage) =
            self.remote.lock().map(|s| (matches!(s.link, Link::Connecting) && s.rows.is_empty(), s.stage)).unwrap_or((false, Stage::Connecting));
        let now = ui.input(|i| i.time);
        let since = if connecting { *self.connecting_since.get_or_insert(now) } else { self.connecting_since.take().unwrap_or(now) };
        let elapsed = now - since;
        let show = connecting && elapsed > loading::SHOW_AFTER_SECS;
        if connecting && !show {
            ui.ctx().request_repaint();
        }
        let t = ui.ctx().animate_bool_with_time(self.base_id().with("connecting"), show, theme::anim_time(0.3));
        if t <= 0.0 {
            return;
        }
        ui.painter().rect_filled(rect, CornerRadius::same(theme::RADIUS), theme::surface().gamma_multiply(t));
        let address = self.profile.address();
        let info = ConnectInfo { name: self.profile.display_name(), address: &address, hue: self.profile.hue, stage, stages: loading::SFTP_STAGES, elapsed };
        loading::connect_screen(ui, rect, &info, t);
    }

    /// Slim bar with the host name and the Local / Remote pane toggles.
    fn header(&mut self, ui: &mut Ui, rect: Rect) {
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(egui::Layout::left_to_right(egui::Align::Center)));
        child.spacing_mut().item_spacing.x = 6.0;
        child.add_space(2.0);
        child.label(egui::RichText::new(self.profile.display_name()).size(12.0).strong().color(theme::text()));
        child.label(egui::RichText::new(self.profile.address()).size(11.0).color(theme::text_faint()));
        child.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let only_remote = self.show_remote && !self.show_local;
            let only_local = self.show_local && !self.show_remote;
            if pill(ui, "Remote", self.show_remote, only_remote, theme::blue()).clicked() && !only_remote {
                self.show_remote = !self.show_remote;
            }
            if pill(ui, "Local", self.show_local, only_local, theme::warn()).clicked() && !only_local {
                self.show_local = !self.show_local;
            }
            self.watch_chip(ui);
        });
    }

    /// Animated split of `body` between the visible panes: (rect, fade) per side.
    fn pane_rects(&self, ctx: &egui::Context, body: Rect) -> (Option<PaneSlot>, Option<PaneSlot>) {
        let base = self.base_id();
        let ease = egui::emath::easing::cubic_in_out;
        let lt = ease(ctx.animate_value_with_time(base.with("local_t"), f32::from(u8::from(self.show_local)), PANE_ANIM));
        let rt = ease(ctx.animate_value_with_time(base.with("remote_t"), f32::from(u8::from(self.show_remote)), PANE_ANIM));
        let gap = GAP * lt.min(rt);
        let usable = body.width() - gap;
        let local_w = usable * lt / (lt + rt).max(f32::EPSILON);
        let remote_w = usable - local_w;
        let local = Rect::from_min_size(body.min, Vec2::new(local_w, body.height()));
        let remote = Rect::from_min_size(body.min + Vec2::new(local_w + gap, 0.0), Vec2::new(remote_w, body.height()));
        ((local_w >= MIN_PANE_W).then_some((local, lt)), (remote_w >= MIN_PANE_W).then_some((remote, rt)))
    }

    /// Whether files dropped from Explorer should land in this view's `rect`. Windows does
    /// not report the pointer during an OLE drag, so with no pointer the view the user last
    /// hovered takes the drop (never two SFTP tabs at once).
    fn takes_os_drop(&self, ui: &Ui, rect: Rect) -> bool {
        match ui.input(|i| i.pointer.hover_pos()) {
            Some(_) => ui.rect_contains_pointer(rect),
            None => ui.ctx().data(|d| d.get_temp::<u64>(last_active_id())) == Some(self.view_n),
        }
    }

    /// Mouse back / forward buttons and Alt+Left / Alt+Right over a pane.
    fn nav_input(&mut self, ui: &Ui, rect: Rect, side: Side) {
        if self.dialog.is_some() || !ui.rect_contains_pointer(rect) {
            return;
        }
        // Only a text field blocks these (the file list keeps focus after a click).
        let typing = ui.ctx().egui_wants_keyboard_input();
        let (back, forward, up, refresh) = ui.ctx().input(|i| {
            let key = |a: Action| !typing && keybindings::pressed_in(i, a);
            (
                i.pointer.button_pressed(PointerButton::Extra1) || key(Action::SftpBack),
                i.pointer.button_pressed(PointerButton::Extra2) || key(Action::SftpForward),
                key(Action::SftpUp),
                key(Action::SftpRefresh),
            )
        });
        if back || forward {
            self.step(side, back);
        }
        if up {
            self.apply(side, ListAction::Up, ui.ctx());
        }
        if refresh {
            self.apply(side, ListAction::Refresh, ui.ctx());
        }
    }

    fn pane(&mut self, ui: &mut Ui, rect: egui::Rect, side: Side, fade: f32) {
        self.nav_input(ui, rect, side);
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect).id_salt((self.view_n, side as u8)));
        child.set_clip_rect(rect.intersect(ui.clip_rect()));
        child.set_opacity(fade);
        let frame = egui::Frame::new().fill(theme::surface()).corner_radius(theme::RADIUS).inner_margin(8.0);
        let pane_resp = frame
            .show(&mut child, |ui| {
                ui.set_min_size(rect.size() - Vec2::splat(16.0));
                let actions = match side {
                    Side::Local => self.local_pane(ui),
                    Side::Remote => self.remote_pane(ui),
                };
                for a in actions {
                    self.apply(side, a, ui.ctx());
                }
            })
            .response;
        let pane_resp = ui.interact(pane_resp.rect, self.base_id().with(("pane", side as u8)), Sense::hover());

        let os_hover = side == Side::Remote && ui.input(|i| !i.raw.hovered_files.is_empty()) && self.takes_os_drop(ui, rect);
        let row_hover = pane_resp.dnd_hover_payload::<DragPayload>().filter(|p| p.from != side).is_some();
        let glow = ui.ctx().animate_bool_with_time(pane_resp.id.with("glow"), row_hover || os_hover, 0.15);
        if glow > 0.0 {
            ui.painter().rect_stroke(rect, CornerRadius::same(theme::RADIUS), Stroke::new(1.5, theme::accent().gamma_multiply(glow)), egui::StrokeKind::Inside);
        }
        let overlay = ui.ctx().animate_bool_with_time(pane_resp.id.with("drop"), os_hover, 0.15);
        if overlay > 0.0 {
            drop_overlay(ui, rect, &self.remote_cwd(), overlay);
        }
        if let Some(payload) = pane_resp.dnd_release_payload::<DragPayload>()
            && payload.from != side
        {
            let dir = if side == Side::Remote { Direction::Upload } else { Direction::Download };
            self.start_transfer(dir, payload.names.clone());
        }
        if side == Side::Remote && self.takes_os_drop(ui, rect) {
            let dropped: Vec<PathBuf> =
                ui.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).filter(|p| !p.as_os_str().is_empty()).collect());
            if !dropped.is_empty() {
                self.upload_paths(dropped);
            }
        }
    }

    fn local_pane(&mut self, ui: &mut Ui) -> Vec<ListAction> {
        let mut actions = Vec::new();
        let cwd = self.local_cwd.display().to_string();
        ui.horizontal(|ui| {
            pane_title(ui, "LOCAL", theme::warn());
            let (back, forward) = nav_buttons(ui, self.local_nav.can_back(), self.local_nav.can_forward());
            if back || forward {
                self.local_step(back);
            }
            let width = (ui.available_width() - PATH_ROW_RESERVE).max(80.0);
            let known = &self.local_known;
            let rank = |q: &str| data::rank_local(known, q);
            let events = self.local_path.show(ui, &cwd, None, width, Some(Suggest { version: self.local_version, rank: &rank }));
            for event in events {
                match event {
                    PathEvent::Navigate(p) => self.local_navigate(PathBuf::from(p)),
                    PathEvent::NeedListing(dir) => match list_local(Path::new(&dir)) {
                        Ok(rows) => self.local_path.provide_listing(dir, candidates(&rows)),
                        Err(_) => self.local_path.listing_failed(&dir),
                    },
                    // Local suggestions read the disk directly; nothing to prefetch.
                    PathEvent::Prefetch(_) => {}
                }
            }
            toolbar(ui, &mut actions, "Upload selected", "⬆", !self.local_list.selected.is_empty());
        });
        loading_bar(ui, self.base_id().with("local_loading"), self.local_loading);
        if let Some(e) = &self.local_error {
            ui.colored_label(theme::danger(), e);
        }
        let state = (self.local_stale, self.local_loading);
        actions.extend(listing(ui, self.base_id().with("local_rows"), Side::Local, &self.local_rows, &mut self.local_list, "⬆  Upload", state));
        actions
    }

    fn remote_pane(&mut self, ui: &mut Ui) -> Vec<ListAction> {
        let mut actions = Vec::new();
        let (rows, loading, error, cwd, can_back, can_forward, stale) = self
            .remote
            .lock()
            .map(|s| (s.rows.clone(), s.loading, s.error.clone(), s.cwd.clone(), s.nav.can_back(), s.nav.can_forward(), s.stale))
            .unwrap_or_default();
        if let Some((dir, listed)) = self.completion.lock().ok().and_then(|mut s| s.take()) {
            match listed {
                Some(c) => self.remote_path.provide_listing(dir, c),
                None => self.remote_path.listing_failed(&dir),
            }
        }
        let history = self.history.lock().map(|h| h.clone()).unwrap_or_default();
        ui.horizontal(|ui| {
            pane_title(ui, "REMOTE", theme::blue());
            let (back, forward) = nav_buttons(ui, can_back, can_forward);
            if back || forward {
                self.step(Side::Remote, back);
            }
            let extra = if self.show_local { 1.0 } else { 2.0 } * EXTRA_ICON_W;
            let width = (ui.available_width() - PATH_ROW_RESERVE - extra).max(80.0);
            let events = {
                let cache = self.cache.clone();
                let guard = cache.lock().ok();
                let version = guard.as_ref().map_or(0, |g| g.version);
                let rank = |q: &str| data::rank_remote(guard.as_deref(), &history, q);
                self.remote_path.show(ui, &cwd, Some(&history), width, Some(Suggest { version, rank: &rank }))
            };
            for event in events {
                match event {
                    PathEvent::Navigate(p) => self.remote_navigate(p),
                    PathEvent::NeedListing(dir) => self.remote_completion_listing(dir, &cwd, &rows),
                    PathEvent::Prefetch(dir) => self.remote_prefetch(dir),
                }
            }
            if self.linked_terminal.is_some() && terminal_jump_button(ui, self.jumping.load(Ordering::Relaxed)) {
                self.jump_to_terminal();
            }
            toolbar(ui, &mut actions, "Download selected", "⬇", !self.remote_list.selected.is_empty());
            if !self.show_local && icon_button(ui, "⬆", "Upload files…") {
                let picked = rfd::FileDialog::new().set_directory(&self.local_cwd).pick_files().unwrap_or_default();
                self.upload_paths(picked);
            }
        });
        loading_bar(ui, self.base_id().with("remote_loading"), loading || self.is_opening());
        if let Some(e) = error {
            ui.colored_label(theme::danger(), e);
        }
        let state = (stale, loading);
        actions.extend(listing(ui, self.base_id().with("remote_rows"), Side::Remote, &rows, &mut self.remote_list, "⬇  Download", state));
        actions
    }

    fn apply(&mut self, side: Side, action: ListAction, ctx: &egui::Context) {
        // Toolbar transfer buttons send an empty list meaning "current selection".
        let action = match action {
            ListAction::Transfer(n) if n.is_empty() => ListAction::Transfer(match side {
                Side::Local => self.local_list.selected.iter().cloned().collect(),
                Side::Remote => self.remote_list.selected.iter().cloned().collect(),
            }),
            other => other,
        };
        match (side, action) {
            (Side::Local, ListAction::Open(n)) => self.local_navigate(self.local_cwd.join(n)),
            (Side::Remote, ListAction::Open(n)) => self.remote_navigate(sftp::join(&self.remote_cwd(), &n)),
            (Side::Remote, ListAction::OpenFile(n)) => self.open_remote_file(n),
            (Side::Remote, ListAction::OpenInEditor(n)) => self.open_in_editor(n),
            (Side::Remote, ListAction::OpenWithSystem(n)) => self.open_with_system(n),
            // Local files are uploaded on double-click; the open-with entries are remote-only.
            (Side::Local, ListAction::OpenFile(n)) => self.start_transfer(Direction::Upload, vec![n]),
            (Side::Local, ListAction::OpenInEditor(_) | ListAction::OpenWithSystem(_)) => {}
            (_, ListAction::AlwaysOpenWith(ext, with)) => self.always_open_with(ext, with),
            (Side::Local, ListAction::Up) => self.local_navigate(self.local_cwd.parent().map(Path::to_path_buf).unwrap_or_default()),
            (Side::Remote, ListAction::Up) => self.remote_navigate(sftp::parent(&self.remote_cwd())),
            (Side::Local, ListAction::Transfer(n)) => self.start_transfer(Direction::Upload, n),
            (Side::Remote, ListAction::Transfer(n)) => self.start_transfer(Direction::Download, n),
            (Side::Local, ListAction::Refresh) => self.refresh_local(),
            (Side::Remote, ListAction::Refresh) => self.remote_go(NavOp::Reload),
            (_, ListAction::NewFolder) => self.dialog = Some(Dialog::NewFolder { side, name: String::new() }),
            (_, ListAction::Rename(old)) => self.dialog = Some(Dialog::Rename { side, new: old.clone(), old, selected: false }),
            (_, ListAction::Delete(names)) => self.dialog = Some(Dialog::Delete { side, names }),
            (Side::Local, ListAction::CopyPath(n)) => ctx.copy_text(self.local_cwd.join(n).display().to_string()),
            (Side::Remote, ListAction::CopyPath(n)) => ctx.copy_text(sftp::join(&self.remote_cwd(), &n)),
        }
    }
}

impl Drop for SftpView {
    /// Closing the pane stops its transfers instead of letting them fail mid-file later, and
    /// stops watching files opened with system apps (their temp copies are deleted).
    fn drop(&mut self) {
        // Stops the session watcher and any reconnect loop of this view.
        set(&self.remote, |s| s.epoch += 1);
        self.watcher.stop_all();
        if let Ok(list) = self.transfers.lock() {
            for t in list.iter() {
                t.progress.cancel();
            }
        }
    }
}
