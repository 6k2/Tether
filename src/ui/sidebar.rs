//! Host list: quick connect, search, a folder tree (drag hosts and folders around,
//! collapse with animation, rename in place), an "Archived" drawer and saved workspaces.
//! Right-click anywhere (empty space, a folder, a host, a workspace) for its actions.

mod menu;
mod paint;
mod workspaces;

use eframe::egui::{self, Align2, CornerRadius, FontId, Id, Pos2, Rect, Sense, Stroke, Ui, Vec2};
use egui::collapsing_header::CollapsingState;
use uuid::Uuid;

use super::autocomplete::AutoField;
use super::suggest;
use crate::folders::{Folder, FolderStore};
use crate::keybindings::{self, Action};
use crate::profile::{Profile, ProfileStore};
use crate::pty::ShellSpec;
use crate::theme;
use menu::host_menu;
pub use paint::logo;
use paint::{Glyph, badge, chevron, folder_icon, glyph_button, mini_button, new_split_button, plus, section_header};
pub use workspaces::WorkspaceInfo;
use workspaces::WorkspacePanel;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum OpenKind {
    Terminal,
    Sftp,
    /// Terminal + SFTP side by side (Termius split view).
    Both,
}

pub enum SidebarAction {
    Open(Uuid, OpenKind),
    /// New host, created inside this folder.
    New(Option<Uuid>),
    Edit(Uuid),
    Duplicate(Uuid),
    Delete(Uuid),
    ForgetHostKey(Uuid),
    QuickConnect(Profile),
    /// Save the open layout under this name (replacing one with the same name).
    SaveWorkspace(String),
    OverwriteWorkspace(String),
    RenameWorkspace(String, String),
    RestoreWorkspace(String),
    DeleteWorkspace(String),
    /// Create a folder (inside the given one) and start renaming it.
    NewFolder(Option<Uuid>),
    RenameFolder(Uuid, String),
    DeleteFolder(Uuid),
    SetCollapsed(Uuid, bool),
    CollapseAll(bool),
    MoveHost(Uuid, Option<Uuid>),
    MoveFolder(Uuid, Option<Uuid>),
    Archive(Uuid, bool),
    /// Archive every host in this folder and its sub-folders.
    ArchiveFolder(Uuid),
    SetArchiveOpen(bool),
    OpenKeys,
    Import,
    /// Open a local shell (`pty::shells` id) in the focused pane.
    NewLocal(String),
}

/// What is being dragged around the tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Drag {
    Host(Uuid),
    Folder(Uuid),
}

/// Read-only data the sidebar renders.
pub struct SidebarData<'a> {
    pub store: &'a ProfileStore,
    pub folders: &'a FolderStore,
    pub workspaces: &'a [WorkspaceInfo],
    /// Whether any tab is open (there is a layout to save).
    pub has_tabs: bool,
    pub is_live: &'a dyn Fn(Uuid) -> bool,
    /// Local shells offered by the "New" dropdown.
    pub shells: &'a [ShellSpec],
}

pub struct Sidebar {
    search: String,
    quick: String,
    quick_error: bool,
    /// `user@host` suggestions from saved profiles.
    quick_ac: AutoField,
    /// Folder being renamed in place, with the edited text.
    renaming: Option<(Uuid, String)>,
    focus_quick: bool,
    focus_search: bool,
    workspaces: WorkspacePanel,
}

impl Default for Sidebar {
    fn default() -> Self {
        Self {
            search: String::new(),
            quick: String::new(),
            quick_error: false,
            quick_ac: AutoField::new(egui::Id::new("sidebar_quick")),
            renaming: None,
            focus_quick: false,
            focus_search: false,
            workspaces: WorkspacePanel::default(),
        }
    }
}

const ROW_H: f32 = 44.0;
const FOLDER_H: f32 = 30.0;
const INDENT: f32 = 14.0;
/// Hovering a collapsed folder this long while dragging opens it.
const SPRING_OPEN_SECS: f64 = 0.65;

impl Sidebar {
    /// Starts renaming `folder` in place (after the app created it).
    pub fn start_rename(&mut self, folder: Uuid, name: &str) {
        self.renaming = Some((folder, name.to_string()));
    }

    pub fn focus_quick_connect(&mut self) {
        self.focus_quick = true;
    }

    pub fn focus_search(&mut self) {
        self.focus_search = true;
    }

    /// The brand row with "New" and "⋯", above the sections.
    /// The Hosts view header's buttons ("More", then "New" with its dropdown), laid
    /// out right to left in `ui`.
    pub fn hosts_header(&mut self, ui: &mut Ui, data: &SidebarData) -> Vec<SidebarAction> {
        let mut actions = Vec::new();
        ui.spacing_mut().item_spacing.x = 4.0;
        let more = glyph_button(ui, Glyph::More, "More");
        egui::Popup::menu(&more).show(|ui| self.background_menu(ui, &mut actions));
        let (main, caret) = new_split_button(ui);
        if main.on_hover_text(keybindings::hint("New host", Action::NewHost)).clicked() {
            actions.push(SidebarAction::New(None));
        }
        egui::Popup::menu(&caret).show(|ui| self.new_menu(ui, data.shells, &mut actions));
        actions
    }

    /// Hosts that aren't archived (the section header's count).
    pub fn host_count(data: &SidebarData) -> usize {
        data.store.profiles.iter().filter(|p| !p.archived).count()
    }

    pub fn workspaces_height(&self, data: &SidebarData) -> f32 {
        self.workspaces.body_height(data.workspaces)
    }

    pub fn workspaces_header(&mut self, ui: &mut Ui, data: &SidebarData) {
        self.workspaces.header_buttons(ui, data.has_tabs);
    }

    pub fn workspaces_body(&mut self, ui: &mut Ui, data: &SidebarData) -> Vec<SidebarAction> {
        let mut actions = Vec::new();
        self.workspaces.body(ui, data.workspaces, data.has_tabs, &mut actions);
        actions
    }

    /// Quick connect, search and the host tree, filling `ui`.
    pub fn hosts_body(&mut self, ui: &mut Ui, data: &SidebarData) -> Vec<SidebarAction> {
        let mut actions = Vec::new();
        ui.add_space(2.0);
        self.quick_connect(ui, data.store, &mut actions);
        ui.add_space(4.0);
        let search = ui
            .add(egui::TextEdit::singleline(&mut self.search).hint_text("🔍 Search hosts").desired_width(f32::INFINITY).margin(egui::Margin::symmetric(10, 6)));
        if std::mem::take(&mut self.focus_search) {
            search.request_focus();
        }
        if search.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.search.clear();
        }
        ui.add_space(6.0);

        egui::ScrollArea::vertical().id_salt("sidebar_hosts").max_height(ui.available_height()).auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            if data.store.profiles.is_empty() && data.folders.folders.is_empty() {
                empty_hint(ui);
            } else if self.search.trim().is_empty() {
                self.tree(ui, data, &mut actions);
            } else {
                self.results(ui, data, &mut actions);
            }
            ui.add_space(6.0);
            if new_host_row(ui) {
                actions.push(SidebarAction::New(None));
            }
            // Leftover space takes drops to the top level and offers "New…" on right-click.
            let rest = ui.available_rect_before_wrap();
            if rest.height() > 4.0 {
                let resp = ui.allocate_rect(rest, Sense::click());
                drop_target(ui, &resp, None, &mut actions, data);
                resp.context_menu(|ui| self.background_menu(ui, &mut actions));
            }
        });
        drag_ghost(ui, data);
        actions
    }

    /// Starts the inline "Save current layout as…" field.
    pub fn save_workspace(&mut self) {
        self.workspaces.start_save();
    }

    fn quick_connect(&mut self, ui: &mut Ui, store: &ProfileStore, actions: &mut Vec<SidebarAction>) {
        let quick_color = if self.quick_error { theme::danger() } else { theme::text() };
        let entries = suggest::quick_entries(&store.profiles);
        let quick = self.quick_ac.show(
            ui,
            &mut self.quick,
            |text| {
                egui::TextEdit::singleline(text)
                    .hint_text("⚡ user@host:port")
                    .desired_width(f32::INFINITY)
                    .margin(egui::Margin::symmetric(10, 6))
                    .text_color(quick_color)
            },
            |q| suggest::rank_quick(q, &entries, suggest::SUGGEST_LIMIT),
            egui::Order::Foreground,
        );
        if std::mem::take(&mut self.focus_quick) {
            quick.request_focus();
        }
        if quick.changed() {
            self.quick_error = false;
        }
        if quick.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && !self.quick.trim().is_empty() {
            match Profile::parse_quick(&self.quick) {
                Some(p) => {
                    actions.push(SidebarAction::QuickConnect(p));
                    self.quick.clear();
                }
                None => self.quick_error = true,
            }
        }
    }

    /// The folder tree, then top-level hosts, then the archive drawer.
    fn tree(&mut self, ui: &mut Ui, data: &SidebarData, actions: &mut Vec<SidebarAction>) {
        // The section header above says "HOSTS"; dropping on the space below the tree
        // files an item at the top level.
        self.contents(ui, data, None, 0, actions);

        let archived: Vec<&Profile> = sorted(data.store.profiles.iter().filter(|p| p.archived));
        if !archived.is_empty() || dragging(ui).is_some_and(|d| matches!(d, Drag::Host(_))) {
            ui.add_space(10.0);
            self.archive(ui, data, archived, actions);
        }
    }

    /// Sub-folders and hosts of `parent`.
    fn contents(&mut self, ui: &mut Ui, data: &SidebarData, parent: Option<Uuid>, depth: usize, actions: &mut Vec<SidebarAction>) {
        for folder in data.folders.children(parent) {
            self.folder(ui, data, folder, depth, actions);
        }
        for p in sorted(data.store.profiles.iter().filter(|p| !p.archived && folder_of(data, p) == parent)) {
            let resp = host_row(ui, p, (data.is_live)(p.id), depth, None, data, actions);
            // Dropping on a host files the dragged item next to it.
            drop_target(ui, &resp, parent, actions, data);
        }
    }

    fn folder(&mut self, ui: &mut Ui, data: &SidebarData, f: &Folder, depth: usize, actions: &mut Vec<SidebarAction>) {
        let id = Id::new(("sidebar_folder", f.id));
        let mut state = CollapsingState::load_with_default_open(ui.ctx(), id, !f.collapsed);
        // The store is the source of truth (collapse all, restarts).
        if state.is_open() == f.collapsed {
            state.set_open(!f.collapsed);
        }
        let count = count_hosts(data, f.id);
        let header = self.folder_header(ui, f, data, depth, state.openness(ui.ctx()), actions);
        if header.clicked() && self.renaming.as_ref().is_none_or(|(r, _)| *r != f.id) {
            state.set_open(f.collapsed);
            actions.push(SidebarAction::SetCollapsed(f.id, !f.collapsed));
        }
        let hovering = drop_target(ui, &header, Some(f.id), actions, data);
        if f.collapsed && spring_open(ui, id, hovering) {
            state.set_open(true);
            actions.push(SidebarAction::SetCollapsed(f.id, false));
        }
        state.show_body_unindented(ui, |ui| {
            self.contents(ui, data, Some(f.id), depth + 1, actions);
            if count == 0 && data.folders.children(Some(f.id)).is_empty() {
                empty_folder(ui, depth + 1);
            }
        });
    }

    fn folder_header(&mut self, ui: &mut Ui, f: &Folder, data: &SidebarData, depth: usize, openness: f32, actions: &mut Vec<SidebarAction>) -> egui::Response {
        let count = count_hosts(data, f.id);
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), FOLDER_H), Sense::click_and_drag());
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::CollapsingHeader, true, &f.name));
        if resp.drag_started() {
            resp.dnd_set_drag_payload(Drag::Folder(f.id));
        }
        let ctx = ui.ctx().clone();
        // Stay "hot" while the ⋯ menu is open, or it would vanish as the pointer moves
        // into it (the button is only drawn while the row is hot).
        // `Popup::default_response_id` of the ⋯ button (its id is `resp.id.with("more")`).
        let menu_open = egui::Popup::is_id_open(&ctx, resp.id.with("more").with("popup"));
        let hot = ui.rect_contains_pointer(rect) || resp.context_menu_opened() || menu_open;
        let hover = ctx.animate_bool_with_time(resp.id.with("h"), hot, theme::anim_time(0.12));
        let p = ui.painter();
        if hover > 0.0 {
            p.rect_filled(rect, CornerRadius::same(theme::RADIUS_SM), theme::surface_2().gamma_multiply(hover));
        }
        let left = rect.left() + depth as f32 * INDENT;
        let col = theme::text_dim().lerp_to_gamma(theme::text(), hover);
        chevron(p, Pos2::new(left + 9.0, rect.center().y), openness, theme::text_faint().lerp_to_gamma(col, hover));
        let icon = Rect::from_center_size(Pos2::new(left + 26.0, rect.center().y), Vec2::new(16.0, 13.0));
        folder_icon(p, icon, openness, theme::warn().gamma_multiply(0.85 + 0.15 * hover));
        let text_x = left + 40.0;

        let renaming = self.renaming.as_ref().is_some_and(|(r, _)| *r == f.id);
        if renaming {
            let edit_rect = Rect::from_min_max(Pos2::new(text_x - 4.0, rect.top() + 4.0), Pos2::new(rect.right() - 4.0, rect.bottom() - 4.0));
            let mut commit = None;
            if let Some((_, text)) = &mut self.renaming {
                let edit = ui.put(edit_rect, egui::TextEdit::singleline(text).margin(egui::Margin::symmetric(4, 2)).font(FontId::proportional(13.0)));
                if !edit.has_focus() && !edit.lost_focus() {
                    edit.request_focus();
                }
                let (enter, esc) = ui.input(|i| (i.key_pressed(egui::Key::Enter), i.key_pressed(egui::Key::Escape)));
                if esc {
                    commit = Some(None);
                } else if enter || edit.lost_focus() {
                    commit = Some(Some(text.clone()));
                }
            }
            if let Some(result) = commit {
                if let Some(name) = result.filter(|n| !n.trim().is_empty()) {
                    actions.push(SidebarAction::RenameFolder(f.id, name));
                }
                self.renaming = None;
            }
            return resp;
        }

        let buttons_w = 48.0 * hover;
        let clip = Rect::from_min_max(Pos2::new(text_x, rect.top()), Pos2::new(rect.right() - 8.0 - buttons_w.max(22.0), rect.bottom()));
        p.with_clip_rect(clip).text(Pos2::new(text_x, rect.center().y), Align2::LEFT_CENTER, &f.name, FontId::proportional(13.0), theme::text());
        if hover < 0.95 && count > 0 {
            badge(p, Pos2::new(rect.right() - 12.0, rect.center().y), count, 1.0 - hover);
        }
        if hover > 0.05 {
            let add = mini_button(ui, resp.id.with("add"), Pos2::new(rect.right() - 36.0, rect.center().y), Glyph::Plus, hover, "New host in this folder");
            if add.clicked() {
                actions.push(SidebarAction::New(Some(f.id)));
            }
            let more = mini_button(ui, resp.id.with("more"), Pos2::new(rect.right() - 13.0, rect.center().y), Glyph::More, hover, "Folder actions");
            egui::Popup::menu(&more).show(|ui| self.folder_menu(ui, f, data, actions));
        }
        if hover > 0.5 && ui.input(|i| i.key_pressed(egui::Key::F2)) {
            self.renaming = Some((f.id, f.name.clone()));
        }
        resp.context_menu(|ui| self.folder_menu(ui, f, data, actions));
        resp
    }

    fn archive(&mut self, ui: &mut Ui, data: &SidebarData, archived: Vec<&Profile>, actions: &mut Vec<SidebarAction>) {
        let id = Id::new("sidebar_archive");
        let mut state = CollapsingState::load_with_default_open(ui.ctx(), id, data.folders.archive_open);
        if state.is_open() != data.folders.archive_open {
            state.set_open(data.folders.archive_open);
        }
        let header = section_header(ui, "ARCHIVED", archived.len(), Some(state.openness(ui.ctx())));
        if header.clicked() {
            state.set_open(!data.folders.archive_open);
            actions.push(SidebarAction::SetArchiveOpen(!data.folders.archive_open));
        }
        let hovering = header.dnd_hover_payload::<Drag>().is_some_and(|d| matches!(*d, Drag::Host(_)));
        let glow = ui.ctx().animate_bool_with_time(header.id.with("drop"), hovering, theme::anim_time(0.12));
        if glow > 0.0 {
            ui.painter().rect_stroke(
                header.rect,
                CornerRadius::same(theme::RADIUS_SM),
                Stroke::new(1.5, theme::warn().gamma_multiply(glow)),
                egui::StrokeKind::Inside,
            );
        }
        if let Some(d) = header.dnd_release_payload::<Drag>()
            && let Drag::Host(h) = *d
        {
            actions.push(SidebarAction::Archive(h, true));
        }
        state.show_body_unindented(ui, |ui| {
            ui.scope(|ui| {
                ui.multiply_opacity(0.75);
                for p in archived {
                    let path = p.folder.map(|f| data.folders.path_of(f)).filter(|s| !s.is_empty());
                    host_row(ui, p, (data.is_live)(p.id), 0, path.as_deref(), data, actions);
                }
            });
        });
    }

    /// Flat search results with each host's folder path.
    fn results(&mut self, ui: &mut Ui, data: &SidebarData, actions: &mut Vec<SidebarAction>) {
        let needle = self.search.trim().to_lowercase();
        let hits: Vec<&Profile> = sorted(data.store.profiles.iter().filter(|p| {
            let path = p.folder.map(|f| data.folders.path_of(f)).unwrap_or_default().to_lowercase();
            p.display_name().to_lowercase().contains(&needle)
                || p.host.to_lowercase().contains(&needle)
                || p.user.to_lowercase().contains(&needle)
                || path.contains(&needle)
        }));
        section_header(ui, "RESULTS", hits.len(), None);
        ui.add_space(2.0);
        if hits.is_empty() {
            ui.add_space(14.0);
            ui.vertical_centered(|ui| ui.label(egui::RichText::new("No matching hosts").color(theme::text_faint())));
        }
        for p in hits {
            let mut path = p.folder.map(|f| data.folders.path_of(f)).unwrap_or_default();
            if p.archived {
                path = if path.is_empty() { "Archived".into() } else { format!("Archived · {path}") };
            }
            host_row(ui, p, (data.is_live)(p.id), 0, Some(&path).filter(|s| !s.is_empty()).map(|s| s.as_str()), data, actions);
        }
    }
}

/// The folder a host is shown in: a folder id that no longer exists (deleted or a lost
/// `folders.json`) counts as the top level, so the host never disappears.
fn folder_of(data: &SidebarData, p: &Profile) -> Option<Uuid> {
    p.folder.filter(|f| data.folders.get(*f).is_some())
}

fn sorted<'a>(it: impl Iterator<Item = &'a Profile>) -> Vec<&'a Profile> {
    let mut v: Vec<&Profile> = it.collect();
    v.sort_by_key(|p| p.display_name().to_lowercase());
    v
}

/// Non-archived hosts in `folder` and below.
fn count_hosts(data: &SidebarData, folder: Uuid) -> usize {
    let mut ids = data.folders.descendants(folder);
    ids.insert(folder);
    data.store.profiles.iter().filter(|p| !p.archived && p.folder.is_some_and(|f| ids.contains(&f))).count()
}

fn dragging(ui: &Ui) -> Option<Drag> {
    egui::DragAndDrop::payload::<Drag>(ui.ctx()).map(|d| *d)
}

/// Highlights `resp` while a host or folder hovers it and files the item into `folder`
/// on release. Returns whether something is hovering it now.
fn drop_target(ui: &Ui, resp: &egui::Response, folder: Option<Uuid>, actions: &mut Vec<SidebarAction>, data: &SidebarData) -> bool {
    let accepts = |d: &Drag| match *d {
        Drag::Host(h) => data.store.get(h).is_some_and(|p| folder_of(data, p) != folder || p.archived),
        Drag::Folder(id) => {
            Some(id) != folder && data.folders.get(id).is_some_and(|f| f.parent != folder) && folder.is_none_or(|t| !data.folders.descendants(id).contains(&t))
        }
    };
    let hovering = resp.dnd_hover_payload::<Drag>().is_some_and(|d| accepts(&d));
    let glow = ui.ctx().animate_bool_with_time(resp.id.with("drop_glow"), hovering, theme::anim_time(0.12));
    if glow > 0.0 {
        let p = ui.painter();
        p.rect_filled(resp.rect, CornerRadius::same(theme::RADIUS_SM), theme::accent().gamma_multiply(0.10 * glow));
        p.rect_stroke(resp.rect, CornerRadius::same(theme::RADIUS_SM), Stroke::new(1.5, theme::accent().gamma_multiply(glow)), egui::StrokeKind::Inside);
    }
    if let Some(d) = resp.dnd_release_payload::<Drag>()
        && accepts(&d)
    {
        match *d {
            Drag::Host(h) => {
                if data.store.get(h).is_some_and(|p| p.archived) {
                    actions.push(SidebarAction::Archive(h, false));
                }
                actions.push(SidebarAction::MoveHost(h, folder));
            }
            Drag::Folder(id) => actions.push(SidebarAction::MoveFolder(id, folder)),
        }
    }
    hovering
}

/// True once a drag has hovered the folder `id` long enough to open it.
fn spring_open(ui: &Ui, id: Id, hovering: bool) -> bool {
    let now = ui.input(|i| i.time);
    let key = id.with("spring");
    if !hovering {
        ui.ctx().data_mut(|d| d.remove::<f64>(key));
        return false;
    }
    let since = ui.ctx().data_mut(|d| *d.get_temp_mut_or_insert_with(key, || now));
    ui.ctx().request_repaint();
    if now - since > SPRING_OPEN_SECS {
        ui.ctx().data_mut(|d| d.remove::<f64>(key));
        return true;
    }
    false
}

/// A chip following the pointer while something is dragged.
fn drag_ghost(ui: &Ui, data: &SidebarData) {
    let Some(d) = dragging(ui) else { return };
    let Some(pos) = ui.ctx().pointer_interact_pos() else { return };
    let name = match d {
        Drag::Host(h) => data.store.get(h).map(|p| p.display_name().to_string()),
        Drag::Folder(f) => data.folders.get(f).map(|f| f.name.clone()),
    };
    let Some(name) = name else { return };
    let painter = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Tooltip, Id::new("sidebar_drag_ghost")));
    let galley = painter.layout_no_wrap(name, FontId::proportional(12.5), theme::text());
    let rect = Rect::from_min_size(pos + Vec2::new(14.0, 8.0), galley.size() + Vec2::new(34.0, 12.0));
    painter.add(egui::Shadow { offset: [0, 4], blur: 12, spread: 0, color: theme::shadow() }.as_shape(rect, CornerRadius::same(8)));
    painter.rect(rect, CornerRadius::same(8), theme::surface_2(), Stroke::new(1.0, theme::accent().gamma_multiply(0.6)), egui::StrokeKind::Inside);
    let icon = Rect::from_center_size(Pos2::new(rect.left() + 14.0, rect.center().y), Vec2::new(12.0, 10.0));
    match d {
        Drag::Host(h) => {
            let hue = data.store.get(h).map_or(0.0, |p| p.hue);
            painter.rect_filled(icon.shrink(1.0), CornerRadius::same(3), theme::hue_color(hue));
        }
        Drag::Folder(_) => folder_icon(&painter, icon, 0.0, theme::warn()),
    }
    painter.galley(Pos2::new(rect.left() + 26.0, rect.center().y - galley.size().y / 2.0), galley, theme::text());
}

fn empty_folder(ui: &mut Ui, depth: usize) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 24.0), Sense::hover());
    let x = rect.left() + depth as f32 * INDENT + 40.0;
    ui.painter().text(
        Pos2::new(x, rect.center().y),
        Align2::LEFT_CENTER,
        "Empty — drag hosts here",
        FontId::proportional(11.5),
        theme::text_faint().gamma_multiply(0.8),
    );
}

/// Dashed "add host" row at the end of the list; fills with the accent on hover.
fn new_host_row(ui: &mut Ui) -> bool {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 38.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "New host"));
    let t = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.14);
    let p = ui.painter();
    let r = rect.shrink2(Vec2::new(0.0, 3.0));
    let accent = theme::accent();
    p.rect_filled(r, CornerRadius::same(theme::RADIUS), accent.gamma_multiply(0.10 * t));
    let stroke = Stroke::new(1.0, theme::border().lerp_to_gamma(accent, t));
    let dashes = [(r.left_top(), r.right_top()), (r.right_top(), r.right_bottom()), (r.right_bottom(), r.left_bottom()), (r.left_bottom(), r.left_top())];
    for (a, b) in dashes {
        p.extend(egui::Shape::dashed_line(&[a, b], stroke, 5.0, 4.0));
    }
    let c = Pos2::new(r.left() + 22.0, r.center().y);
    let col = theme::text_dim().lerp_to_gamma(accent, t);
    plus(p, c, 5.0, Stroke::new(1.6, col));
    p.text(Pos2::new(r.left() + 38.0, r.center().y), Align2::LEFT_CENTER, "New host", FontId::proportional(13.0), col);
    resp.clicked()
}

fn empty_hint(ui: &mut Ui) {
    ui.add_space(30.0);
    ui.vertical_centered(|ui| {
        ui.label(egui::RichText::new("No hosts yet").color(theme::text_dim()));
        ui.label(egui::RichText::new("Press + or use quick connect").size(11.5).color(theme::text_faint()));
    });
}

fn host_row(ui: &mut Ui, p: &Profile, live: bool, depth: usize, path: Option<&str>, data: &SidebarData, actions: &mut Vec<SidebarAction>) -> egui::Response {
    let width = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, ROW_H), Sense::click_and_drag());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, p.display_name()));
    if resp.drag_started() {
        resp.dnd_set_drag_payload(Drag::Host(p.id));
    }
    let ctx = ui.ctx().clone();
    let being_dragged = dragging(ui) == Some(Drag::Host(p.id));
    // Rect-based (not `hovered`/`contains_pointer`, which turn false over covering widgets) so
    // the row stays "hot" while the pointer is on its own action buttons layered on top.
    let hot = (ui.rect_contains_pointer(rect) || resp.context_menu_opened()) && dragging(ui).is_none();
    let hover = ctx.animate_bool_with_time(resp.id, hot, 0.14);
    let lift = ctx.animate_bool_with_time(resp.id.with("drag"), being_dragged, 0.12);
    let painter = ui.painter();
    if hover > 0.0 {
        painter.rect_filled(rect, CornerRadius::same(theme::RADIUS), theme::surface_2().gamma_multiply(hover));
    }
    let alpha = 1.0 - 0.6 * lift;
    let left = rect.left() + depth as f32 * INDENT + if depth > 0 { 6.0 } else { 0.0 };
    // Avatar: rounded square with the host's hue and initial.
    let accent = theme::hue_color(p.hue);
    let avatar = Rect::from_center_size(Pos2::new(left + 22.0, rect.center().y), Vec2::splat(28.0));
    painter.rect_filled(avatar, CornerRadius::same(7), accent.gamma_multiply(0.22 * alpha));
    let initial = p.display_name().chars().next().unwrap_or('?').to_uppercase().to_string();
    painter.text(avatar.center(), Align2::CENTER_CENTER, initial, FontId::proportional(13.0), accent.gamma_multiply(alpha));
    if live {
        // A static dot with a soft halo: a pulsing one would repaint the app forever.
        let c = Pos2::new(avatar.right() - 1.0, avatar.bottom() - 1.0);
        painter.circle_filled(c, 4.5, theme::bg());
        painter.circle_filled(c, 3.0, theme::accent());
        painter.circle_stroke(c, 4.5, Stroke::new(1.0, theme::accent().gamma_multiply(0.35)));
    }

    let text_left = avatar.right() + 10.0;
    let buttons_w = 78.0 * hover;
    let clip = Rect::from_min_max(Pos2::new(text_left, rect.top()), Pos2::new(rect.right() - 6.0 - buttons_w, rect.bottom()));
    let tp = painter.with_clip_rect(clip);
    tp.text(
        Pos2::new(text_left, rect.center().y - 8.0),
        Align2::LEFT_CENTER,
        p.display_name(),
        FontId::proportional(13.5),
        theme::text().gamma_multiply(alpha),
    );
    let sub = match path {
        Some(path) => format!("{path} · {}", p.address()),
        None => p.address(),
    };
    tp.text(Pos2::new(text_left, rect.center().y + 9.0), Align2::LEFT_CENTER, sub, FontId::proportional(11.5), theme::text_faint().gamma_multiply(alpha));

    if hover > 0.05 {
        let icons = [("⌨", OpenKind::Terminal, "Terminal"), ("📁", OpenKind::Sftp, "SFTP"), ("🗗", OpenKind::Both, "Terminal + SFTP")];
        for (i, (icon, kind, tip)) in icons.iter().enumerate() {
            let x = rect.right() - 14.0 - (2 - i) as f32 * 24.0 + (1.0 - hover) * 12.0;
            let r = Rect::from_center_size(Pos2::new(x, rect.center().y), Vec2::splat(22.0));
            let br = ui.interact(r, resp.id.with(i), Sense::click()).on_hover_text(*tip);
            let bg = if br.hovered() { theme::surface_3() } else { theme::surface_2() };
            ui.painter().rect_filled(r, CornerRadius::same(6), bg.gamma_multiply(hover));
            ui.painter().text(r.center(), Align2::CENTER_CENTER, *icon, FontId::proportional(12.5), theme::text().gamma_multiply(hover));
            if br.clicked() {
                actions.push(SidebarAction::Open(p.id, *kind));
            }
        }
    }

    if resp.double_clicked() {
        actions.push(SidebarAction::Open(p.id, OpenKind::Terminal));
    }
    resp.context_menu(|ui| host_menu(ui, p, data, actions));
    resp
}
