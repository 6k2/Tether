//! The "Workspaces" section at the bottom of the sidebar: saved layouts as clickable
//! cards, an inline "save as" field, rename in place and a confirm-before-delete row.

use eframe::egui::{self, Align2, CornerRadius, FontId, Key, Pos2, Rect, RichText, Sense, Stroke, Ui, Vec2};

use super::SidebarAction;
use super::menu;
use super::paint::{Glyph, glyph_button, mini_button, paint_glyph};
use crate::theme;
use crate::workspace::LAST_SESSION;

/// What the sidebar shows for one saved layout.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkspaceInfo {
    pub name: String,
    pub tabs: usize,
    /// Display names of the hosts it opens, de-duplicated, in layout order.
    pub hosts: Vec<String>,
}

impl WorkspaceInfo {
    fn automatic(&self) -> bool {
        self.name == LAST_SESSION
    }

    /// "3 tabs · api-prod-01, db-replica +2".
    pub fn summary(&self) -> String {
        let tabs = format!("{} tab{}", self.tabs, if self.tabs == 1 { "" } else { "s" });
        if self.automatic() {
            return format!("Saved automatically · {tabs}");
        }
        const SHOWN: usize = 2;
        let mut hosts = self.hosts.iter().take(SHOWN).cloned().collect::<Vec<_>>().join(", ");
        if self.hosts.len() > SHOWN {
            hosts.push_str(&format!(" +{}", self.hosts.len() - SHOWN));
        }
        if hosts.is_empty() { tabs } else { format!("{tabs} · {hosts}") }
    }
}

const ROW_H: f32 = 42.0;
const FIELD_H: f32 = 34.0;
const EMPTY_H: f32 = 74.0;
const VISIBLE_ROWS: usize = 4;

#[derive(Default)]
pub struct WorkspacePanel {
    /// Name typed for "Save current layout as…".
    naming: Option<String>,
    renaming: Option<(String, String)>,
    confirm_delete: Option<String>,
}

/// The "?" glyph with an explanation of workspaces on hover.
fn help_glyph(ui: &mut Ui) {
    let (rect, help) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::hover());
    let col = if help.hovered() { theme::text() } else { theme::text_faint() };
    paint_glyph(ui.painter(), rect.center(), Glyph::Help, col);
    help.on_hover_ui(|ui| {
        ui.set_max_width(250.0);
        ui.label(RichText::new("Workspaces").strong());
        ui.label("A workspace saves your open tabs and how they are split, so you can reopen the whole arrangement in one click.");
        ui.add_space(4.0);
        ui.label(RichText::new("• Click a workspace to restore it (it replaces the open tabs).").color(theme::text_dim()));
        ui.label(RichText::new("• + saves the current layout under a name.").color(theme::text_dim()));
        ui.label(RichText::new(format!("• “{LAST_SESSION}” is kept automatically.")).color(theme::text_dim()));
    });
}

impl WorkspacePanel {
    pub fn start_save(&mut self) {
        self.naming = Some(String::new());
        self.confirm_delete = None;
    }

    /// Body height inside a sidebar section (the section draws the header).
    pub fn body_height(&self, list: &[WorkspaceInfo]) -> f32 {
        let rows = if list.is_empty() { EMPTY_H } else { list.len().min(VISIBLE_ROWS) as f32 * ROW_H };
        rows + if self.naming.is_some() { FIELD_H } else { 0.0 } + 6.0
    }

    /// The section header's buttons ("+" save, help), for a right-to-left `ui`.
    pub fn header_buttons(&mut self, ui: &mut Ui, can_save: bool) {
        let tip = if can_save { "Save current layout as workspace…" } else { "Open a terminal or SFTP tab first" };
        let add = ui.add_enabled_ui(can_save, |ui| glyph_button(ui, Glyph::Plus, tip)).inner;
        if add.clicked() {
            self.start_save();
        }
        help_glyph(ui);
    }

    /// The list (or empty state) and the inline "save as" field, without a header.
    pub fn body(&mut self, ui: &mut Ui, list: &[WorkspaceInfo], can_save: bool, actions: &mut Vec<SidebarAction>) {
        ui.add_space(2.0);
        self.contents(ui, list, can_save, actions);
    }

    fn contents(&mut self, ui: &mut Ui, list: &[WorkspaceInfo], can_save: bool, actions: &mut Vec<SidebarAction>) {
        if self.naming.is_some() {
            self.name_field(ui, list, actions);
        }
        if list.is_empty() {
            self.empty_state(ui, can_save);
            return;
        }
        egui::ScrollArea::vertical().id_salt("sidebar_ws").max_height(VISIBLE_ROWS as f32 * ROW_H).auto_shrink([false, true]).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for ws in list {
                if self.confirm_delete.as_deref() == Some(ws.name.as_str()) {
                    self.delete_row(ui, &ws.name, actions);
                } else if self.renaming.as_ref().is_some_and(|(n, _)| *n == ws.name) {
                    self.rename_row(ui, actions);
                } else {
                    self.row(ui, ws, can_save, actions);
                }
            }
        });
    }

    fn name_field(&mut self, ui: &mut Ui, list: &[WorkspaceInfo], actions: &mut Vec<SidebarAction>) {
        let Some(name) = &mut self.naming else { return };
        let mut done = None;
        ui.allocate_ui_with_layout(Vec2::new(ui.available_width(), FIELD_H), egui::Layout::left_to_right(egui::Align::Center), |ui| {
            let save_w = 52.0;
            let edit = ui.add(
                egui::TextEdit::singleline(name)
                    .hint_text("Name this layout…")
                    .desired_width(ui.available_width() - save_w - 6.0)
                    .margin(egui::Margin::symmetric(8, 5)),
            );
            if !edit.has_focus() && !edit.lost_focus() {
                edit.request_focus();
            }
            let valid = !name.trim().is_empty() && name.trim() != LAST_SESSION;
            let save = ui
                .add_enabled(valid, egui::Button::new(RichText::new("Save").color(theme::on_accent())).fill(theme::accent()).min_size(Vec2::new(save_w, 26.0)));
            let (enter, esc) = ui.input(|i| (i.key_pressed(Key::Enter), i.key_pressed(Key::Escape)));
            if esc {
                done = Some(None);
            } else if (save.clicked() || (enter && edit.lost_focus())) && valid {
                done = Some(Some(name.trim().to_string()));
            } else if edit.lost_focus() && !save.hovered() && name.trim().is_empty() {
                done = Some(None);
            }
            if list.iter().any(|w| w.name == name.trim()) {
                edit.on_hover_text("Replaces the workspace with this name");
            }
        });
        if let Some(result) = done {
            if let Some(n) = result {
                actions.push(SidebarAction::SaveWorkspace(n));
            }
            self.naming = None;
        }
    }

    fn empty_state(&mut self, ui: &mut Ui, can_save: bool) {
        let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), EMPTY_H - 8.0), Sense::hover());
        let p = ui.painter();
        p.rect(rect, CornerRadius::same(theme::RADIUS), theme::surface().gamma_multiply(0.5), Stroke::new(1.0, theme::border()), egui::StrokeKind::Inside);
        paint_glyph(p, Pos2::new(rect.left() + 18.0, rect.top() + 20.0), Glyph::Layout, theme::text_faint());
        let text = "Save your open tabs and splits,\nthen reopen them in one click.";
        p.text(Pos2::new(rect.left() + 34.0, rect.top() + 20.0), Align2::LEFT_CENTER, text, FontId::proportional(11.5), theme::text_dim());
        let link_rect = Rect::from_min_size(Pos2::new(rect.left() + 34.0, rect.bottom() - 24.0), Vec2::new(rect.width() - 44.0, 18.0));
        let label = if can_save { "+ Save current layout" } else { "Open a tab first" };
        let link = ui.put(
            link_rect,
            egui::Label::new(RichText::new(label).size(11.5).color(if can_save { theme::accent() } else { theme::text_faint() })).sense(Sense::click()),
        );
        if can_save && link.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if can_save && link.clicked() {
            self.start_save();
        }
    }

    fn row(&mut self, ui: &mut Ui, ws: &WorkspaceInfo, can_save: bool, actions: &mut Vec<SidebarAction>) {
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_H), Sense::click());
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, format!("Workspace {}", ws.name)));
        let ctx = ui.ctx().clone();
        let more_id = resp.id.with("more");
        let menu_open = egui::Popup::is_id_open(&ctx, more_id.with("popup"));
        let hot = ui.rect_contains_pointer(rect) || resp.context_menu_opened() || menu_open;
        let hover = ctx.animate_bool_with_time(resp.id.with("h"), hot, theme::anim_time(0.12));
        if resp.hovered() {
            ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        let p = ui.painter();
        let card = rect.shrink2(Vec2::new(0.0, 2.0));
        p.rect_filled(card, CornerRadius::same(theme::RADIUS), theme::surface_2().gamma_multiply(hover));
        let tile = Rect::from_center_size(Pos2::new(card.left() + 18.0, card.center().y), Vec2::splat(26.0));
        let (glyph, tint) = if ws.automatic() { (Glyph::Clock, theme::blue()) } else { (Glyph::Layout, theme::accent()) };
        p.rect_filled(tile, CornerRadius::same(7), tint.gamma_multiply(0.16 + 0.08 * hover));
        paint_glyph(p, tile.center(), glyph, tint);

        let text_x = tile.right() + 10.0;
        let right = card.right() - 6.0 - 76.0 * hover;
        let clip = Rect::from_min_max(Pos2::new(text_x, card.top()), Pos2::new(right.max(text_x + 20.0), card.bottom()));
        let tp = p.with_clip_rect(clip);
        tp.text(Pos2::new(text_x, card.center().y - 7.5), Align2::LEFT_CENTER, &ws.name, FontId::proportional(13.0), theme::text());
        tp.text(Pos2::new(text_x, card.center().y + 8.5), Align2::LEFT_CENTER, ws.summary(), FontId::proportional(11.0), theme::text_faint());

        let mut more_clicked = false;
        if hover > 0.05 {
            let open = Rect::from_min_size(Pos2::new(card.right() - 78.0, card.center().y - 11.0), Vec2::new(46.0, 22.0));
            p.rect_filled(open, CornerRadius::same(6), theme::accent_soft().gamma_multiply(hover));
            p.text(open.center(), Align2::CENTER_CENTER, "Open", FontId::proportional(11.5), theme::accent().gamma_multiply(hover));
            let more = mini_button(ui, more_id, Pos2::new(card.right() - 16.0, card.center().y), Glyph::More, hover, "Workspace actions");
            more_clicked = more.clicked() || more.hovered();
            egui::Popup::menu(&more).show(|ui| self.menu(ui, ws, can_save, actions));
        }
        if resp.clicked() && !more_clicked {
            actions.push(SidebarAction::RestoreWorkspace(ws.name.clone()));
        }
        resp.context_menu(|ui| self.menu(ui, ws, can_save, actions));
    }

    fn menu(&mut self, ui: &mut Ui, ws: &WorkspaceInfo, can_save: bool, actions: &mut Vec<SidebarAction>) {
        menu::begin(ui);
        menu::title(ui, "WORKSPACE", &ws.name);
        if menu::item(ui, "↗", "Open").on_hover_text("Replaces the open tabs with this layout").clicked() {
            actions.push(SidebarAction::RestoreWorkspace(ws.name.clone()));
        }
        if ws.automatic() {
            if ui.add_enabled_ui(can_save, |ui| menu::item(ui, "💾", "Save as new workspace…")).inner.clicked() {
                self.start_save();
            }
        } else {
            if menu::item(ui, "✏", "Rename").clicked() {
                self.renaming = Some((ws.name.clone(), ws.name.clone()));
            }
            let overwrite = ui.add_enabled_ui(can_save, |ui| menu::item(ui, "⟳", "Overwrite with current layout")).inner;
            if overwrite.clicked() {
                actions.push(SidebarAction::OverwriteWorkspace(ws.name.clone()));
            }
        }
        ui.separator();
        if menu::danger(ui, "🗑", "Delete…").clicked() {
            self.confirm_delete = Some(ws.name.clone());
        }
    }

    fn rename_row(&mut self, ui: &mut Ui, actions: &mut Vec<SidebarAction>) {
        let Some((from, text)) = &mut self.renaming else { return };
        let mut done = None;
        ui.allocate_ui_with_layout(Vec2::new(ui.available_width(), ROW_H), egui::Layout::left_to_right(egui::Align::Center), |ui| {
            let edit = ui.add(egui::TextEdit::singleline(text).desired_width(f32::INFINITY).margin(egui::Margin::symmetric(8, 5)));
            if !edit.has_focus() && !edit.lost_focus() {
                edit.request_focus();
            }
            let (enter, esc) = ui.input(|i| (i.key_pressed(Key::Enter), i.key_pressed(Key::Escape)));
            if esc {
                done = Some(None);
            } else if enter || edit.lost_focus() {
                done = Some(Some((from.clone(), text.trim().to_string())));
            }
        });
        if let Some(result) = done {
            if let Some((from, to)) = result.filter(|(f, t)| !t.is_empty() && f != t) {
                actions.push(SidebarAction::RenameWorkspace(from, to));
            }
            self.renaming = None;
        }
    }

    fn delete_row(&mut self, ui: &mut Ui, name: &str, actions: &mut Vec<SidebarAction>) {
        let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_H), Sense::hover());
        let card = rect.shrink2(Vec2::new(0.0, 2.0));
        ui.painter().rect(
            card,
            CornerRadius::same(theme::RADIUS),
            theme::danger().gamma_multiply(0.10),
            Stroke::new(1.0, theme::danger().gamma_multiply(0.5)),
            egui::StrokeKind::Inside,
        );
        // Buttons first (right to left), so the question truncates instead of them.
        let mut child =
            ui.new_child(egui::UiBuilder::new().max_rect(card.shrink2(Vec2::new(8.0, 0.0))).layout(egui::Layout::right_to_left(egui::Align::Center)));
        child.spacing_mut().item_spacing.x = 4.0;
        let cancel = child.add(egui::Button::new(RichText::new("Cancel").size(12.0)).frame_when_inactive(false));
        if cancel.clicked() || child.input(|i| i.key_pressed(Key::Escape)) {
            self.confirm_delete = None;
        }
        let delete = child.add(egui::Button::new(RichText::new("Delete").size(12.0).color(theme::on_accent())).fill(theme::danger()));
        if delete.clicked() {
            actions.push(SidebarAction::DeleteWorkspace(name.to_string()));
            self.confirm_delete = None;
        }
        child.add_space(4.0);
        child.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
            ui.add_space(5.0);
            ui.spacing_mut().item_spacing.y = 0.0;
            ui.label(RichText::new("Delete workspace?").size(12.0).color(theme::danger()));
            ui.add(egui::Label::new(RichText::new(name).size(11.0).color(theme::text_dim())).truncate());
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(name: &str, tabs: usize, hosts: &[&str]) -> WorkspaceInfo {
        WorkspaceInfo { name: name.into(), tabs, hosts: hosts.iter().map(|h| h.to_string()).collect() }
    }

    #[test]
    fn summary_names_a_few_hosts() {
        assert_eq!(info("dev", 1, &["web"]).summary(), "1 tab · web");
        assert_eq!(info("dev", 5, &["a", "b", "c", "d"]).summary(), "5 tabs · a, b +2");
        assert_eq!(info("dev", 2, &[]).summary(), "2 tabs");
        assert_eq!(info(LAST_SESSION, 3, &["a"]).summary(), "Saved automatically · 3 tabs");
    }

    #[test]
    fn height_grows_with_rows_and_the_name_field() {
        let mut panel = WorkspacePanel::default();
        let one = [info("a", 1, &[])];
        let many: Vec<WorkspaceInfo> = (0..9).map(|i| info(&i.to_string(), 1, &[])).collect();
        assert!(panel.body_height(&many) > panel.body_height(&one));
        assert_eq!(panel.body_height(&many), panel.body_height(&many[..VISIBLE_ROWS]), "scrolls past the visible rows");
        let before = panel.body_height(&one);
        panel.start_save();
        assert!(panel.body_height(&one) > before);
    }
}
