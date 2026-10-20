//! Window arrangement: activity bar, sidebar sections, the vertical tabs panel, and
//! tab / pane operations (focus, move, split, zoom, rename). A child module of `app`.

use eframe::egui::{self, RichText};
use egui_dock::{DockState, Node, NodePath, TabIndex, TabInsert, TabViewer};

use super::TetherApp;
use crate::keybindings::{self, Action};
use crate::layout::{Direction, LayoutSettings, Side, SidebarSection, TabPosition};
use crate::profile::secret;
use crate::settings;
use crate::theme;
use crate::ui::agents_panel;
use crate::ui::settings_view::{Section, SettingsView};
use crate::ui::sidebar::{Sidebar, SidebarAction, SidebarData};
use crate::ui::sidebar_sections::{self as sections, ActivityEvent, BodySize, SectionEvent, SectionHost};
use crate::ui::tabs::{PaneFx, Tab, TabBody, Viewer};
use crate::ui::vertical_tabs::{ListOptions, TabAction, TabTarget, VerticalTabs};
use crate::workspace::TabKind;

/// Max share of the sidebar the Sessions section takes before it scrolls.
const SESSIONS_MAX_SHARE: f32 = 0.4;
const SESSIONS_EMPTY_H: f32 = 34.0;
const AGENTS_MAX_SHARE: f32 = 0.45;

/// Gives the sidebar sections their content.
struct Host<'a, 'd> {
    sidebar: &'a mut Sidebar,
    data: &'a SidebarData<'d>,
    dock: &'a mut DockState<Tab>,
    vtabs: &'a mut VerticalTabs,
    opts: ListOptions,
    actions: Vec<SidebarAction>,
    tab_actions: Vec<TabAction>,
    /// AI agent sessions for the "Agents" section, and what was clicked there.
    agents: &'a [crate::agents::SessionInfo],
    agent_action: Option<crate::ui::agents_panel::PanelAction>,
}

impl SectionHost for Host<'_, '_> {
    fn size(&self, kind: SidebarSection, avail: f32) -> BodySize {
        match kind {
            SidebarSection::Hosts => BodySize::Flex,
            SidebarSection::Sessions => {
                let h = VerticalTabs::list_height(self.dock, self.opts);
                BodySize::Fixed(if h <= 0.0 { SESSIONS_EMPTY_H } else { h.min(avail * SESSIONS_MAX_SHARE) + 4.0 })
            }
            SidebarSection::Workspaces => BodySize::Fixed(self.sidebar.workspaces_height(self.data)),
            SidebarSection::Agents => BodySize::Fixed(agents_panel::list_height(self.agents).min(avail * AGENTS_MAX_SHARE)),
        }
    }

    fn count(&self, kind: SidebarSection) -> Option<usize> {
        match kind {
            SidebarSection::Hosts => Some(Sidebar::host_count(self.data)),
            SidebarSection::Sessions => Some(self.dock.iter_all_tabs().filter(|(p, _)| p.surface.is_main()).count()),
            SidebarSection::Workspaces => Some(self.data.workspaces.len()),
            SidebarSection::Agents => Some(agents_panel::waiting(self.agents)),
        }
    }

    fn header_extra(&mut self, kind: SidebarSection, ui: &mut egui::Ui) {
        match kind {
            SidebarSection::Workspaces => self.sidebar.workspaces_header(ui, self.data),
            SidebarSection::Sessions => {
                let tip = keybindings::hint("New terminal", Action::NewLocalTerminal);
                if ui.add(egui::Button::new(RichText::new("+").size(14.0)).frame(false)).on_hover_text(tip).clicked() {
                    self.tab_actions.push(TabAction::NewShell(None));
                }
            }
            SidebarSection::Hosts | SidebarSection::Agents => {}
        }
    }

    fn body(&mut self, kind: SidebarSection, ui: &mut egui::Ui) {
        match kind {
            SidebarSection::Hosts => {
                let a = self.sidebar.hosts_body(ui, self.data);
                self.actions.extend(a);
            }
            SidebarSection::Sessions => {
                if self.dock.iter_all_tabs().next().is_none() {
                    ui.label(RichText::new("   No open sessions").size(11.5).color(theme::text_faint()));
                    return;
                }
                egui::ScrollArea::vertical().id_salt("sidebar_sessions").max_height(ui.available_height()).auto_shrink([false, true]).show(ui, |ui| {
                    let a = self.vtabs.list(ui, self.dock, self.opts);
                    self.tab_actions.extend(a);
                });
            }
            SidebarSection::Workspaces => {
                let a = self.sidebar.workspaces_body(ui, self.data);
                self.actions.extend(a);
            }
            SidebarSection::Agents => {
                self.agent_action = agents_panel::list(ui, self.agents, std::time::Instant::now());
                // Elapsed times tick while sessions are listed.
                if !self.agents.is_empty() {
                    ui.ctx().request_repaint_after(std::time::Duration::from_secs(1));
                }
            }
        }
    }
}

fn list_options(l: &LayoutSettings) -> ListOptions {
    ListOptions { grouped: l.group_by_pane, compact: l.compact_tabs }
}

fn side_panel(id: &'static str, side: Side) -> egui::Panel {
    match side {
        Side::Left => egui::Panel::left(id),
        Side::Right => egui::Panel::right(id),
    }
}

impl TetherApp {
    /// Activity bar, sidebar and the vertical tabs panel, in that order from the edge.
    pub(super) fn side_panels(&mut self, ui: &mut egui::Ui) {
        let l = settings::snapshot().layout.clone();
        if l.activity_bar {
            self.activity_bar(ui, &l);
        }
        self.sidebar_panel(ui, &l);
        if l.tab_position.is_vertical() {
            self.tabs_panel(ui, &l);
        }
    }

    fn activity_bar(&mut self, ui: &mut egui::Ui, l: &LayoutSettings) {
        let frame = egui::Frame::new().fill(theme::bg());
        let entries: Vec<_> = l.sections().into_iter().filter(|e| e.kind != SidebarSection::Sessions || l.sessions_in_sidebar()).collect();
        let tabs = self.dock.iter_all_tabs().count();
        let workspaces = self.workspaces.names().len();
        let badge = move |k: SidebarSection| match k {
            SidebarSection::Sessions => Some(tabs),
            SidebarSection::Workspaces if workspaces > 0 => None,
            _ => None,
        };
        let on_right = l.sidebar_side == Side::Right;
        let event = side_panel("activity_bar", l.sidebar_side)
            .exact_size(sections::ACTIVITY_W)
            .resizable(false)
            .show_separator_line(false)
            .frame(frame)
            .show(ui, |ui| {
                let r = ui.max_rect();
                let x = if on_right { r.left() + 0.5 } else { r.right() - 0.5 };
                ui.painter().vline(x, r.y_range(), egui::Stroke::new(1.0, theme::border().gamma_multiply(0.5)));
                sections::activity_bar(ui, &entries, l.active_view, l.sidebar_visible, &badge, on_right)
            })
            .inner;
        match event {
            Some(ActivityEvent::Show(kind)) => {
                let _ = settings::update(|s| {
                    s.layout.active_view = kind;
                    s.layout.sidebar_visible = true;
                });
            }
            Some(ActivityEvent::ToggleSidebar) => toggle_sidebar(),
            Some(ActivityEvent::OpenSettings) => {
                self.settings_view.get_or_insert_with(SettingsView::new);
            }
            Some(ActivityEvent::OpenPalette) => self.open_palette(),
            None => {}
        }
    }

    fn sidebar_panel(&mut self, ui: &mut egui::Ui, l: &LayoutSettings) {
        let margin = match l.sidebar_side {
            Side::Left => egui::Margin { left: 12, right: 8, top: 4, bottom: 10 },
            Side::Right => egui::Margin { left: 8, right: 12, top: 4, bottom: 10 },
        };
        let frame = egui::Frame::new().fill(theme::bg()).inner_margin(margin);
        let workspaces = self.workspace_infos();
        let pool = self.pool.clone();
        let is_live = move |id| pool.has_live(id);
        let has_tabs = self.layout_specs().is_some();
        let shells = crate::pty::shells::detected().unwrap_or_default();
        let data = SidebarData { store: &self.store, folders: &self.folders, workspaces: &workspaces, has_tabs, is_live: &is_live, shells };
        let sessions = self.agents.sessions();
        let mut host = Host {
            sidebar: &mut self.sidebar,
            data: &data,
            dock: &mut self.dock,
            vtabs: &mut self.vtabs,
            opts: list_options(l),
            actions: Vec::new(),
            tab_actions: Vec::new(),
            agents: sessions,
            agent_action: None,
        };
        let mut visible = l.sidebar_visible;
        let entries: Vec<_> = l.sections().into_iter().filter(|e| e.kind != SidebarSection::Sessions || l.sessions_in_sidebar()).collect();
        let single = l.activity_bar.then(|| if entries.iter().any(|e| e.kind == l.active_view) { l.active_view } else { SidebarSection::Hosts });
        let shown = side_panel("sidebar", l.sidebar_side)
            .resizable(true)
            .default_size(l.sidebar_width())
            .size_range(LayoutSettings::SIDEBAR_MIN..=LayoutSettings::SIDEBAR_MAX)
            .frame(frame)
            .show_separator_line(false)
            .show_collapsible(ui, &mut visible, |ui| {
                let title = host.sidebar.title_bar(ui, host.data);
                host.actions.extend(title);
                match single {
                    Some(kind) => {
                        sections::single(ui, kind, &mut host);
                        Vec::new()
                    }
                    None => sections::stacked(ui, &entries, &mut host),
                }
            });
        let (actions, tab_actions, agent_action) = (std::mem::take(&mut host.actions), std::mem::take(&mut host.tab_actions), host.agent_action.take());
        drop(host);
        if let Some(r) = &shown {
            // A hairline between the sidebar and what is next to it.
            let edge = r.response.rect;
            let x = if l.sidebar_side == Side::Left { edge.right() - 0.5 } else { edge.left() + 0.5 };
            ui.painter().vline(x, edge.y_range().shrink(8.0), egui::Stroke::new(1.0, theme::border().gamma_multiply(0.35)));
            persist_width(r.response.rect.width() - f32::from(margin.left + margin.right), l.sidebar_width(), ui.ctx(), |s, w| s.layout.sidebar_width = w);
        }
        let events = shown.map(|r| r.inner).unwrap_or_default();
        if visible != l.sidebar_visible {
            let _ = settings::update(|s| s.layout.sidebar_visible = visible);
        }
        apply_section_events(events);
        for a in actions {
            self.apply_sidebar(a, ui.ctx());
        }
        self.apply_tab_actions(tab_actions, ui.ctx());
        self.apply_agents_panel(agent_action, ui.ctx());
    }

    fn tabs_panel(&mut self, ui: &mut egui::Ui, l: &LayoutSettings) {
        let side = if l.tab_position == TabPosition::Right { Side::Right } else { Side::Left };
        let frame = egui::Frame::new().fill(theme::bg()).inner_margin(egui::Margin { left: 6, right: 6, top: 4, bottom: 8 });
        let resp = side_panel("vertical_tabs", side)
            .resizable(true)
            .default_size(l.tabs_width())
            .size_range(LayoutSettings::TABS_MIN..=LayoutSettings::TABS_MAX)
            .frame(frame)
            .show_separator_line(false)
            .show(ui, |ui| self.vtabs.panel(ui, &mut self.dock, list_options(l), l.tab_position));
        persist_width(resp.response.rect.width() - 12.0, l.tabs_width(), ui.ctx(), |s, w| s.layout.tabs_width = w);
        self.apply_tab_actions(resp.inner, ui.ctx());
    }

    // ---- tab operations -------------------------------------------------------------

    pub(super) fn apply_tab_actions(&mut self, actions: Vec<TabAction>, ctx: &egui::Context) {
        for a in actions {
            match a {
                TabAction::Focus(uid) => self.show_tab(uid),
                TabAction::Close(uid) => self.close_tab(uid),
                TabAction::CloseOthers(uid) => {
                    let others: Vec<u64> = self.dock.iter_all_tabs().filter(|(p, t)| p.surface.is_main() && t.uid != uid).map(|(_, t)| t.uid).collect();
                    for other in others {
                        self.close_tab(other);
                    }
                    self.show_tab(uid);
                }
                TabAction::StartRename(uid) => {
                    if let Some(tab) = self.dock.iter_all_tabs_mut().map(|(_, t)| t).find(|t| t.uid == uid) {
                        self.rename = Some((uid, tab.display_title()));
                    }
                }
                TabAction::Rename(uid, name) => {
                    if let Some(tab) = self.dock.iter_all_tabs_mut().map(|(_, t)| t).find(|t| t.uid == uid) {
                        tab.rename(name);
                    }
                    // Custom titles are part of the saved layout.
                    self.last_layout.clear();
                }
                TabAction::Duplicate(uid) => {
                    if let (Some(tab), Some(path)) = (self.duplicate_of(uid, ctx), self.dock.find_tab_from(|t| t.uid == uid)) {
                        let new_uid = tab.uid;
                        if let Ok(leaf) = self.dock.leaf_mut(path.node_path()) {
                            leaf.append_tab(tab);
                        }
                        self.show_tab(new_uid);
                    }
                }
                TabAction::SplitWith(uid, split) => {
                    if let (Some(tab), Some(path)) = (self.duplicate_of(uid, ctx), self.dock.find_tab_from(|t| t.uid == uid)) {
                        let new_uid = tab.uid;
                        self.dock.split(path.node_path(), split, 0.5, Node::leaf_with(vec![tab]));
                        self.show_tab(new_uid);
                    }
                }
                TabAction::PopOut(uid) => {
                    if let Some(path) = self.dock.find_tab_from(|t| t.uid == uid)
                        && let Some(tab) = self.dock.remove_tab(path)
                    {
                        let origin = ctx.input(|i| i.viewport().inner_rect).map_or(egui::pos2(120.0, 120.0), |r| r.min + egui::vec2(120.0, 90.0));
                        self.detached.pop_out(tab, origin);
                    }
                }
                TabAction::Move { uid, target } => self.move_tab(uid, target),
                TabAction::NewShell(id) => {
                    let spec = id.as_deref().and_then(crate::pty::shells::find);
                    self.open_local(spec, ctx);
                }
                TabAction::SetGrouped(on) => {
                    let _ = settings::update(|s| s.layout.group_by_pane = on);
                }
                TabAction::SetCompact(on) => {
                    let _ = settings::update(|s| s.layout.compact_tabs = on);
                }
                TabAction::SetPosition(p) => set_tab_position(p),
            }
        }
    }

    /// Shows `uid` in its pane, focuses that pane and gives its terminal the keyboard.
    pub(super) fn show_tab(&mut self, uid: u64) {
        let Some(path) = self.dock.find_tab_from(|t| t.uid == uid) else { return };
        let _ = self.dock.set_active_tab(path);
        self.dock.set_focused_node_and_surface(path.node_path());
        self.focus_request = Some(uid);
        if self.zoomed.is_some() {
            self.zoomed = Some(uid);
        }
    }

    /// Closes a tab of the main dock the way its ✕ would (editors may refuse).
    pub(super) fn close_tab(&mut self, uid: u64) {
        let Some(path) = self.dock.find_tab_from(|t| t.uid == uid) else { return };
        let Some(tab) = self.dock.iter_all_tabs_mut().map(|(_, t)| t).find(|t| t.uid == uid) else { return };
        let close = match &mut tab.body {
            TabBody::Terminal(t) => {
                t.handle.close();
                true
            }
            TabBody::Editor(e) => e.request_close(),
            TabBody::Sftp(_) => true,
        };
        if close {
            self.dock.remove_tab(path);
        }
    }

    /// A new tab running the same thing as `uid` (local shells start in its folder).
    fn duplicate_of(&mut self, uid: u64, ctx: &egui::Context) -> Option<Tab> {
        let tab = self.dock.iter_all_tabs().map(|(_, t)| t).find(|t| t.uid == uid)?;
        if let Some((spec, cwd)) = super::local::origin(tab) {
            return Some(self.local_tab(&spec, cwd, ctx));
        }
        let kind = match tab.body {
            TabBody::Terminal(_) => TabKind::Terminal,
            TabBody::Sftp(_) => TabKind::Sftp,
            TabBody::Editor(_) => return None,
        };
        let profile = self.store.get(tab.profile.id).cloned().unwrap_or_else(|| tab.profile.clone());
        let secret = secret::get(profile.secret_id());
        Some(match kind {
            TabKind::Sftp => self.sftp_tab(&profile, secret, ctx),
            _ => self.terminal_tab(&profile, secret, ctx),
        })
    }

    fn move_tab(&mut self, uid: u64, target: TabTarget) {
        let Some(src) = self.dock.find_tab_from(|t| t.uid == uid) else { return };
        let same_leaf = |leaf: NodePath| src.node_path() == leaf;
        match target {
            TabTarget::Insert { leaf, index } => {
                // `index` counts the dragged tab when it sits before the drop point.
                let index = if same_leaf(leaf) && src.tab.0 < index { index - 1 } else { index };
                if same_leaf(leaf) && src.tab.0 == index {
                    return;
                }
                if self.dock.leaf(leaf).is_err() {
                    return;
                }
                self.dock.move_tab(src, (leaf, TabInsert::Insert(TabIndex(index))));
            }
            TabTarget::Append(leaf) => {
                if same_leaf(leaf) || self.dock.leaf(leaf).is_err() {
                    return;
                }
                self.dock.move_tab(src, (leaf, TabInsert::Append));
            }
            TabTarget::Split { leaf, split } => {
                if self.dock.leaf(leaf).is_err() {
                    return;
                }
                self.dock.move_tab(src, (leaf, TabInsert::Split(split)));
            }
        }
        self.show_tab(uid);
    }

    // ---- panes ---------------------------------------------------------------------

    /// Main-surface leaves that hold tabs, with their rects (as of the last frame).
    fn panes(&self) -> Vec<(NodePath, egui::Rect)> {
        self.dock.iter_leaves().filter(|(p, l)| p.surface.is_main() && !l.tabs.is_empty()).map(|(p, l)| (p, l.rect)).collect()
    }

    pub(super) fn focus_pane(&mut self, dir: Direction) {
        let panes = self.panes();
        let Some(cur) = self.dock.focused_leaf().filter(|p| p.surface.is_main()).or_else(|| panes.first().map(|p| p.0)) else { return };
        let Some(cur_rect) = panes.iter().find(|p| p.0 == cur).map(|p| p.1) else { return };
        let rects: Vec<egui::Rect> = panes.iter().map(|p| p.1).collect();
        let Some(i) = crate::layout::neighbor(cur_rect, &rects, dir) else { return };
        let target = panes[i].0;
        self.dock.set_focused_node_and_surface(target);
        if let Ok(leaf) = self.dock.leaf(target) {
            self.focus_request = leaf.tabs.get(leaf.active.0).map(|t| t.uid);
        }
    }

    pub(super) fn toggle_zoom(&mut self) {
        if self.zoomed.take().is_some() {
            if let Some(uid) = self.dock.find_active_focused().map(|(_, t)| t.uid) {
                self.focus_request = Some(uid);
            }
            return;
        }
        if self.panes().len() < 2 {
            self.toasts.info("Maximize needs a split: there is only one pane");
            return;
        }
        self.zoomed = self.dock.find_active_focused().map(|(_, t)| t.uid);
        self.focus_request = self.zoomed;
    }

    /// The tab of the focused pane (what pane-wide commands act on).
    pub(super) fn active_uid(&self) -> Option<u64> {
        let path = self.dock.focused_leaf()?;
        let leaf = self.dock.leaf(path).ok()?;
        leaf.tabs.get(leaf.active.0).map(|t| t.uid)
    }

    /// Gives the keyboard to a tab that just opened in the focused pane (new shell,
    /// split, connect), the way a new terminal window would have it.
    pub(super) fn focus_new_tab(&mut self) {
        let now: std::collections::HashSet<u64> = self.dock.iter_all_tabs().filter(|(p, _)| p.surface.is_main()).map(|(_, t)| t.uid).collect();
        if let Some(active) = self.active_uid()
            && !self.seen_tabs.contains(&active)
            && self.focus_request.is_none()
        {
            self.focus_request = Some(active);
        }
        self.seen_tabs = now;
    }

    /// Dimming / outline / focus inputs for this frame's tab views. Also hands a
    /// pending keyboard-focus request to its terminal (`TerminalView::want_focus`).
    pub(super) fn pane_fx(&mut self, ctx: &egui::Context) -> PaneFx {
        if let Some(uid) = self.focus_request.take()
            && let Some(TabBody::Terminal(view)) = self.dock.iter_all_tabs_mut().map(|(_, t)| t).find(|t| t.uid == uid).map(|t| &mut t.body)
        {
            view.want_focus = true;
        }
        let l = &settings::snapshot().layout;
        let overlay_open = self.palette.is_some() || self.settings_view.is_some() || ctx.memory(|m| m.top_modal_layer().is_some());
        PaneFx {
            active_uid: self.active_uid(),
            multi_pane: self.panes().len() > 1 && self.zoomed.is_none(),
            dim: if l.dim_inactive_panes { l.dim_amount() } else { 0.0 },
            outline: l.tab_position.is_vertical(),
            focus_follows_mouse: l.focus_follows_mouse && !overlay_open,
            terminal_ids: std::mem::take(&mut self.terminal_ids),
        }
    }

    /// Applies what the views reported about focus (hover focus, terminal ids).
    pub(super) fn after_views(&mut self, viewer: &mut Viewer) {
        self.terminal_ids = std::mem::take(&mut viewer.terminal_ids);
        if let Some(uid) = viewer.hover_focus.take()
            && let Some(path) = self.dock.find_tab_from(|t| t.uid == uid)
        {
            self.dock.set_focused_node_and_surface(path.node_path());
            self.focus_request = Some(uid);
        }
    }

    /// Shows only the maximized tab. Returns false when nothing is zoomed (or it closed).
    pub(super) fn zoomed_view(&mut self, ui: &mut egui::Ui, viewer: &mut Viewer) -> bool {
        let Some(uid) = self.zoomed else { return false };
        let Some(tab) = self.dock.iter_all_tabs_mut().filter(|(p, _)| p.surface.is_main()).map(|(_, t)| t).find(|t| t.uid == uid) else {
            self.zoomed = None;
            return false;
        };
        let rect = ui.available_rect_before_wrap().shrink(6.0);
        let see_through = crate::themes::see_through();
        let fill = if see_through { theme::surface().gamma_multiply(crate::themes::window_opacity()) } else { theme::surface() };
        ui.painter().rect(rect, egui::CornerRadius::same(theme::RADIUS), fill, egui::Stroke::new(1.0, theme::border().gamma_multiply(0.55)), egui::StrokeKind::Inside);
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect).id_salt(("zoomed", uid)));
        child.set_clip_rect(rect);
        viewer.ui(&mut child, tab);
        // "Maximized" pill, top right: click (or the shortcut) restores the split.
        let text = format!("Maximized  ·  {} to restore", keybindings::label(Action::ZoomPane));
        let galley = ui.painter().layout_no_wrap(text, egui::FontId::proportional(11.5), theme::text());
        let pill = egui::Rect::from_min_size(egui::pos2(rect.right() - galley.size().x - 34.0, rect.top() + 8.0), galley.size() + egui::vec2(22.0, 10.0));
        let resp = ui.interact(pill, egui::Id::new("zoom_pill"), egui::Sense::click()).on_hover_text("Restore the split layout");
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Maximized pane: restore"));
        let h = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), theme::anim_time(0.1));
        let fg = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("zoom_pill_layer")));
        fg.rect(pill, egui::CornerRadius::same(11), theme::surface_2().lerp_to_gamma(theme::surface_3(), h), egui::Stroke::new(1.0, theme::accent().gamma_multiply(0.5)), egui::StrokeKind::Inside);
        fg.galley(pill.min + egui::vec2(11.0, 5.0), galley, theme::text());
        if resp.clicked() {
            self.toggle_zoom();
        }
        true
    }

    pub(super) fn rename_modal(&mut self, ctx: &egui::Context) {
        let Some((uid, text)) = &mut self.rename else { return };
        let uid = *uid;
        let (mut done, mut cancel) = (false, false);
        let modal = egui::Modal::new(egui::Id::new("rename_tab")).show(ctx, |ui| {
            ui.set_width(320.0);
            ui.heading("Rename tab");
            ui.add_space(6.0);
            let edit = ui.add(egui::TextEdit::singleline(text).hint_text("Leave empty for the automatic title").desired_width(f32::INFINITY));
            if !edit.has_focus() && !edit.lost_focus() {
                edit.request_focus();
            }
            done |= edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                done |= ui.add(egui::Button::new("Rename").fill(theme::accent_soft())).clicked();
                cancel |= ui.button("Cancel").clicked();
            });
        });
        if cancel || modal.should_close() {
            self.rename = None;
        } else if done {
            let name = self.rename.take().map(|(_, t)| t);
            self.apply_tab_actions(vec![TabAction::Rename(uid, name)], ctx);
            self.focus_request = Some(uid);
        }
    }

    pub(super) fn welcome(&mut self, ui: &mut egui::Ui) {
        use crate::ui::welcome::{self, WelcomeAction};
        match welcome::show(ui, &self.store.profiles) {
            Some(WelcomeAction::Shell(s)) => self.open_local(Some(s), ui.ctx()),
            Some(WelcomeAction::Open(p)) => self.request_open(p, crate::ui::sidebar::OpenKind::Terminal, ui.ctx()),
            Some(WelcomeAction::NewHost) => self.open_editor_new(None),
            Some(WelcomeAction::Import) => self.import = Some(crate::ui::import_view::ImportDialog::new(&self.store)),
            Some(WelcomeAction::Palette) => self.open_palette(),
            None => {}
        }
    }

    pub(super) fn open_layout_settings(&mut self) {
        self.settings_view = Some(SettingsView::at(Section::Layout));
    }
}

/// Saves a panel's width once the user lets go of its edge.
fn persist_width(width: f32, saved: f32, ctx: &egui::Context, set: impl FnOnce(&mut settings::Settings, f32)) {
    if (width - saved).abs() > 1.0 && !ctx.input(|i| i.pointer.any_down()) {
        let _ = settings::update(|s| set(s, width.round()));
    }
}

fn apply_section_events(events: Vec<SectionEvent>) {
    if events.is_empty() {
        return;
    }
    let _ = settings::update(|s| {
        for e in events {
            match e {
                SectionEvent::SetCollapsed(kind, c) => s.layout.set_collapsed(kind, c),
                SectionEvent::Move { from, to } => {
                    // Indices count the sections shown, which may skip Sessions.
                    let shown: Vec<SidebarSection> = s.layout.sections().into_iter().filter(|e| e.kind != SidebarSection::Sessions || s.layout.sessions_in_sidebar()).map(|e| e.kind).collect();
                    let all: Vec<SidebarSection> = s.layout.sections().into_iter().map(|e| e.kind).collect();
                    let Some(kind) = shown.get(from) else { continue };
                    let Some(from_all) = all.iter().position(|k| k == kind) else { continue };
                    let to_all = shown.get(to).and_then(|k| all.iter().position(|a| a == k)).unwrap_or(all.len());
                    s.layout.move_section(from_all, to_all);
                }
            }
        }
    });
}

pub(super) fn toggle_sidebar() {
    let _ = settings::update(|s| s.layout.sidebar_visible = !s.layout.sidebar_visible);
}

pub(super) fn set_tab_position(p: TabPosition) {
    let _ = settings::update(|s| s.layout.tab_position = p);
}

pub(super) fn set_sidebar_side(side: Side) {
    let _ = settings::update(|s| s.layout.sidebar_side = side);
}

/// The dock with tab bars hidden in vertical mode; returns whether they are hidden.
pub(super) fn sync_tab_bars(dock: &mut DockState<Tab>) -> bool {
    let hide = settings::snapshot().layout.tab_position.is_vertical();
    for (_, leaf) in dock.iter_leaves_mut() {
        leaf.tab_bar_hidden = hide;
    }
    hide
}
