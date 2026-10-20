//! Vertical tabs: every open tab as a row (icon, title, folder or address, status),
//! optionally grouped by pane. Rows can be clicked, renamed (double-click), closed
//! (hover ✕ or middle-click), reordered or moved between panes by dragging, and dropped
//! onto the panes to split them. Used by the Sessions panel and the sidebar section.

use eframe::egui::{self, Align2, CornerRadius, FontId, Id, Pos2, Rect, Sense, Stroke, Ui, UiBuilder, Vec2};
use egui_dock::{DockState, NodePath, Split};

use super::tab_badges::TabBadges;
use super::tabs::{Tab, TabBody};
use crate::keybindings::{self, Action};
use crate::layout::TabPosition;
use crate::theme;

const ROW_H: f32 = 44.0;
const ROW_H_COMPACT: f32 = 30.0;
const GROUP_H: f32 = 26.0;
const TILE: f32 = 24.0;
/// Share of a pane's width / height near an edge that splits instead of joining.
const EDGE: f32 = 0.28;

/// Where a dragged tab goes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TabTarget {
    /// Into `leaf` at `index` (counted before the dragged tab is removed).
    Insert { leaf: NodePath, index: usize },
    Append(NodePath),
    /// A new pane next to `leaf`.
    Split { leaf: NodePath, split: Split },
}

/// What the tab list (or a dock tab's context menu) asks the app to do.
#[derive(Debug, Clone, PartialEq)]
pub enum TabAction {
    Focus(u64),
    Close(u64),
    CloseOthers(u64),
    /// Ask for a new name (a small dialog; the list renames inline instead).
    StartRename(u64),
    Rename(u64, Option<String>),
    Duplicate(u64),
    /// The same session again, in a new pane next to the tab's pane.
    SplitWith(u64, Split),
    /// Into its own native window.
    PopOut(u64),
    Move { uid: u64, target: TabTarget },
    /// A local shell by id; `None` = the default shell.
    NewShell(Option<String>),
    SetGrouped(bool),
    SetCompact(bool),
    SetPosition(TabPosition),
}

/// Drag-and-drop payload of a tab row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TabDrag(pub u64);

#[derive(Debug, Clone, Copy)]
pub struct ListOptions {
    pub grouped: bool,
    pub compact: bool,
}

/// A row as rendered (collected first, so drawing doesn't borrow the dock).
struct RowModel {
    uid: u64,
    title: String,
    subtitle: String,
    icon: String,
    hue: f32,
    status: egui::Color32,
    leaf: NodePath,
    index: usize,
    /// The visible tab of its pane.
    visible: bool,
    /// The visible tab of the focused pane.
    focused: bool,
}

struct Group {
    leaf: NodePath,
    focused: bool,
    rows: Vec<RowModel>,
}

#[derive(Default)]
pub struct VerticalTabs {
    renaming: Option<(u64, String)>,
    badges: TabBadges,
}

impl VerticalTabs {
    pub fn start_rename(&mut self, uid: u64, current: String) {
        self.renaming = Some((uid, current));
    }

    /// Height the list needs with `tabs` rows in `panes` panes.
    pub fn list_height(dock: &DockState<Tab>, opts: ListOptions) -> f32 {
        let leaves: Vec<usize> = dock.iter_leaves().filter(|(p, _)| p.surface.is_main()).map(|(_, l)| l.tabs.len()).filter(|n| *n > 0).collect();
        let rows: usize = leaves.iter().sum();
        let headers = if opts.grouped && leaves.len() > 1 { leaves.len() as f32 * GROUP_H } else { 0.0 };
        rows as f32 * row_h(opts) + headers
    }

    /// The Sessions panel: header with "+" and options, the list, an empty state.
    pub fn panel(&mut self, ui: &mut Ui, dock: &mut DockState<Tab>, opts: ListOptions, position: TabPosition) -> Vec<TabAction> {
        let mut actions = Vec::new();
        let count = dock.iter_all_tabs().filter(|(p, _)| p.surface.is_main()).count();
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.set_height(28.0);
            ui.add_space(4.0);
            ui.label(egui::RichText::new("SESSIONS").size(10.5).color(theme::text_faint()));
            if count > 0 {
                ui.label(egui::RichText::new(count.to_string()).size(10.5).color(theme::text_faint().gamma_multiply(0.7)));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                let more = small_button(ui, "", "Tab list options");
                egui::Popup::menu(&more).show(|ui| options_menu(ui, opts, position, &mut actions));
                let add = small_button(ui, "+", &keybindings::hint("New terminal", Action::NewLocalTerminal));
                if add.clicked() {
                    actions.push(TabAction::NewShell(None));
                }
                add.context_menu(|ui| shells_menu(ui, &mut actions));
            });
        });
        ui.add_space(4.0);
        if count == 0 {
            empty_state(ui, &mut actions);
            return actions;
        }
        egui::ScrollArea::vertical().id_salt("vtabs_scroll").auto_shrink([false, false]).show(ui, |ui| {
            self.list_into(ui, dock, opts, &mut actions);
            // Space below the rows takes drops "to the end of the last pane".
            let rest = ui.available_rect_before_wrap();
            if rest.height() > 8.0 {
                let resp = ui.allocate_rect(rest, Sense::click());
                if let Some(leaf) = last_leaf(dock) {
                    drop_zone(ui, &resp, TabTarget::Append(leaf), &mut actions);
                }
                resp.context_menu(|ui| {
                    shells_menu(ui, &mut actions);
                    ui.separator();
                    options_menu(ui, opts, position, &mut actions);
                });
            }
        });
        actions
    }

    /// Just the rows (the sidebar section scrolls them itself).
    pub fn list(&mut self, ui: &mut Ui, dock: &mut DockState<Tab>, opts: ListOptions) -> Vec<TabAction> {
        let mut actions = Vec::new();
        self.list_into(ui, dock, opts, &mut actions);
        actions
    }

    fn list_into(&mut self, ui: &mut Ui, dock: &mut DockState<Tab>, opts: ListOptions, actions: &mut Vec<TabAction>) {
        ui.spacing_mut().item_spacing.y = 0.0;
        let groups = collect(dock);
        let headers = opts.grouped && groups.len() > 1;
        for (gi, group) in groups.iter().enumerate() {
            if headers {
                if gi > 0 {
                    ui.add_space(2.0);
                }
                let resp = group_header(ui, gi, group);
                drop_zone(ui, &resp, TabTarget::Append(group.leaf), actions);
            }
            for row in &group.rows {
                let tab = dock.iter_all_tabs().find(|(_, t)| t.uid == row.uid).map(|(_, t)| t);
                self.row(ui, row, tab, opts, actions);
            }
        }
        drag_ghost(ui, &groups);
    }

    fn row(&mut self, ui: &mut Ui, row: &RowModel, tab: Option<&Tab>, opts: ListOptions, actions: &mut Vec<TabAction>) {
        let h = row_h(opts);
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), h), Sense::click_and_drag());
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, format!("Tab {}", row.title)));
        if resp.drag_started() {
            resp.dnd_set_drag_payload(TabDrag(row.uid));
        }
        let ctx = ui.ctx().clone();
        let dragging_me = egui::DragAndDrop::payload::<TabDrag>(&ctx).is_some_and(|d| d.0 == row.uid);
        let any_drag = egui::DragAndDrop::has_payload_of_type::<TabDrag>(&ctx);
        let hot = (ui.rect_contains_pointer(rect) || resp.context_menu_opened()) && !any_drag;
        let hover = ctx.animate_bool_with_time(resp.id.with("h"), hot, theme::anim_time(0.12));
        let sel = ctx.animate_bool_with_time(resp.id.with("sel"), row.visible, theme::anim_time(0.16));
        let foc = ctx.animate_bool_with_time(resp.id.with("foc"), row.focused, theme::anim_time(0.16));
        let alpha = if dragging_me { 0.45 } else { 1.0 };
        let p = ui.painter().clone();
        let card = rect.shrink2(Vec2::new(0.0, 1.0));
        let radius = CornerRadius::same(theme::RADIUS);
        let fill = theme::surface_2().gamma_multiply((0.55 * sel + 0.45 * foc).max(hover * 0.8));
        if fill.a() > 0 {
            p.rect_filled(card, radius, fill);
        }
        if foc > 0.0 {
            let bar = Rect::from_center_size(Pos2::new(card.left() + 1.5, card.center().y), Vec2::new(3.0, (h - 18.0) * foc));
            p.rect_filled(bar, CornerRadius::same(2), theme::accent().gamma_multiply(foc));
        }

        // Icon tile with the host / shell colour; a status dot on its corner.
        let tint = theme::hue_color(row.hue);
        let tile = Rect::from_center_size(Pos2::new(card.left() + 10.0 + TILE / 2.0, card.center().y), Vec2::splat(if opts.compact { 20.0 } else { TILE }));
        p.rect_filled(tile, CornerRadius::same(6), tint.gamma_multiply(0.2 * alpha));
        p.text(tile.center(), Align2::CENTER_CENTER, &row.icon, FontId::proportional(if opts.compact { 11.0 } else { 12.5 }), tint.gamma_multiply(alpha));
        let dot = Pos2::new(tile.right() - 1.0, tile.bottom() - 1.0);
        p.circle_filled(dot, 4.5, theme::bg());
        p.circle_filled(dot, 3.0, row.status.gamma_multiply(alpha));

        let text_x = tile.right() + 10.0;
        let close_w = 26.0;
        let renaming = self.renaming.as_ref().is_some_and(|(u, _)| *u == row.uid);
        // Chips (agent state, CPU / RAM) on the title line, right-aligned; the title is
        // clipped before them. Hidden while the close button shows.
        let mut badges_w = 0.0;
        if !renaming
            && hover <= 0.05
            && let Some(tab) = tab
        {
            let line = if opts.compact { card } else { Rect::from_min_max(card.min, Pos2::new(card.right(), card.center().y + 1.0)) };
            let badge_rect = Rect::from_min_max(Pos2::new(text_x + 40.0, line.top() + if opts.compact { 0.0 } else { 6.0 }), Pos2::new(card.right() - 8.0, line.bottom()));
            // A child that doesn't allocate in the list (the row already did).
            let mut badges = ui.new_child(UiBuilder::new().max_rect(badge_rect).layout(egui::Layout::right_to_left(egui::Align::Center)));
            self.badges.row_badges(tab, &mut badges);
            let used = badges.min_rect();
            if used.width() > 0.5 {
                badges_w = card.right() - used.left() + 4.0;
            }
        }
        let right = card.right() - 6.0 - (close_w * hover).max(badges_w);
        if renaming {
            self.rename_field(ui, row.uid, Rect::from_min_max(Pos2::new(text_x - 4.0, card.top() + 5.0), Pos2::new(card.right() - 6.0, card.bottom() - 5.0)), actions);
        } else {
            let clip = Rect::from_min_max(Pos2::new(text_x, card.top()), Pos2::new(right.max(text_x + 10.0), card.bottom()));
            let tp = p.with_clip_rect(clip.intersect(ui.clip_rect()));
            let title_col = if row.visible { theme::text() } else { theme::text_dim().lerp_to_gamma(theme::text(), hover) };
            if opts.compact {
                tp.text(Pos2::new(text_x, card.center().y), Align2::LEFT_CENTER, &row.title, FontId::proportional(12.5), title_col.gamma_multiply(alpha));
            } else {
                tp.text(Pos2::new(text_x, card.center().y - 8.0), Align2::LEFT_CENTER, &row.title, FontId::proportional(13.0), title_col.gamma_multiply(alpha));
                // The subtitle runs under the chips too (they sit on the title line).
                let sub_clip = Rect::from_min_max(Pos2::new(text_x, card.top()), Pos2::new(card.right() - 6.0 - close_w * hover, card.bottom()));
                p.with_clip_rect(sub_clip.intersect(ui.clip_rect())).text(
                    Pos2::new(text_x, card.center().y + 9.0),
                    Align2::LEFT_CENTER,
                    &row.subtitle,
                    FontId::proportional(11.0),
                    theme::text_faint().gamma_multiply(alpha),
                );
            }
        }

        if !renaming && hover > 0.05 {
            let c = Pos2::new(card.right() - 16.0 + (1.0 - hover) * 8.0, card.center().y);
            if close_button(ui, resp.id.with("close"), c, hover).clicked() {
                actions.push(TabAction::Close(row.uid));
            }
        }

        if resp.clicked() {
            actions.push(TabAction::Focus(row.uid));
        }
        if resp.middle_clicked() {
            actions.push(TabAction::Close(row.uid));
        }
        if resp.double_clicked() {
            self.renaming = Some((row.uid, row.title.clone()));
        }
        if !renaming {
            let tip = if opts.compact { format!("{}\n{}", row.title, row.subtitle) } else { row.subtitle.clone() };
            if !any_drag {
                resp.clone().on_hover_text_at_pointer(tip);
            }
        }
        resp.context_menu(|ui| {
            if ui.button("✏  Rename…").clicked() {
                self.renaming = Some((row.uid, row.title.clone()));
                ui.close();
            }
            tab_menu_items(ui, row.uid, actions);
        });

        // Dropping another row here: above the middle = before this tab, below = after.
        if let Some(pos) = ui.ctx().pointer_hover_pos()
            && let Some(drag) = resp.dnd_hover_payload::<TabDrag>()
            && drag.0 != row.uid
        {
            let after = pos.y > rect.center().y;
            let y = if after { rect.bottom() } else { rect.top() };
            ui.painter().hline(rect.x_range().shrink(4.0), y, Stroke::new(2.0, theme::accent()));
        }
        if let Some(drag) = resp.dnd_release_payload::<TabDrag>()
            && drag.0 != row.uid
        {
            let after = ui.ctx().pointer_interact_pos().is_some_and(|p| p.y > rect.center().y);
            let index = row.index + usize::from(after);
            actions.push(TabAction::Move { uid: drag.0, target: TabTarget::Insert { leaf: row.leaf, index } });
        }
    }

    fn rename_field(&mut self, ui: &mut Ui, uid: u64, rect: Rect, actions: &mut Vec<TabAction>) {
        let Some((_, text)) = &mut self.renaming else { return };
        let edit = ui.put(rect, egui::TextEdit::singleline(text).margin(egui::Margin::symmetric(6, 4)).font(FontId::proportional(13.0)).hint_text("Tab name"));
        if !edit.has_focus() && !edit.lost_focus() {
            edit.request_focus();
        }
        let (enter, esc) = ui.input(|i| (i.key_pressed(egui::Key::Enter), i.key_pressed(egui::Key::Escape)));
        if esc {
            self.renaming = None;
        } else if enter || edit.lost_focus() {
            let name = text.trim().to_string();
            actions.push(TabAction::Rename(uid, Some(name).filter(|n| !n.is_empty())));
            self.renaming = None;
        }
    }
}

/// The context menu entries shared by tab rows and dock tab buttons (rename excluded).
pub fn tab_menu_items(ui: &mut Ui, uid: u64, actions: &mut Vec<TabAction>) {
    let item = |ui: &mut Ui, text: &str, action: Option<Action>| {
        let mut b = egui::Button::new(text);
        if let Some(keys) = action.and_then(keybindings::binding) {
            b = b.shortcut_text(keys.label());
        }
        ui.add(b).clicked()
    };
    if item(ui, "⧉  Duplicate", None) {
        actions.push(TabAction::Duplicate(uid));
        ui.close();
    }
    if item(ui, "◫  Split right", Some(Action::SplitRight)) {
        actions.push(TabAction::SplitWith(uid, Split::Right));
        ui.close();
    }
    if item(ui, "⊟  Split down", Some(Action::SplitDown)) {
        actions.push(TabAction::SplitWith(uid, Split::Below));
        ui.close();
    }
    if item(ui, "🗗  Move to new window", None) {
        actions.push(TabAction::PopOut(uid));
        ui.close();
    }
    ui.separator();
    if item(ui, "Close others", None) {
        actions.push(TabAction::CloseOthers(uid));
        ui.close();
    }
    if item(ui, "Close", Some(Action::CloseTab)) {
        actions.push(TabAction::Close(uid));
        ui.close();
    }
}

fn options_menu(ui: &mut Ui, opts: ListOptions, position: TabPosition, actions: &mut Vec<TabAction>) {
    ui.set_min_width(200.0);
    let mut grouped = opts.grouped;
    if ui.checkbox(&mut grouped, "Group by pane").changed() {
        actions.push(TabAction::SetGrouped(grouped));
    }
    let mut compact = opts.compact;
    if ui.checkbox(&mut compact, "Compact rows").changed() {
        actions.push(TabAction::SetCompact(compact));
    }
    ui.separator();
    ui.label(egui::RichText::new("Tab position").size(11.0).color(theme::text_faint()));
    for p in TabPosition::ALL {
        if ui.radio(position == p, p.label()).clicked() && position != p {
            actions.push(TabAction::SetPosition(p));
            ui.close();
        }
    }
}

fn shells_menu(ui: &mut Ui, actions: &mut Vec<TabAction>) {
    ui.set_min_width(200.0);
    let shells = crate::pty::shells::available();
    if shells.is_empty() {
        ui.label(egui::RichText::new("No local shells found").color(theme::text_dim()));
    }
    for shell in shells {
        if ui.button(format!("{}  New {}", shell.icon_kind.glyph(), shell.name)).clicked() {
            actions.push(TabAction::NewShell(Some(shell.id.clone())));
            ui.close();
        }
    }
}

fn row_h(opts: ListOptions) -> f32 {
    if opts.compact { ROW_H_COMPACT } else { ROW_H }
}

fn collect(dock: &mut DockState<Tab>) -> Vec<Group> {
    let focused = dock.focused_leaf();
    let mut groups = Vec::new();
    for (path, leaf) in dock.iter_leaves_mut() {
        if !path.surface.is_main() || leaf.tabs.is_empty() {
            continue;
        }
        let active = leaf.active.0;
        let is_focused = focused == Some(path);
        let rows = leaf
            .tabs
            .iter_mut()
            .enumerate()
            .map(|(index, tab)| RowModel {
                uid: tab.uid,
                title: tab.display_title(),
                subtitle: tab.subtitle(),
                icon: row_icon(tab),
                hue: tab.profile.hue,
                status: tab.status_color(),
                leaf: path,
                index,
                visible: index == active,
                focused: is_focused && index == active,
            })
            .collect();
        groups.push(Group { leaf: path, focused: is_focused, rows });
    }
    groups
}

/// Shell glyph for local shells, the host's initial for SSH terminals.
fn row_icon(tab: &Tab) -> String {
    match (&tab.body, &tab.local) {
        (TabBody::Terminal(_), None) => tab.profile.display_name().chars().next().unwrap_or('?').to_uppercase().to_string(),
        _ => tab.icon().to_string(),
    }
}

fn last_leaf(dock: &DockState<Tab>) -> Option<NodePath> {
    dock.iter_leaves().filter(|(p, l)| p.surface.is_main() && !l.tabs.is_empty()).map(|(p, _)| p).last()
}

fn group_header(ui: &mut Ui, index: usize, group: &Group) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), GROUP_H), Sense::click());
    let p = ui.painter();
    let y = rect.center().y + 2.0;
    let col = if group.focused { theme::text_dim() } else { theme::text_faint() };
    // A tiny pane glyph: the focused pane's is filled with the accent.
    let glyph = Rect::from_center_size(Pos2::new(rect.left() + 10.0, y), Vec2::new(10.0, 8.0));
    if group.focused {
        p.rect_filled(glyph, CornerRadius::same(2), theme::accent().gamma_multiply(0.8));
    } else {
        p.rect_stroke(glyph, CornerRadius::same(2), Stroke::new(1.0, col), egui::StrokeKind::Inside);
    }
    let label = format!("PANE {}", index + 1);
    p.text(Pos2::new(rect.left() + 22.0, y), Align2::LEFT_CENTER, label, FontId::proportional(10.5), col);
    let n = group.rows.len();
    p.text(Pos2::new(rect.right() - 6.0, y), Align2::RIGHT_CENTER, format!("{n} tab{}", if n == 1 { "" } else { "s" }), FontId::proportional(10.5), theme::text_faint().gamma_multiply(0.8));
    resp
}

/// Highlights `resp` while a tab row hovers it and emits a move on release.
fn drop_zone(ui: &Ui, resp: &egui::Response, target: TabTarget, actions: &mut Vec<TabAction>) {
    let hovering = resp.dnd_hover_payload::<TabDrag>().is_some();
    let glow = ui.ctx().animate_bool_with_time(resp.id.with("drop"), hovering, theme::anim_time(0.12));
    if glow > 0.0 {
        let r = resp.rect.shrink(1.0);
        ui.painter().rect_filled(r, CornerRadius::same(theme::RADIUS_SM), theme::accent().gamma_multiply(0.08 * glow));
        ui.painter().rect_stroke(r, CornerRadius::same(theme::RADIUS_SM), Stroke::new(1.0, theme::accent().gamma_multiply(glow)), egui::StrokeKind::Inside);
    }
    if let Some(d) = resp.dnd_release_payload::<TabDrag>() {
        actions.push(TabAction::Move { uid: d.0, target });
    }
}

/// A chip with the dragged tab's name following the pointer.
fn drag_ghost(ui: &Ui, groups: &[Group]) {
    let Some(drag) = egui::DragAndDrop::payload::<TabDrag>(ui.ctx()) else { return };
    let Some(pos) = ui.ctx().pointer_interact_pos() else { return };
    let Some(row) = groups.iter().flat_map(|g| &g.rows).find(|r| r.uid == drag.0) else { return };
    paint_ghost(ui.ctx(), pos, &row.icon, &row.title, row.hue);
}

pub fn paint_ghost(ctx: &egui::Context, pos: Pos2, icon: &str, title: &str, hue: f32) {
    let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, Id::new("vtabs_drag_ghost")));
    let galley = painter.layout_no_wrap(title.to_string(), FontId::proportional(12.5), theme::text());
    let rect = Rect::from_min_size(pos + Vec2::new(14.0, 8.0), galley.size() + Vec2::new(40.0, 14.0));
    painter.add(egui::Shadow { offset: [0, 4], blur: 12, spread: 0, color: theme::shadow() }.as_shape(rect, CornerRadius::same(8)));
    painter.rect(rect, CornerRadius::same(8), theme::surface_2(), Stroke::new(1.0, theme::accent().gamma_multiply(0.6)), egui::StrokeKind::Inside);
    let tile = Rect::from_center_size(Pos2::new(rect.left() + 15.0, rect.center().y), Vec2::splat(18.0));
    let tint = theme::hue_color(hue);
    painter.rect_filled(tile, CornerRadius::same(5), tint.gamma_multiply(0.22));
    painter.text(tile.center(), Align2::CENTER_CENTER, icon, FontId::proportional(11.0), tint);
    painter.galley(Pos2::new(rect.left() + 30.0, rect.center().y - galley.size().y / 2.0), galley, theme::text());
}

/// While a tab row is dragged over the panes: highlight where it would land (a half
/// of a pane near its edges = split, the middle = join that pane) and return the move
/// once it is released there. Call after the dock was shown (pane rects are current).
pub fn drop_on_panes(ctx: &egui::Context, dock: &DockState<Tab>, area: Rect) -> Option<TabAction> {
    let drag = *egui::DragAndDrop::payload::<TabDrag>(ctx)?;
    let pos = ctx.pointer_latest_pos().filter(|p| area.contains(*p))?;
    let (leaf, rect) = dock.iter_leaves().filter(|(p, l)| p.surface.is_main() && !l.tabs.is_empty()).map(|(p, l)| (p, l.rect)).find(|(_, r)| r.contains(pos))?;
    let (target, preview) = pane_zone(leaf, rect, pos);
    let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, Id::new("vtabs_pane_drop")));
    let r = preview.shrink(4.0);
    painter.rect_filled(r, CornerRadius::same(theme::RADIUS), theme::accent().gamma_multiply(0.14));
    painter.rect_stroke(r, CornerRadius::same(theme::RADIUS), Stroke::new(1.5, theme::accent().gamma_multiply(0.8)), egui::StrokeKind::Inside);
    let label = match target {
        TabTarget::Split { split: Split::Left | Split::Right, .. } => "Split side by side",
        TabTarget::Split { .. } => "Split top / bottom",
        _ => "Move to this pane",
    };
    let galley = painter.layout_no_wrap(label.to_string(), FontId::proportional(12.5), theme::on_accent());
    let pill = Rect::from_center_size(r.center(), galley.size() + Vec2::new(24.0, 12.0));
    painter.rect_filled(pill, CornerRadius::same((pill.height() / 2.0) as u8), theme::accent());
    painter.galley(pill.center() - galley.size() / 2.0, galley, theme::on_accent());
    ctx.input(|i| i.pointer.any_released()).then_some(TabAction::Move { uid: drag.0, target })
}

/// Which part of a pane `pos` is over: the target and the rect to highlight.
pub fn pane_zone(leaf: NodePath, rect: Rect, pos: Pos2) -> (TabTarget, Rect) {
    let rel = (pos - rect.min) / rect.size();
    // The closest edge wins when the pointer is in a corner.
    let edges = [(rel.x, Split::Left), (1.0 - rel.x, Split::Right), (rel.y, Split::Above), (1.0 - rel.y, Split::Below)];
    let (dist, split) = edges.into_iter().fold((f32::INFINITY, Split::Left), |best, e| if e.0 < best.0 { e } else { best });
    if dist > EDGE {
        return (TabTarget::Append(leaf), rect);
    }
    let half = match split {
        Split::Left => Rect::from_min_max(rect.min, Pos2::new(rect.center().x, rect.bottom())),
        Split::Right => Rect::from_min_max(Pos2::new(rect.center().x, rect.top()), rect.max),
        Split::Above => Rect::from_min_max(rect.min, Pos2::new(rect.right(), rect.center().y)),
        Split::Below => Rect::from_min_max(Pos2::new(rect.left(), rect.center().y), rect.max),
    };
    (TabTarget::Split { leaf, split }, half)
}

fn small_button(ui: &mut Ui, text: &str, tip: &str) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(24.0), Sense::click());
    let h = ui.ctx().animate_bool_with_time(resp.id, resp.hovered() || egui::Popup::is_id_open(ui.ctx(), resp.id.with("popup")), theme::anim_time(0.1));
    if h > 0.0 {
        ui.painter().rect_filled(rect, CornerRadius::same(6), theme::surface_3().gamma_multiply(h));
    }
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let col = theme::text_dim().lerp_to_gamma(theme::text(), h);
    if text.is_empty() {
        // "More": three painted dots (the font has no reliable ellipsis glyph).
        for dx in [-5.0, 0.0, 5.0] {
            ui.painter().circle_filled(rect.center() + Vec2::new(dx, 0.0), 1.5, col);
        }
    } else {
        ui.painter().text(rect.center(), Align2::CENTER_CENTER, text, FontId::proportional(15.0), col);
    }
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, tip));
    resp.on_hover_text(tip)
}

fn close_button(ui: &mut Ui, id: Id, c: Pos2, alpha: f32) -> egui::Response {
    let rect = Rect::from_center_size(c, Vec2::splat(20.0));
    let resp = ui.interact(rect, id, Sense::click()).on_hover_text(keybindings::hint("Close", Action::CloseTab));
    let bg = if resp.hovered() { theme::danger().gamma_multiply(0.22) } else { theme::surface_3().gamma_multiply(0.6) };
    ui.painter().rect_filled(rect, CornerRadius::same(5), bg.gamma_multiply(alpha));
    let col = if resp.hovered() { theme::danger() } else { theme::text_dim() };
    let s = Stroke::new(1.4, col.gamma_multiply(alpha));
    let d = 3.5;
    ui.painter().line_segment([c + Vec2::new(-d, -d), c + Vec2::new(d, d)], s);
    ui.painter().line_segment([c + Vec2::new(d, -d), c + Vec2::new(-d, d)], s);
    resp
}

fn empty_state(ui: &mut Ui, actions: &mut Vec<TabAction>) {
    ui.add_space(24.0);
    ui.vertical_centered(|ui| {
        ui.label(egui::RichText::new("No open sessions").color(theme::text_dim()));
        ui.add_space(6.0);
        let hint = keybindings::hint("Open the default shell", Action::NewLocalTerminal);
        if ui.add(egui::Button::new(egui::RichText::new("+  New terminal").size(12.5)).fill(theme::accent_soft())).on_hover_text(hint).clicked() {
            actions.push(TabAction::NewShell(None));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_dock::{NodeIndex, SurfaceIndex};

    fn leaf() -> NodePath {
        NodePath { surface: SurfaceIndex::main(), node: NodeIndex::root() }
    }

    #[test]
    fn pane_zones_split_near_edges_and_join_in_the_middle() {
        let r = Rect::from_min_size(Pos2::ZERO, Vec2::new(400.0, 200.0));
        assert!(matches!(pane_zone(leaf(), r, Pos2::new(200.0, 100.0)).0, TabTarget::Append(_)));
        let (t, half) = pane_zone(leaf(), r, Pos2::new(20.0, 100.0));
        assert_eq!(t, TabTarget::Split { leaf: leaf(), split: Split::Left });
        assert_eq!(half.width(), 200.0);
        assert_eq!(pane_zone(leaf(), r, Pos2::new(390.0, 100.0)).0, TabTarget::Split { leaf: leaf(), split: Split::Right });
        assert_eq!(pane_zone(leaf(), r, Pos2::new(200.0, 5.0)).0, TabTarget::Split { leaf: leaf(), split: Split::Above });
        let (t, half) = pane_zone(leaf(), r, Pos2::new(200.0, 195.0));
        assert_eq!(t, TabTarget::Split { leaf: leaf(), split: Split::Below });
        assert_eq!(half.height(), 100.0);
    }
}
