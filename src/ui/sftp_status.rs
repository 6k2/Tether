//! Connection status over the SFTP remote pane: a slim "reconnecting" banner while the
//! session comes back on its own, and a failure card (plain message, Reconnect / Close
//! tab, technical details folded away) once it can't.

use eframe::egui::{self, Align2, CornerRadius, FontId, Pos2, Rect, RichText, Stroke, Ui, Vec2};

use super::loading;
use super::settings_widgets::{ghost_button, primary_button};
use crate::theme;

/// What the user picked on the failure card or banner.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct StatusAction {
    pub reconnect: bool,
    pub close: bool,
}

const BANNER_H: f32 = 34.0;
/// The banner sits below the pane's path row.
const BANNER_TOP: f32 = 46.0;
const CARD_W: f32 = 420.0;

/// The banner text for automatic reconnect attempt `attempt` of `max`.
pub fn reconnect_text(attempt: u32, max: u32) -> String {
    if attempt <= 1 { "Connection lost — reconnecting…".into() } else { format!("Connection lost — reconnecting… (attempt {attempt} of {max})") }
}

/// A floating pill at the top of `rect`: spinner, `text`, the current stage and a
/// "Retry now" link. `fade` (0..1) drives its slide-in.
pub fn reconnect_banner(ui: &mut Ui, rect: Rect, text: &str, stage: &str, fade: f32) -> StatusAction {
    let mut action = StatusAction::default();
    if fade <= 0.0 || rect.width() < 160.0 {
        return action;
    }
    let eased = 1.0 - (1.0 - fade).powi(3);
    let width = (rect.width() - 24.0).min(560.0);
    let top = rect.top() + BANNER_TOP - 8.0 * (1.0 - eased);
    let banner = Rect::from_min_size(Pos2::new(rect.center().x - width / 2.0, top), Vec2::new(width, BANNER_H));
    let p = ui.painter_at(rect);
    p.rect_filled(banner.translate(Vec2::new(0.0, 4.0)), CornerRadius::same(17), theme::shadow().gamma_multiply(0.5 * eased));
    p.rect(
        banner,
        CornerRadius::same(17),
        theme::surface_2().gamma_multiply(eased),
        Stroke::new(1.0, theme::warn().gamma_multiply(0.45 * eased)),
        egui::StrokeKind::Inside,
    );
    loading::mini_spinner(ui, Pos2::new(banner.left() + 20.0, banner.center().y), 6.0, theme::warn().gamma_multiply(eased));
    let mut child =
        ui.new_child(egui::UiBuilder::new().max_rect(banner.shrink2(Vec2::new(34.0, 0.0))).layout(egui::Layout::left_to_right(egui::Align::Center)));
    child.set_opacity(eased);
    child.spacing_mut().item_spacing.x = 8.0;
    child.add(egui::Label::new(RichText::new(text).size(12.5).color(theme::text())).truncate());
    child.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        let retry = ui.add(egui::Button::new(RichText::new("Retry now").size(12.0).color(theme::accent())).frame(false));
        if retry.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        action.reconnect = retry.clicked();
        if !stage.is_empty() {
            ui.add(egui::Label::new(RichText::new(stage).size(11.5).color(theme::text_faint())).truncate());
        }
    });
    action
}

/// Dims `rect` and shows a centred card: a warning badge, `title`, `message`, the
/// Reconnect / Close tab buttons, and `detail` under a collapsed "Technical details".
pub fn failure_card(ui: &mut Ui, rect: Rect, title: &str, message: &str, detail: &str, fade: f32) -> StatusAction {
    let mut action = StatusAction::default();
    if fade <= 0.0 {
        return action;
    }
    let eased = 1.0 - (1.0 - fade).powi(3);
    ui.painter_at(rect).rect_filled(rect, CornerRadius::same(theme::RADIUS), theme::bg().gamma_multiply(0.7 * eased));
    let width = CARD_W.min(rect.width() - 24.0).max(120.0);
    let anchor = Pos2::new(rect.center().x - width / 2.0, rect.top() + rect.height() * 0.22 + 10.0 * (1.0 - eased));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_max(anchor, Pos2::new(anchor.x + width, rect.bottom()))));
    child.set_opacity(eased);
    egui::Frame::new()
        .fill(theme::surface_2())
        .stroke(Stroke::new(1.0, theme::border()))
        .corner_radius(14)
        .inner_margin(20)
        .shadow(egui::Shadow { offset: [0, 10], blur: 28, spread: 0, color: theme::shadow() })
        .show(&mut child, |ui| {
            ui.set_width(width - 40.0);
            ui.horizontal(|ui| {
                badge(ui);
                ui.vertical(|ui| {
                    ui.label(RichText::new(title).size(16.0).strong().color(theme::text()));
                    ui.add_space(2.0);
                    ui.label(RichText::new(message).size(12.5).color(theme::text_dim()));
                });
            });
            ui.add_space(14.0);
            ui.horizontal(|ui| {
                action.reconnect = primary_button(ui, "⟳  Reconnect").clicked();
                action.close = ghost_button(ui, "Close tab", theme::text()).clicked();
            });
            if !detail.is_empty() {
                ui.add_space(10.0);
                details(ui, detail);
            }
        });
    action
}

/// Round warning badge with a painted "!".
fn badge(ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(36.0), egui::Sense::hover());
    let p = ui.painter();
    p.circle_filled(rect.center(), 18.0, theme::danger().gamma_multiply(0.16));
    p.text(rect.center(), Align2::CENTER_CENTER, "!", FontId::proportional(18.0), theme::danger());
}

fn details(ui: &mut Ui, detail: &str) {
    egui::CollapsingHeader::new(RichText::new("Technical details").size(11.5).color(theme::text_faint()))
        .id_salt("sftp_failure_details")
        .default_open(false)
        .show(ui, |ui| {
            egui::Frame::new().fill(theme::surface()).corner_radius(8).inner_margin(8).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.add(egui::Label::new(RichText::new(detail).monospace().size(11.0).color(theme::text_dim())).wrap().selectable(true));
            });
            if ui.small_button("Copy").clicked() {
                ui.ctx().copy_text(detail.to_string());
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn banner_text_counts_attempts_after_the_first() {
        assert_eq!(reconnect_text(1, 4), "Connection lost — reconnecting…");
        assert_eq!(reconnect_text(3, 4), "Connection lost — reconnecting… (attempt 3 of 4)");
    }

    #[test]
    fn paints_headless() {
        let ctx = egui::Context::default();
        let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0))), ..Default::default() };
        let mut out = ctx.run_ui(input, |ui| {
            let rect = ui.max_rect();
            assert_eq!(reconnect_banner(ui, rect, "Connection lost", "Authenticating", 1.0), StatusAction::default());
            assert_eq!(failure_card(ui, rect, "Connection lost", "The connection dropped.", "I/O: reset", 1.0), StatusAction::default());
        });
        out.textures_delta.clear();
    }

    /// Renders the banner and the failure card to `target/ui-shots/sftp-reconnect.png`.
    /// Needs a GPU adapter: `cargo test --lib sftp_status_screens -- --ignored`.
    #[test]
    #[ignore]
    fn sftp_status_screens() {
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/ui-shots");
        std::fs::create_dir_all(&dir).unwrap();
        let mut h = egui_kittest::Harness::builder().with_size(Vec2::new(1100.0, 520.0)).wgpu().build_ui(|ui| {
            let rect = ui.max_rect();
            ui.painter().rect_filled(rect, 0.0, theme::bg());
            let (left, right) = rect.shrink(8.0).split_left_right_at_fraction(0.5);
            for pane in [left.shrink(4.0), right.shrink(4.0)] {
                ui.painter().rect_filled(pane, CornerRadius::same(theme::RADIUS), theme::surface());
                loading::skeleton_rows(ui, pane.shrink(8.0), 30.0, 0.5);
            }
            reconnect_banner(ui, left.shrink(4.0), &reconnect_text(2, 4), "Authenticating…", 1.0);
            failure_card(
                ui,
                right.shrink(4.0),
                "Connection lost",
                "The connection to the server dropped.",
                "timed out reconnecting: I/O: An existing connection was forcibly closed by the remote host. (os error 10054)",
                1.0,
            );
        });
        for _ in 0..20 {
            h.step();
        }
        h.render().expect("render").save(dir.join("sftp-reconnect.png")).unwrap();
    }
}
