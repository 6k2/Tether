//! SFTP browser: remote pane (plus an optional local pane) with drag-and-drop, Explorer
//! drops, per-host path history, live path suggestions, browser-style back / forward,
//! a per-host listing cache (stale-while-revalidate), a transfer queue, and opening
//! remote files in the built-in editor or the PC's default app (`sftp_open.rs`).

use std::collections::HashSet;
use std::future::Future;
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

use super::dir_cache::{self, SharedHostCache};
use super::file_list::{self, DragPayload, ListAction, ListState, ROW_H, Row, Side};
use super::loading::{self, ConnectInfo};
use super::nav_history::NavHistory;
use super::path_complete;
use super::path_field::{PathEvent, PathField, Suggest};
use super::sftp_chrome::{drop_overlay, failed_card, icon_button, loading_bar, nav_buttons, pane_title, pill, toolbar};
use super::sftp_data::{self as data, HistoryRecorder, cached, candidates, learn, list_local, list_rows, set};
use super::toast::Toasts;
use super::transfers_view;
use crate::keybindings::{self, Action};
use crate::profile::{Profile, secret, unix_now};
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

enum Link {
    Connecting,
    Ready(Arc<Sftp>),
    Failed(String),
}

struct RemoteState {
    link: Link,
    cwd: String,
    rows: Vec<Row>,
    loading: bool,
    error: Option<String>,
    /// Bumped per listing request; results from older requests are discarded.
    generation: u64,
    nav: NavHistory<String>,
    /// `rows` still belong to the previous folder while the new one loads (shown dimmed,
    /// never blanked, so navigating doesn't flash an empty list).
    stale: bool,
    /// Connect progress while `link` is `Connecting`.
    stage: Stage,
}

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
    /// Pool + toasts captured at connect, used for "go to terminal folder".
    services: Arc<Mutex<Option<(Pool, Toasts)>>>,
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
            remote: Arc::new(Mutex::new(RemoteState {
                link: Link::Connecting,
                cwd: String::new(),
                rows: Vec::new(),
                loading: true,
                error: None,
                generation: 0,
                nav: NavHistory::default(),
                stale: false,
                stage: Stage::Connecting,
            })),
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
            services: Arc::default(),
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
        set(&self.services, |s| *s = Some((pool.clone(), toasts.clone())));
        let (state, profile, ctx, cache) = (self.remote.clone(), self.profile.clone(), self.ctx.clone(), self.cache.clone());
        let recorder = self.recorder();
        let (history, history_file, host) = (self.history.clone(), self.history_file.clone(), self.profile.id);
        set(&state, |s| {
            s.link = Link::Connecting;
            s.loading = true;
            s.error = None;
            s.stage = Stage::Connecting;
        });
        self.rt.spawn(async move {
            let stage = {
                let (state, ctx) = (state.clone(), ctx.clone());
                move |st: Stage| {
                    set(&state, |s| s.stage = st);
                    ctx.request_repaint();
                }
            };
            let result = async {
                if history.lock().map(|h| h.is_empty()).unwrap_or(false) {
                    let loaded = sftp_history::load(&history_file, host);
                    set(&history, |h| *h = loaded);
                }
                let conn = pool.get_staged(&profile, secret, &stage).await?;
                if let Some(fp) = conn.learned_fingerprint.lock().ok().and_then(|mut f| f.take()) {
                    toasts.info(format!("New host key trusted for {}\n{fp}", profile.host));
                }
                stage(Stage::Opening);
                let sftp = Sftp::open(&conn).await?;
                stage(Stage::Loading);
                let resume_dir = settings::get().sftp.resume_last_dir;
                let last = if resume_dir { history.lock().ok().and_then(|h| h.first().cloned()) } else { None };
                let (cwd, rows) = match resume(&sftp, last.as_deref(), &cache, &state, &ctx).await {
                    Some(found) => found,
                    None => {
                        let start = if profile.remote_start_dir.trim().is_empty() { "." } else { profile.remote_start_dir.trim() };
                        let cwd = sftp.canonicalize(start).await?;
                        show_cached(&cache, &cwd, &state, &ctx);
                        let rows = list_rows(&sftp, &cwd).await?;
                        (cwd, rows)
                    }
                };
                anyhow::Ok((sftp, cwd, rows))
            }
            .await;
            if let Ok((_, cwd, rows)) = &result {
                learn(&cache, profile.id, cwd, rows, true);
                recorder.record(cwd);
            }
            set(&state, |s| match result {
                Ok((sftp, cwd, rows)) => {
                    s.link = Link::Ready(sftp);
                    s.cwd = cwd;
                    s.rows = rows;
                    s.loading = false;
                }
                Err(e) => {
                    s.link = Link::Failed(format!("{e:#}"));
                    s.loading = false;
                }
            });
            ctx.request_repaint();
        });
    }

    /// The app links a visible terminal of the same host; `None` hides the jump button.
    pub fn set_linked_terminal(&mut self, terminal: Option<TerminalHandle>) {
        self.linked_terminal = terminal;
    }

    /// Asks where the linked terminal is and navigates there (OSC 7 first, then /proc).
    fn jump_to_terminal(&self) {
        let Some(term) = self.linked_terminal.clone() else { return };
        if let Some(cwd) = term.shared.lock().ok().and_then(|s| s.parser.callbacks().cwd.clone()) {
            set(&self.jump, |j| *j = Some(cwd));
            return;
        }
        let Some((pool, toasts)) = self.services.lock().ok().and_then(|s| s.clone()) else { return };
        let (profile, jump, jumping, ctx) = (self.profile.clone(), self.jump.clone(), self.jumping.clone(), self.ctx.clone());
        let (age_secs, rows, cols) = term.probe_facts();
        jumping.store(true, Ordering::Relaxed);
        self.rt.spawn(async move {
            let found = async {
                // The terminal's transport is normally live; the stored secret covers a
                // dropped one instead of failing with "Password required".
                let conn = pool.get(&profile, secret::get(profile.secret_id())).await?;
                let probe = crate::ssh::cwd::Probe { tag: &term.tag, age_secs, rows, cols };
                crate::ssh::cwd::terminal_cwd(&conn, probe).await
            }
            .await;
            match found {
                Ok(path) => set(&jump, |j| *j = Some(path)),
                Err(e) => toasts.error(format!("{e:#}")),
            }
            jumping.store(false, Ordering::Relaxed);
            ctx.request_repaint();
        });
    }

    pub fn is_ready(&self) -> bool {
        self.remote.lock().map(|s| matches!(s.link, Link::Ready(_))).unwrap_or(false)
    }

    fn sftp(&self) -> Option<Arc<Sftp>> {
        match &self.remote.lock().ok()?.link {
            Link::Ready(s) => Some(s.clone()),
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

    // ---- remote ops ------------------------------------------------------------

    /// Runs an op against the server, then re-lists the current directory.
    fn remote_op<F, Fut>(&self, op: F)
    where
        F: FnOnce(Arc<Sftp>) -> Fut + Send + 'static,
        Fut: Future<Output = Result<()>> + Send,
    {
        let Some(sftp) = self.sftp() else { return };
        let (state, ctx, cache, host) = (self.remote.clone(), self.ctx.clone(), self.cache.clone(), self.profile.id);
        let (mut generation, mut cwd) = (0, String::new());
        set(&state, |s| {
            s.loading = true;
            s.generation += 1;
            generation = s.generation;
            cwd = s.cwd.clone();
        });
        self.rt.spawn(async move {
            let op_result = op(sftp.clone()).await;
            let listed = list_rows(&sftp, &cwd).await;
            if let Ok(rows) = &listed {
                learn(&cache, host, &cwd, rows, false);
            }
            set(&state, |s| {
                if s.generation != generation {
                    return;
                }
                s.loading = false;
                s.error = op_result.err().map(|e| format!("{e:#}"));
                s.stale = false;
                match listed {
                    Ok(rows) if s.cwd == cwd => s.rows = rows,
                    Ok(_) => {}
                    Err(e) => s.error = Some(format!("{e:#}")),
                }
            });
            ctx.request_repaint();
        });
    }

    /// Navigates at once (cached rows, or an empty list, under the new path) and loads the
    /// fresh listing in the background; a failure restores the previous folder and history.
    fn remote_go(&mut self, op: NavOp) {
        let Some(sftp) = self.sftp() else { return };
        let visited = !matches!(op, NavOp::Reload);
        let cache = self.cache.clone();
        let mut plan = None;
        set(&self.remote, |s| {
            let before = (s.cwd.clone(), s.rows.clone(), s.nav.clone());
            let target = match op {
                NavOp::Visit(p) => {
                    let abs = if p.starts_with('/') { p } else { sftp::join(&s.cwd, &p) };
                    let target = path_complete::normalize_remote(&abs);
                    s.nav.visit(s.cwd.clone(), &target);
                    target
                }
                NavOp::Back => match s.nav.back(s.cwd.clone()) {
                    Some(t) => t,
                    None => return,
                },
                NavOp::Forward => match s.nav.forward(s.cwd.clone()) {
                    Some(t) => t,
                    None => return,
                },
                NavOp::Reload => s.cwd.clone(),
            };
            if target != s.cwd {
                match cached(&cache, &target) {
                    Some(rows) => {
                        s.rows = rows;
                        s.stale = false;
                    }
                    None => s.stale = !s.rows.is_empty(),
                }
                s.cwd = target.clone();
            }
            s.loading = true;
            s.error = None;
            s.generation += 1;
            plan = Some((target, s.generation, before));
        });
        let Some((target, generation, (prev_cwd, prev_rows, prev_nav))) = plan else { return };
        if visited {
            self.remote_list.clear();
        }
        let (state, ctx, host, recorder) = (self.remote.clone(), self.ctx.clone(), self.profile.id, self.recorder());
        self.rt.spawn(async move {
            let canonical = sftp.canonicalize(&target).await.unwrap_or(target);
            let listed = list_rows(&sftp, &canonical).await;
            if let Ok(rows) = &listed {
                learn(&cache, host, &canonical, rows, visited);
                if visited {
                    recorder.record(&canonical);
                }
            }
            set(&state, |s| {
                if s.generation != generation {
                    return;
                }
                s.loading = false;
                s.stale = false;
                match listed {
                    Ok(rows) => {
                        s.cwd = canonical;
                        s.rows = rows;
                    }
                    Err(e) => {
                        s.error = Some(format!("{e:#}"));
                        if visited {
                            (s.cwd, s.rows, s.nav) = (prev_cwd, prev_rows, prev_nav);
                        }
                    }
                }
            });
            ctx.request_repaint();
        });
    }

    fn remote_navigate(&mut self, path: String) {
        self.remote_go(NavOp::Visit(path));
    }

    /// Lists `dir` in the background (for suggestions) unless it is cached or in flight.
    fn remote_prefetch(&self, dir: String) {
        if !dir.starts_with('/') {
            return;
        }
        let dir = path_complete::normalize_remote(&dir);
        if self.cache.lock().map(|c| c.listings.contains(&dir)).unwrap_or(true) {
            return;
        }
        let Some(sftp) = self.sftp() else { return };
        if !self.prefetching.lock().map(|mut p| p.insert(dir.clone())).unwrap_or(false) {
            return;
        }
        let (cache, inflight, ctx, host) = (self.cache.clone(), self.prefetching.clone(), self.ctx.clone(), self.profile.id);
        self.rt.spawn(async move {
            if let Ok(rows) = list_rows(&sftp, &dir).await {
                learn(&cache, host, &dir, &rows, false);
            }
            set(&inflight, |p| {
                p.remove(&dir);
            });
            ctx.request_repaint();
        });
    }

    /// Supplies the remote path field with a directory listing for Tab completion:
    /// the current rows, the host cache, or (no cached data) a live listing.
    fn remote_completion_listing(&mut self, dir: String, cwd: &str, rows: &[Row]) {
        if dir == cwd || path_complete::listing_dir(cwd, path_complete::REMOTE) == dir {
            self.remote_path.provide_listing(dir, candidates(rows));
            return;
        }
        if let Some(rows) = cached(&self.cache, &path_complete::normalize_remote(&dir)) {
            self.remote_path.provide_listing(dir, candidates(&rows));
            return;
        }
        let Some(sftp) = self.sftp() else {
            self.remote_path.listing_failed(&dir);
            return;
        };
        let (slot, ctx, cache, host) = (self.completion.clone(), self.ctx.clone(), self.cache.clone(), self.profile.id);
        self.rt.spawn(async move {
            let listed = list_rows(&sftp, &dir).await.ok();
            if let Some(rows) = &listed {
                learn(&cache, host, &path_complete::normalize_remote(&dir), rows, false);
            }
            set(&slot, |s| *s = Some((dir, listed.map(|rows| candidates(&rows)))));
            ctx.request_repaint();
        });
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
        let remote_cwd = self.remote_cwd();
        self.rt.spawn(async move {
            let result = transfer::run(sftp.clone(), direction, local, remote, pipeline, progress.clone()).await;
            let final_state = match result {
                Ok(()) => TransferState::Done,
                Err(_) if progress.is_cancelled() => TransferState::Cancelled,
                Err(e) => TransferState::Failed(format!("{e:#}")),
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

    pub fn ui(&mut self, ui: &mut Ui) -> bool {
        self.poll_open();
        self.poll_local();
        if let Some(path) = self.jump.lock().ok().and_then(|mut j| j.take()) {
            self.remote_navigate(path);
        }
        if self.local_dirty.load(Ordering::Relaxed) {
            self.refresh_local();
        }
        let mut reconnect = false;
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
            let failed = self.remote.lock().ok().and_then(|s| match &s.link {
                Link::Failed(e) => Some(e.clone()),
                _ => None,
            });
            match failed {
                Some(err) => {
                    // The next connect starts its own "connecting" timer.
                    self.connecting_since = None;
                    reconnect = failed_card(ui, rect, &err);
                }
                None => {
                    self.pane(ui, rect, Side::Remote, fade);
                    self.connecting_screen(ui, rect);
                }
            }
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
        reconnect
    }

    /// Covers the remote pane while the session connects (only if that takes a moment),
    /// then fades away onto the listing.
    fn connecting_screen(&mut self, ui: &Ui, rect: Rect) {
        let (connecting, stage) = self.remote.lock().map(|s| (matches!(s.link, Link::Connecting), s.stage)).unwrap_or((false, Stage::Connecting));
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
            (_, ListAction::Rename(old)) => self.dialog = Some(Dialog::Rename { side, new: old.clone(), old }),
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
        self.watcher.stop_all();
        if let Ok(list) = self.transfers.lock() {
            for t in list.iter() {
                t.progress.cancel();
            }
        }
    }
}

/// Tries the folder the last session ended in; `None` (use the start folder instead)
/// when there is none or it no longer exists.
async fn resume(sftp: &Sftp, last: Option<&str>, cache: &SharedHostCache, state: &Arc<Mutex<RemoteState>>, ctx: &egui::Context) -> Option<(String, Vec<Row>)> {
    let cwd = sftp.canonicalize(last?).await.ok()?;
    show_cached(cache, &cwd, state, ctx);
    let rows = list_rows(sftp, &cwd).await.ok()?;
    Some((cwd, rows))
}

/// Shows the last known listing of `cwd` at once while the fresh one loads.
fn show_cached(cache: &SharedHostCache, cwd: &str, state: &Arc<Mutex<RemoteState>>, ctx: &egui::Context) {
    if let Some(rows) = cached(cache, cwd) {
        set(state, |s| {
            s.cwd = cwd.to_string();
            s.rows = rows;
        });
        ctx.request_repaint();
    }
}

/// The file table, plus skeleton rows while a first listing loads. Rows of the previous
/// folder (`stale`) stay visible, dimmed and inert, until the new listing arrives.
fn listing(ui: &mut Ui, id: Id, side: Side, rows: &[Row], list: &mut ListState, transfer_label: &str, (stale, loading): (bool, bool)) -> Vec<ListAction> {
    let dim = ui.ctx().animate_bool_with_time(id.with("stale"), stale, theme::anim_time(0.15));
    let skeleton = ui.ctx().animate_bool_with_time(id.with("skeleton"), loading && rows.is_empty(), theme::anim_time(0.2));
    let area = ui.available_rect_before_wrap();
    let actions = ui
        .scope(|ui| {
            ui.multiply_opacity(1.0 - 0.55 * dim);
            if stale {
                ui.disable();
            }
            file_list::show(ui, side, rows, list, transfer_label)
        })
        .inner;
    if skeleton > 0.0 {
        let body = Rect::from_min_max(area.min + Vec2::new(0.0, file_list::HEADER_H + ROW_H), area.max);
        loading::skeleton_rows(ui, body, ROW_H, skeleton);
    }
    if stale { Vec::new() } else { actions }
}

/// Memory key holding the `view_n` of the SFTP view the pointer was last over.
fn last_active_id() -> Id {
    Id::new("sftp_last_active_view")
}

/// Small painted "terminal prompt -> folder" button; spins while the lookup runs.
fn terminal_jump_button(ui: &mut Ui, busy: bool) -> bool {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(26.0, 24.0), egui::Sense::click());
    let resp = resp.on_hover_text("Go to the folder of the terminal next to this pane");
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Go to terminal folder"));
    let t = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.12);
    let p = ui.painter();
    if t > 0.0 {
        p.rect_filled(rect, egui::CornerRadius::same(6), theme::surface_3().gamma_multiply(t));
    }
    let col = theme::text_dim().lerp_to_gamma(theme::accent(), t);
    let stroke = egui::Stroke::new(1.5, col);
    let box_ = egui::Rect::from_center_size(rect.center(), egui::vec2(16.0, 12.0));
    p.rect_stroke(box_, egui::CornerRadius::same(2), stroke, egui::StrokeKind::Inside);
    if busy {
        let a = ui.input(|i| i.time) as f32 * 6.0;
        let c = box_.center();
        p.line_segment([c, c + egui::vec2(a.cos(), a.sin()) * 4.0], stroke);
        ui.ctx().request_repaint();
    } else {
        // ">_" prompt glyph drawn with lines.
        let o = box_.left_top() + egui::vec2(3.5, 3.0);
        p.line_segment([o, o + egui::vec2(3.0, 3.0)], stroke);
        p.line_segment([o + egui::vec2(3.0, 3.0), o + egui::vec2(0.0, 6.0)], stroke);
        p.line_segment([o + egui::vec2(5.0, 6.5), o + egui::vec2(9.0, 6.5)], stroke);
    }
    resp.clicked() && !busy
}
