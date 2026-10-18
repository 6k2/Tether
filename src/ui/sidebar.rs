//! Host list: quick connect, search, a folder tree (drag hosts and folders around,
//! collapse with animation, rename in place), an "Archived" drawer and saved workspaces.

use eframe::egui::{self, Align2, CornerRadius, FontId, Id, Pos2, Rect, Sense, Stroke, Ui, Vec2};
use egui::collapsing_header::CollapsingState;
use uuid::Uuid;

use super::autocomplete::AutoField;
use super::suggest;
use crate::folders::{Folder, FolderStore};
use crate::keybindings::{self, Action};
use crate::profile::{Profile, ProfileStore};
use crate::theme;

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
    SaveWorkspace,
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
    SetArchiveOpen(bool),
    OpenKeys,
    Import,
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
    pub workspaces: &'a [String],
    pub is_live: &'a dyn Fn(Uuid) -> bool,
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

    pub fn ui(&mut self, ui: &mut Ui, data: &SidebarData) -> Vec<SidebarAction> {
        let mut actions = Vec::new();
        ui.add_space(6.0);
        self.title_row(ui, &mut actions);
        ui.add_space(8.0);
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
        ui.add_space(10.0);

        let bottom_h = 40.0 + data.workspaces.len().min(5) as f32 * 26.0;
        egui::ScrollArea::vertical().max_height(ui.available_height() - bottom_h).auto_shrink([false, false]).show(ui, |ui| {
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
            // Leftover space takes drops to the top level.
            let rest = ui.available_rect_before_wrap();
            if rest.height() > 4.0 {
                let resp = ui.allocate_rect(rest, Sense::hover());
                drop_target(ui, &resp, None, &mut actions, data);
            }
        });
        drag_ghost(ui, data);

        workspaces(ui, data.workspaces, &mut actions);
        actions
    }

    fn title_row(&mut self, ui: &mut Ui, actions: &mut Vec<SidebarAction>) {
        ui.horizontal(|ui| {
            logo(ui, 18.0);
            ui.label(egui::RichText::new("tether").size(17.0).strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                if glyph_button(ui, Glyph::Plus, &keybindings::hint("New host", Action::NewHost)).clicked() {
                    actions.push(SidebarAction::New(None));
                }
                if glyph_button(ui, Glyph::NewFolder, &keybindings::hint("New folder", Action::NewFolder)).clicked() {
                    actions.push(SidebarAction::NewFolder(None));
                }
                let more = glyph_button(ui, Glyph::More, "More");
                egui::Popup::menu(&more).show(|ui| {
                    ui.set_min_width(190.0);
                    if ui.button("🔑  Key manager").clicked() {
                        actions.push(SidebarAction::OpenKeys);
                    }
                    if ui.button("⬇  Import hosts…").clicked() {
                        actions.push(SidebarAction::Import);
                    }
                    ui.separator();
                    if ui.button("▸  Collapse all folders").clicked() {
                        actions.push(SidebarAction::CollapseAll(true));
                    }
                    if ui.button("▾  Expand all folders").clicked() {
                        actions.push(SidebarAction::CollapseAll(false));
                    }
                });
            });
        });
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
        let active = data.store.profiles.iter().filter(|p| !p.archived).count();
        let header = section_header(ui, "HOSTS", active, None);
        drop_target(ui, &header, None, actions, data);
        ui.add_space(2.0);
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
        let header = self.folder_header(ui, f, depth, state.openness(ui.ctx()), count, actions);
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

    fn folder_header(&mut self, ui: &mut Ui, f: &Folder, depth: usize, openness: f32, count: usize, actions: &mut Vec<SidebarAction>) -> egui::Response {
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
            egui::Popup::menu(&more).show(|ui| self.folder_menu(ui, f, actions));
        }
        if hover > 0.5 && ui.input(|i| i.key_pressed(egui::Key::F2)) {
            self.renaming = Some((f.id, f.name.clone()));
        }
        resp.context_menu(|ui| self.folder_menu(ui, f, actions));
        resp
    }

    fn folder_menu(&mut self, ui: &mut Ui, f: &Folder, actions: &mut Vec<SidebarAction>) {
        ui.set_min_width(180.0);
        if ui.button("+  New host here").clicked() {
            actions.push(SidebarAction::New(Some(f.id)));
        }
        if ui.button("📁  New subfolder").clicked() {
            actions.push(SidebarAction::NewFolder(Some(f.id)));
        }
        ui.separator();
        if ui.button("✏  Rename").clicked() {
            self.renaming = Some((f.id, f.name.clone()));
        }
        if f.parent.is_some() && ui.button("⤴  Move to top level").clicked() {
            actions.push(SidebarAction::MoveFolder(f.id, None));
        }
        if ui.button(egui::RichText::new("🗑  Delete folder").color(theme::danger())).on_hover_text("Its hosts and sub-folders move up a level").clicked() {
            actions.push(SidebarAction::DeleteFolder(f.id));
        }
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

/// "HOSTS · 12" row; with `openness` it is a collapsible header with a chevron.
fn section_header(ui: &mut Ui, title: &str, count: usize, openness: Option<f32>) -> egui::Response {
    let sense = if openness.is_some() { Sense::click() } else { Sense::hover() };
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 22.0), sense);
    let p = ui.painter();
    let mut x = rect.left() + 2.0;
    if let Some(o) = openness {
        let hover = ui.ctx().animate_bool_with_time(resp.id.with("h"), resp.hovered(), theme::anim_time(0.12));
        chevron(p, Pos2::new(x + 5.0, rect.center().y), o, theme::text_faint().lerp_to_gamma(theme::text(), hover));
        x += 14.0;
    }
    let galley = p.layout_no_wrap(title.to_string(), FontId::proportional(10.5), theme::text_faint());
    let w = galley.size().x;
    p.galley(Pos2::new(x, rect.center().y - galley.size().y / 2.0), galley, theme::text_faint());
    if count > 0 {
        p.text(
            Pos2::new(x + w + 6.0, rect.center().y),
            Align2::LEFT_CENTER,
            count.to_string(),
            FontId::proportional(10.5),
            theme::text_faint().gamma_multiply(0.7),
        );
    }
    resp
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

fn workspaces(ui: &mut Ui, names: &[String], actions: &mut Vec<SidebarAction>) {
    ui.separator();
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("WORKSPACES").size(10.5).strong().color(theme::text_faint()));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.add(egui::Button::new("💾").frame(false)).on_hover_text("Save current layout as workspace").clicked() {
                actions.push(SidebarAction::SaveWorkspace);
            }
        });
    });
    egui::ScrollArea::vertical().id_salt("ws").max_height(130.0).show(ui, |ui| {
        for name in names {
            let r = ui.add(egui::Button::new(format!("›  {name}")).frame(false));
            if r.clicked() {
                actions.push(SidebarAction::RestoreWorkspace(name.clone()));
            }
            r.context_menu(|ui| {
                if ui.button(egui::RichText::new("🗑  Delete workspace").color(theme::danger())).clicked() {
                    actions.push(SidebarAction::DeleteWorkspace(name.clone()));
                }
            });
        }
    });
}

/// The tether mark: two linked rounded diamonds.
pub fn logo(ui: &mut Ui, size: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    let p = ui.painter();
    let c = rect.center();
    let r = size * 0.32;
    for (dx, color) in [(-size * 0.14, theme::accent()), (size * 0.14, theme::blue())] {
        let center = c + Vec2::new(dx, 0.0);
        let pts = vec![center + Vec2::new(0.0, -r), center + Vec2::new(r, 0.0), center + Vec2::new(0.0, r), center + Vec2::new(-r, 0.0)];
        p.add(egui::Shape::convex_polygon(pts, color.gamma_multiply(0.18), Stroke::new(size * 0.08, color)));
    }
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
        let pulse = (ctx.input(|i| i.time) * 2.0).sin() as f32 * 0.5 + 0.5;
        let c = Pos2::new(avatar.right() - 1.0, avatar.bottom() - 1.0);
        painter.circle_filled(c, 4.5, theme::bg());
        painter.circle_filled(c, 3.0, theme::accent());
        painter.circle_stroke(c, 3.0 + pulse * 2.5, Stroke::new(1.0, theme::accent().gamma_multiply(1.0 - pulse)));
        ctx.request_repaint_after(std::time::Duration::from_millis(50));
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

fn host_menu(ui: &mut Ui, p: &Profile, data: &SidebarData, actions: &mut Vec<SidebarAction>) {
    ui.set_min_width(200.0);
    if ui.button("⌨  Open terminal").clicked() {
        actions.push(SidebarAction::Open(p.id, OpenKind::Terminal));
    }
    if ui.button("📁  Open SFTP").clicked() {
        actions.push(SidebarAction::Open(p.id, OpenKind::Sftp));
    }
    if ui.button("🗗  Terminal + SFTP").clicked() {
        actions.push(SidebarAction::Open(p.id, OpenKind::Both));
    }
    ui.separator();
    if ui.button("✏  Edit").clicked() {
        actions.push(SidebarAction::Edit(p.id));
    }
    if ui.button("⧉  Duplicate").clicked() {
        actions.push(SidebarAction::Duplicate(p.id));
    }
    ui.menu_button("📂  Move to", |ui| {
        ui.set_min_width(180.0);
        if ui.add_enabled(p.folder.is_some(), egui::Button::new("Top level")).clicked() {
            actions.push(SidebarAction::MoveHost(p.id, None));
        }
        let tree = data.folders.flattened();
        if !tree.is_empty() {
            ui.separator();
        }
        for (id, depth) in tree {
            let name = data.folders.get(id).map(|f| f.name.as_str()).unwrap_or_default();
            let label = format!("{}{name}", "    ".repeat(depth));
            if ui.add_enabled(p.folder != Some(id), egui::Button::new(label)).clicked() {
                actions.push(SidebarAction::MoveHost(p.id, Some(id)));
            }
        }
    });
    let (label, archive) = if p.archived { ("📤  Unarchive", false) } else { ("📥  Archive", true) };
    if ui.button(label).clicked() {
        actions.push(SidebarAction::Archive(p.id, archive));
    }
    ui.separator();
    if ui.button("🔑  Forget host key").on_hover_text("Use after the server was reinstalled; the next connect re-learns its key").clicked() {
        actions.push(SidebarAction::ForgetHostKey(p.id));
    }
    if ui.button(egui::RichText::new("🗑  Delete").color(theme::danger())).clicked() {
        actions.push(SidebarAction::Delete(p.id));
    }
}

// ---- painted bits ---------------------------------------------------------------------

#[derive(Clone, Copy)]
enum Glyph {
    Plus,
    NewFolder,
    More,
}

fn paint_glyph(p: &egui::Painter, c: Pos2, g: Glyph, col: egui::Color32) {
    let s = Stroke::new(1.5, col);
    match g {
        Glyph::Plus => plus(p, c, 5.0, s),
        Glyph::More => {
            for dx in [-5.0, 0.0, 5.0] {
                p.circle_filled(c + Vec2::new(dx, 0.0), 1.5, col);
            }
        }
        Glyph::NewFolder => {
            let r = Rect::from_center_size(c + Vec2::new(-1.0, 0.5), Vec2::new(13.0, 10.0));
            folder_icon(p, r, 0.0, col);
            plus(p, Pos2::new(r.right() + 1.0, r.bottom() - 1.0), 2.8, Stroke::new(1.4, col));
        }
    }
}

fn glyph_button(ui: &mut Ui, g: Glyph, tip: &str) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(24.0), Sense::click());
    let h = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), theme::anim_time(0.1));
    if h > 0.0 {
        ui.painter().rect_filled(rect, CornerRadius::same(6), theme::surface_3().gamma_multiply(h));
    }
    paint_glyph(ui.painter(), rect.center(), g, theme::text_dim().lerp_to_gamma(theme::text(), h));
    resp.on_hover_text(tip)
}

fn mini_button(ui: &mut Ui, id: Id, c: Pos2, g: Glyph, alpha: f32, tip: &str) -> egui::Response {
    let rect = Rect::from_center_size(c, Vec2::splat(20.0));
    let resp = ui.interact(rect, id, Sense::click()).on_hover_text(tip);
    let bg = if resp.hovered() { theme::surface_3() } else { theme::surface_2() };
    ui.painter().rect_filled(rect, CornerRadius::same(5), bg.gamma_multiply(alpha));
    paint_glyph(ui.painter(), c, g, theme::text().gamma_multiply(alpha));
    resp
}

fn plus(p: &egui::Painter, c: Pos2, r: f32, s: Stroke) {
    p.line_segment([c - Vec2::new(r, 0.0), c + Vec2::new(r, 0.0)], s);
    p.line_segment([c - Vec2::new(0.0, r), c + Vec2::new(0.0, r)], s);
}

/// Chevron rotating from "▸" (0) to "▾" (1).
fn chevron(p: &egui::Painter, c: Pos2, openness: f32, col: egui::Color32) {
    let a = openness * std::f32::consts::FRAC_PI_2;
    let rot = |v: Vec2| Vec2::new(v.x * a.cos() - v.y * a.sin(), v.x * a.sin() + v.y * a.cos());
    let pts = [Vec2::new(-1.8, -3.6), Vec2::new(2.2, 0.0), Vec2::new(-1.8, 3.6)].map(|v| c + rot(v));
    p.add(egui::Shape::line(pts.to_vec(), Stroke::new(1.5, col)));
}

/// Folder outline; the flap lifts as it opens.
fn folder_icon(p: &egui::Painter, r: Rect, openness: f32, col: egui::Color32) {
    let at = |x: f32, y: f32| Pos2::new(r.left() + x * r.width(), r.top() + y * r.height());
    let s = Stroke::new(1.4, col);
    p.rect_filled(Rect::from_min_max(at(0.0, 0.25), at(1.0, 1.0)), CornerRadius::same(2), col.gamma_multiply(0.16));
    p.add(egui::Shape::line(vec![at(0.0, 0.95), at(0.0, 0.05), at(0.38, 0.05), at(0.5, 0.22), at(1.0, 0.22), at(1.0, 0.95), at(0.0, 0.95)], s));
    if openness > 0.0 {
        let lift = 0.25 * openness;
        p.add(egui::Shape::line(
            vec![at(0.0, 0.95), at(0.12 + lift * 0.2, 0.45 - lift * 0.3), at(1.0 + lift * 0.1, 0.45 - lift * 0.3), at(1.0, 0.95)],
            Stroke::new(1.4, col.gamma_multiply(openness)),
        ));
    }
}

fn badge(p: &egui::Painter, right_center: Pos2, n: usize, alpha: f32) {
    let text = n.to_string();
    let galley = p.layout_no_wrap(text, FontId::proportional(10.5), theme::text_faint());
    let w = galley.size().x + 10.0;
    let rect = Rect::from_center_size(right_center - Vec2::new(w / 2.0 - 4.0, 0.0), Vec2::new(w, 16.0));
    p.rect_filled(rect, CornerRadius::same(8), theme::surface_3().gamma_multiply(0.7 * alpha));
    p.galley(rect.center() - galley.size() / 2.0, galley, theme::text_faint().gamma_multiply(alpha));
}
