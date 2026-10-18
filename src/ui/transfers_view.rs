//! Transfer queue: animated progress, throughput, ETA, cancel.

use std::sync::atomic::Ordering;

use eframe::egui::{self, Align2, Color32, CornerRadius, FontId, Painter, Pos2, Rect, Sense, Stroke, Ui, Vec2, pos2, vec2};

use crate::ssh::transfer::{Direction, Transfer, TransferState};
use crate::theme;

const ROW_H: f32 = 38.0;
/// Horizontal inset of the hover background, matching the file list.
const ROW_INSET: f32 = 4.0;
const TEXT_X: f32 = 34.0;

pub fn show(ui: &mut Ui, list: &mut Vec<Transfer>) {
    egui::Frame::new().fill(theme::surface()).corner_radius(theme::RADIUS).inner_margin(8.0).show(ui, |ui| {
        ui.set_min_size(ui.available_size());
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("TRANSFERS").size(10.5).strong().color(theme::text_dim()));
            let running = list.iter().filter(|t| t.state() == TransferState::Running).count();
            if running > 0 {
                let total: f64 = list.iter().filter(|t| t.state() == TransferState::Running).map(Transfer::speed).sum();
                ui.label(egui::RichText::new(format!("{running} active · {}/s", theme::human_bytes(total as u64))).size(11.0).color(theme::accent()));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add(egui::Button::new("Clear finished").frame(false)).clicked() {
                    list.retain(|t| t.state() == TransferState::Running);
                }
            });
        });
        let divider = ui.available_rect_before_wrap();
        ui.painter().hline((divider.left() + ROW_INSET)..=(divider.right() - ROW_INSET), divider.top() + 2.0, Stroke::new(1.0, theme::border()));
        ui.add_space(4.0);
        egui::ScrollArea::vertical().auto_shrink([false, false]).stick_to_bottom(true).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for t in list.iter() {
                row(ui, t);
            }
        });
    });
    if list.iter().any(|t| t.state() == TransferState::Running) {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
    }
}

fn row(ui: &mut Ui, t: &Transfer) {
    let width = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, ROW_H), Sense::hover());
    let hover = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.12);
    let p = ui.painter();
    if hover > 0.0 {
        p.rect_filled(rect.shrink2(vec2(ROW_INSET, 1.5)), CornerRadius::same(6), theme::surface_2().gamma_multiply(hover));
    } else {
        p.hline((rect.left() + TEXT_X)..=(rect.right() - 12.0), rect.bottom() - 0.5, Stroke::new(1.0, theme::row_separator()));
    }
    let state = t.state();
    let arrow_color = match t.direction {
        Direction::Upload => theme::accent(),
        Direction::Download => theme::blue(),
    };
    paint_direction(p, pos2(rect.left() + 17.0, rect.center().y), t.direction == Direction::Upload, arrow_color);

    let done = t.progress.done.load(Ordering::Relaxed);
    let total = t.progress.total.load(Ordering::Relaxed);
    let files = (t.progress.files_done.load(Ordering::Relaxed), t.progress.files_total.load(Ordering::Relaxed));
    let label = if files.1 > 1 { format!("{}  ({}/{} files)", t.label, files.0, files.1) } else { t.label.clone() };
    p.text(rect.left_top() + Vec2::new(TEXT_X, 7.0), Align2::LEFT_TOP, label, FontId::proportional(12.5), theme::text());

    let speed = t.speed();
    let (status, color) = match &state {
        TransferState::Running => {
            let eta = if speed > 1.0 && total > done { format!(" · {}", eta((total - done) as f64 / speed)) } else { String::new() };
            (format!("{} / {} · {}/s{eta}", theme::human_bytes(done), theme::human_bytes(total), theme::human_bytes(speed as u64)), theme::text_dim())
        }
        TransferState::Done => (format!("{} · avg {}/s", theme::human_bytes(total), theme::human_bytes(speed as u64)), theme::accent()),
        TransferState::Cancelled => ("Cancelled".into(), theme::text_faint()),
        TransferState::Failed(e) => (e.clone(), theme::danger()),
    };
    let right = rect.right() - 34.0;
    p.text(egui::pos2(right, rect.top() + 8.0), Align2::RIGHT_TOP, status, FontId::proportional(11.0), color);

    // Progress bar eases toward the real value for a smooth, non-jumpy feel.
    let target = if state == TransferState::Done { 1.0 } else { t.progress.fraction() };
    let shown = ui.ctx().animate_value_with_time(ui.id().with(("tx", t.id)), target, 0.25);
    let bar = Rect::from_min_max(egui::pos2(rect.left() + TEXT_X, rect.bottom() - 10.0), egui::pos2(right, rect.bottom() - 7.0));
    p.rect_filled(bar, CornerRadius::same(2), theme::surface_3());
    let fill_color = match state {
        TransferState::Failed(_) => theme::danger(),
        TransferState::Cancelled => theme::text_faint(),
        _ => arrow_color,
    };
    let mut fill = bar;
    fill.set_width(bar.width() * shown.clamp(0.0, 1.0));
    p.rect_filled(fill, CornerRadius::same(2), fill_color);

    if state == TransferState::Running {
        let btn = Rect::from_center_size(egui::pos2(rect.right() - 14.0, rect.center().y), Vec2::splat(20.0));
        let resp = ui.interact(btn, ui.id().with(("cancel", t.id)), Sense::click()).on_hover_text("Cancel");
        let c = if resp.hovered() { theme::danger() } else { theme::text_faint() };
        if resp.hovered() {
            ui.painter().circle_filled(btn.center(), 9.0, theme::danger().gamma_multiply(0.15));
        }
        paint_cross(ui.painter(), btn.center(), c);
        if resp.clicked() {
            t.progress.cancel();
        }
    }
}

/// Disc with an up (upload) or down (download) arrow.
fn paint_direction(p: &Painter, center: Pos2, up: bool, color: Color32) {
    p.circle_filled(center, 9.0, color.gamma_multiply(0.16));
    let dir = if up { -1.0 } else { 1.0 };
    let s = Stroke::new(1.6, color);
    let tip = center + vec2(0.0, 4.0 * dir);
    p.line_segment([center - vec2(0.0, 4.0 * dir), tip], s);
    p.line_segment([tip, tip + vec2(-3.2, -3.2 * dir)], s);
    p.line_segment([tip, tip + vec2(3.2, -3.2 * dir)], s);
}

fn paint_cross(p: &Painter, center: Pos2, color: Color32) {
    let s = Stroke::new(1.5, color);
    let d = 3.5;
    p.line_segment([center + vec2(-d, -d), center + vec2(d, d)], s);
    p.line_segment([center + vec2(-d, d), center + vec2(d, -d)], s);
}

fn eta(secs: f64) -> String {
    let s = secs.round() as u64;
    match s {
        0..=59 => format!("{s}s left"),
        60..=3599 => format!("{}m {}s left", s / 60, s % 60),
        _ => format!("{}h {}m left", s / 3600, (s % 3600) / 60),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eta_formats() {
        assert_eq!(eta(5.2), "5s left");
        assert_eq!(eta(125.0), "2m 5s left");
        assert_eq!(eta(7322.0), "2h 2m left");
    }
}
