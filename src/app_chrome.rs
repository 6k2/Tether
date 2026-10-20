//! The top bar: sidebar toggle, import / keys, the command palette pill, a quick layout
//! menu, broadcast and settings. A child module of `app`.

use eframe::egui::{self, Align2, CornerRadius, FontId, Pos2, Rect, RichText, Sense, Stroke, Vec2};

use super::TetherApp;
use crate::keybindings::{self, Action};
use crate::layout::{Side, TabPosition};
use crate::settings;
use crate::theme;
use crate::ui::import_view::ImportDialog;
use crate::ui::key_manager::KeyManager;
use crate::ui::settings_view::SettingsView;
use crate::ui::settings_widgets::{self as w, Icon, icon_button, paint_icon};

const BAR_H: f32 = 40.0;
const PILL_W: f32 = 340.0;

impl TetherApp {
    pub(super) fn top_bar(&mut self, ui: &mut egui::Ui) {
        let frame = egui::Frame::new().fill(theme::bg()).inner_margin(egui::Margin::symmetric(8, 6));
        egui::Panel::top("top").exact_size(BAR_H).frame(frame).show(ui, |ui| {
            let bar = ui.max_rect();
            self.palette_pill(ui, bar);
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                // The sidebar toggle sits on the sidebar's side of the bar.
                let sidebar_right = settings::snapshot().layout.sidebar_side == Side::Right;
                if !sidebar_right {
                    sidebar_toggle(ui);
                    ui.add_space(4.0);
                } else {
                    ui.add_space(4.0);
                }
                if bar_button(ui, "⬇ Import").on_hover_text("Import hosts from Bitvise or an SSH config").clicked() {
                    self.import = Some(ImportDialog::new(&self.store));
                }
                if bar_button(ui, "🔑 Keys").on_hover_text(keybindings::hint("Key manager", Action::KeyManager)).clicked() {
                    self.key_manager = Some(KeyManager::new(&self.keys));
                }
                let tabs = self.all_tabs().count();
                ui.add_space(4.0);
                ui.label(RichText::new(format!("{tabs} session{}", if tabs == 1 { "" } else { "s" })).size(11.5).color(theme::text_faint()));
                ui.add_space(4.0);
                self.agents_top_bar(ui);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    if sidebar_right {
                        sidebar_toggle(ui);
                        ui.add_space(4.0);
                    }
                    if icon_button(ui, Icon::Gear, 28.0).on_hover_text(keybindings::hint("Settings", Action::OpenSettings)).clicked() {
                        self.settings_view.get_or_insert_with(SettingsView::new);
                    }
                    let layout = icon_button(ui, Icon::Layout, 28.0).on_hover_text("Layout: tabs, sidebar, panes");
                    egui::Popup::menu(&layout).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| self.layout_menu(ui));
                    ui.add_space(4.0);
                    self.broadcast_button(ui);
                    crate::ui::update_banner::top_bar(ui);
                });
            });
        });
    }

    /// "Search or run a command…" centred in the bar; opens the palette.
    fn palette_pill(&mut self, ui: &mut egui::Ui, bar: Rect) {
        let width = PILL_W.min(bar.width() * 0.34);
        if width < 160.0 {
            return;
        }
        let rect = Rect::from_center_size(bar.center(), Vec2::new(width, 28.0));
        let resp = ui.interact(rect, egui::Id::new("palette_pill"), Sense::click());
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Command palette"));
        let h = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), theme::anim_time(0.12));
        if resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        let p = ui.painter();
        p.rect(rect, CornerRadius::same(8), theme::surface().lerp_to_gamma(theme::surface_2(), h), Stroke::new(1.0, theme::border().gamma_multiply(0.6 + 0.4 * h)), egui::StrokeKind::Inside);
        paint_icon(p, Rect::from_center_size(Pos2::new(rect.left() + 16.0, rect.center().y), Vec2::splat(13.0)), Icon::Search, theme::text_faint());
        p.text(Pos2::new(rect.left() + 30.0, rect.center().y), Align2::LEFT_CENTER, "Search or run a command…", FontId::proportional(12.5), theme::text_faint().lerp_to_gamma(theme::text_dim(), h));
        let keys = keybindings::label(Action::CommandPalette);
        if !keys.is_empty() {
            p.text(Pos2::new(rect.right() - 10.0, rect.center().y), Align2::RIGHT_CENTER, keys, FontId::monospace(10.5), theme::text_faint());
        }
        if resp.clicked() {
            self.open_palette();
        }
    }

    fn broadcast_button(&mut self, ui: &mut egui::Ui) {
        let glow = ui.ctx().animate_bool_with_time(egui::Id::new("bc_glow"), self.broadcast, theme::anim_time(0.2));
        let fill = theme::danger().gamma_multiply(0.25 * glow);
        let text = RichText::new("📡 Broadcast").size(12.0).color(if self.broadcast { theme::danger() } else { theme::text_dim() });
        if ui
            .add(egui::Button::new(text).fill(fill).corner_radius(CornerRadius::same(7)).min_size(Vec2::new(0.0, 28.0)))
            .on_hover_text(keybindings::hint("Type into every terminal at once", Action::ToggleBroadcast))
            .clicked()
        {
            self.broadcast = !self.broadcast;
        }
    }

    /// The quick layout popover: the most used arrangement switches in one place.
    fn layout_menu(&mut self, ui: &mut egui::Ui) {
        ui.set_width(280.0);
        let before = settings::snapshot().layout.clone();
        let mut l = before.clone();
        ui.spacing_mut().item_spacing.y = 6.0;
        ui.label(RichText::new("TABS").size(10.5).color(theme::text_faint()));
        w::segmented(ui, &mut l.tab_position, &[(TabPosition::Top, "Top"), (TabPosition::Left, "Left"), (TabPosition::Right, "Right")]);
        ui.add_space(4.0);
        ui.label(RichText::new("SIDEBAR").size(10.5).color(theme::text_faint()));
        ui.horizontal(|ui| {
            w::segmented(ui, &mut l.sidebar_side, &[(Side::Left, "Left"), (Side::Right, "Right")]);
            ui.add_space(8.0);
            ui.checkbox(&mut l.sidebar_visible, "Visible");
        });
        ui.checkbox(&mut l.activity_bar, "Activity bar");
        ui.add_space(4.0);
        ui.label(RichText::new("PANES").size(10.5).color(theme::text_faint()));
        ui.checkbox(&mut l.dim_inactive_panes, "Dim inactive panes");
        ui.checkbox(&mut l.focus_follows_mouse, "Focus follows mouse");
        let zoom = if self.zoomed.is_some() { "Restore panes" } else { "Maximize focused pane" };
        if ui.add(egui::Button::new(zoom).shortcut_text(keybindings::label(Action::ZoomPane))).clicked() {
            self.toggle_zoom();
            ui.close();
        }
        ui.separator();
        if ui.button("All layout settings…").clicked() {
            self.open_layout_settings();
            ui.close();
        }
        if l != before {
            let _ = settings::update(|s| s.layout = l);
        }
    }
}

fn sidebar_toggle(ui: &mut egui::Ui) {
    let l = settings::snapshot().layout.clone();
    let icon = if l.sidebar_side == Side::Right { Icon::SidebarRight } else { Icon::SidebarLeft };
    let tip = keybindings::hint(if l.sidebar_visible { "Hide sidebar" } else { "Show sidebar" }, Action::ToggleSidebar);
    if icon_button(ui, icon, 28.0).on_hover_text(tip).clicked() {
        super::layout_ui::toggle_sidebar();
    }
}

fn bar_button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.add(egui::Button::new(RichText::new(text).size(12.0)).frame(false).min_size(Vec2::new(0.0, 28.0)))
}
