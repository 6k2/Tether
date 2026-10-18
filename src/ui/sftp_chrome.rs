//! Small presentational pieces of the SFTP view: titles, toolbar buttons, pane toggles,
//! the Explorer drop overlay and the connection-failed card.

use eframe::egui::{self, Align2, Color32, CornerRadius, FontId, Rect, Sense, Stroke, Ui, Vec2};

use super::file_list::ListAction;
use crate::theme;

pub fn pane_title(ui: &mut Ui, text: &str, color: Color32) {
    ui.label(egui::RichText::new(text).size(10.5).strong().color(color));
}

pub fn toolbar(ui: &mut Ui, actions: &mut Vec<ListAction>, transfer_tip: &str, transfer_icon: &str, can_transfer: bool) {
    if icon_button(ui, "⟳", "Refresh") {
        actions.push(ListAction::Refresh);
    }
    if icon_button(ui, "📁", "New folder") {
        actions.push(ListAction::NewFolder);
    }
    ui.add_enabled_ui(can_transfer, |ui| {
        if icon_button(ui, transfer_icon, transfer_tip) {
            actions.push(ListAction::Transfer(Vec::new()));
        }
    });
}

/// Width taken by [`nav_buttons`].
pub const NAV_BUTTONS_W: f32 = 2.0 * NAV_BTN + 4.0;
const NAV_BTN: f32 = 22.0;

/// Browser-style back / forward buttons with painted chevrons. Returns (back, forward) clicks.
pub fn nav_buttons(ui: &mut Ui, can_back: bool, can_forward: bool) -> (bool, bool) {
    let back = chevron_button(ui, true, can_back, "Back (Alt+Left, mouse back)");
    let forward = chevron_button(ui, false, can_forward, "Forward (Alt+Right, mouse forward)");
    (back, forward)
}

fn chevron_button(ui: &mut Ui, left: bool, enabled: bool, tip: &str) -> bool {
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(NAV_BTN), sense);
    let ctx = ui.ctx();
    let hover = ctx.animate_bool_with_time(resp.id.with("h"), enabled && resp.hovered(), 0.12);
    let on = ctx.animate_bool_with_time(resp.id.with("on"), enabled, 0.15);
    let p = ui.painter();
    if hover > 0.0 {
        p.rect_filled(rect, CornerRadius::same(theme::RADIUS_SM), theme::surface_2().gamma_multiply(hover));
    }
    let color = theme::text_faint().gamma_multiply(0.6).lerp_to_gamma(theme::text_dim().lerp_to_gamma(theme::text(), hover), on);
    let c = rect.center();
    let (dx, dy) = (if left { 2.5 } else { -2.5 }, 4.5);
    let stroke = Stroke::new(1.6, color);
    p.line_segment([c + Vec2::new(dx, -dy), c + Vec2::new(-dx, 0.0)], stroke);
    p.line_segment([c + Vec2::new(-dx, 0.0), c + Vec2::new(dx, dy)], stroke);
    let clicked = resp.clicked();
    if enabled {
        resp.on_hover_text(tip);
    }
    clicked
}

/// Thin indeterminate progress bar (always takes its height so the list never jumps).
pub fn loading_bar(ui: &mut Ui, id: egui::Id, loading: bool) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 2.0), Sense::hover());
    let t = ui.ctx().animate_bool_with_time(id, loading, 0.2);
    if t <= 0.0 {
        return;
    }
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::same(1), theme::accent().gamma_multiply(0.12 * t));
    let phase = (ui.input(|i| i.time) * 0.9).fract() as f32;
    let seg = rect.width() * 0.3;
    let x0 = rect.left() - seg + (rect.width() + seg) * phase;
    let bar = Rect::from_min_max(egui::pos2(x0.max(rect.left()), rect.top()), egui::pos2((x0 + seg).min(rect.right()), rect.bottom()));
    if bar.width() > 0.0 {
        p.rect_filled(bar, CornerRadius::same(1), theme::accent().gamma_multiply(t));
    }
    ui.ctx().request_repaint();
}

pub fn icon_button(ui: &mut Ui, icon: &str, tip: &str) -> bool {
    ui.add(egui::Button::new(icon).frame(false).min_size(Vec2::splat(24.0))).on_hover_text(tip).clicked()
}

/// Rounded toggle chip: filled with `color` when on. `locked` = on and cannot be turned off.
pub fn pill(ui: &mut Ui, label: &str, on: bool, locked: bool, color: Color32) -> egui::Response {
    let font = FontId::proportional(11.5);
    let galley = ui.painter().layout_no_wrap(label.to_string(), font, theme::text());
    let size = Vec2::new(galley.size().x + 26.0, 20.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let ctx = ui.ctx();
    let t = ctx.animate_bool_with_time(resp.id, on, 0.16);
    let hover = ctx.animate_bool_with_time(resp.id.with("h"), resp.hovered() && !locked, 0.12);
    let radius = CornerRadius::same(10);
    let p = ui.painter();
    p.rect_filled(rect, radius, theme::surface_2().lerp_to_gamma(color.gamma_multiply(0.22), t));
    p.rect_stroke(rect, radius, Stroke::new(1.0, theme::border().lerp_to_gamma(color.gamma_multiply(0.6), t.max(hover * 0.6))), egui::StrokeKind::Inside);
    let dot = egui::pos2(rect.left() + 10.0, rect.center().y);
    p.circle_filled(dot, 2.5 + t, theme::text_faint().lerp_to_gamma(color, t));
    let text_color = theme::text_dim().lerp_to_gamma(theme::text(), t.max(hover));
    p.galley(egui::pos2(rect.left() + 17.0, rect.center().y - galley.size().y / 2.0), galley, text_color);
    let tip = match (on, locked) {
        (_, true) => "At least one pane stays visible",
        (true, false) => "Hide pane",
        (false, false) => "Show pane",
    };
    resp.on_hover_text(tip)
}

/// Dimmed card over a pane while files from Explorer hover it. `t` is the fade (0..1).
pub fn drop_overlay(ui: &Ui, rect: Rect, cwd: &str, t: f32) {
    let p = ui.painter();
    let radius = CornerRadius::same(theme::RADIUS);
    p.rect_filled(rect, radius, theme::bg().gamma_multiply(0.82 * t));
    let inner = rect.shrink(10.0 + 4.0 * (1.0 - t));
    p.rect_stroke(inner, radius, Stroke::new(1.5, theme::accent().gamma_multiply(t)), egui::StrokeKind::Inside);
    let c = inner.center();
    p.text(c - Vec2::new(0.0, 22.0), Align2::CENTER_CENTER, "⬆", FontId::proportional(26.0), theme::accent().gamma_multiply(t));
    p.text(c + Vec2::new(0.0, 8.0), Align2::CENTER_CENTER, "Drop to upload to", FontId::proportional(13.0), theme::text().gamma_multiply(t));
    let shown = if cwd.is_empty() { "the remote folder" } else { cwd };
    let mut job = egui::text::LayoutJob::simple_singleline(shown.to_string(), FontId::monospace(12.5), theme::accent().gamma_multiply(t));
    job.wrap = egui::text::TextWrapping { max_width: inner.width() - 24.0, max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
    let galley = ui.painter().layout_job(job);
    p.galley(c + Vec2::new(-galley.size().x / 2.0, 20.0), galley, theme::accent());
}

pub fn failed_card(ui: &mut Ui, rect: Rect, err: &str) -> bool {
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
    let mut clicked = false;
    egui::Frame::new().fill(theme::surface()).corner_radius(theme::RADIUS).inner_margin(16.0).show(&mut child, |ui| {
        ui.set_min_size(rect.size() - Vec2::splat(32.0));
        ui.vertical_centered(|ui| {
            ui.add_space(rect.height() * 0.3);
            ui.label(egui::RichText::new("SFTP connection failed").strong().color(theme::danger()));
            ui.label(egui::RichText::new(err).color(theme::text_dim()).size(12.0));
            ui.add_space(8.0);
            clicked = ui.button("⟳  Reconnect").clicked();
        });
    });
    clicked
}
