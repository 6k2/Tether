//! Loading visuals shared by terminals and SFTP panes: the "connecting" screen with a
//! host avatar, an orbiting arc and a stage stepper, plus shimmering skeleton rows for
//! listings that have not arrived yet. Everything is painted (no font glyph icons) and
//! eased, and nothing here blocks: it only reads state the background tasks update.

use std::f32::consts::{PI, TAU};

use eframe::egui::{self, Align2, Color32, CornerRadius, FontId, Pos2, Rect, Stroke, Ui, Vec2};

use crate::ssh::connection::Stage;
use crate::theme;

/// Connects that finish faster than this never show the connecting screen (no flash).
pub const SHOW_AFTER_SECS: f64 = 0.12;
/// From here on the screen also shows how long it has been trying.
const SLOW_AFTER_SECS: f64 = 4.0;

pub struct ConnectInfo<'a> {
    pub name: &'a str,
    pub address: &'a str,
    pub hue: f32,
    pub stage: Stage,
    /// Stages this kind of session goes through, in order.
    pub stages: &'a [Stage],
    /// Seconds since the connect started.
    pub elapsed: f64,
}

pub const TERMINAL_STAGES: &[Stage] = &[Stage::Connecting, Stage::Authenticating, Stage::Opening];
pub const SFTP_STAGES: &[Stage] = &[Stage::Connecting, Stage::Authenticating, Stage::Opening, Stage::Loading];

/// Paints the connecting screen centred in `rect` at opacity `fade` (0..1).
pub fn connect_screen(ui: &Ui, rect: Rect, info: &ConnectInfo, fade: f32) {
    if fade <= 0.0 || rect.width() < 60.0 || rect.height() < 60.0 {
        return;
    }
    let ctx = ui.ctx();
    let painter = ui.painter_at(rect);
    let time = ui.input(|i| i.time) as f32;
    let accent = theme::hue_color(info.hue);
    let eased = 1.0 - (1.0 - fade).powi(3);
    let c = rect.center() + Vec2::new(0.0, -18.0 + 10.0 * (1.0 - eased));

    // Soft glow behind the avatar: a few stacked translucent discs.
    for i in 0..10 {
        let r = 92.0 - i as f32 * 5.5;
        painter.circle_filled(c, r, accent.gamma_multiply(0.012 * eased));
    }
    orbit(&painter, c, 34.0, accent, time, eased);

    let tile = Rect::from_center_size(c, Vec2::splat(38.0));
    painter.rect_filled(tile, CornerRadius::same(10), accent.gamma_multiply(0.22 * eased));
    let initial = info.name.chars().next().unwrap_or('?').to_uppercase().to_string();
    painter.text(tile.center(), Align2::CENTER_CENTER, initial, FontId::proportional(17.0), accent.gamma_multiply(eased));

    let text = theme::text().gamma_multiply(eased);
    let y = c.y + 58.0;
    painter.text(Pos2::new(c.x, y), Align2::CENTER_CENTER, info.name, FontId::proportional(15.0), text);
    painter.text(Pos2::new(c.x, y + 19.0), Align2::CENTER_CENTER, info.address, FontId::monospace(11.5), theme::text_faint().gamma_multiply(eased));

    let step_y = y + 50.0;
    stepper(ui, Pos2::new(c.x, step_y), info, accent, time, eased);

    // Stage label, cross-faded and lifted in whenever the stage changes.
    let id = ui.id().with(("connect_stage", info.address));
    let changed_at = ctx.data_mut(|d| {
        let slot = d.get_temp_mut_or_insert_with(id, || (info.stage, time));
        if slot.0 != info.stage {
            *slot = (info.stage, time);
        }
        slot.1
    });
    let k = ((time - changed_at) / theme::anim_time(0.25).max(1e-3)).clamp(0.0, 1.0);
    let k = 1.0 - (1.0 - k).powi(3);
    let label = format!("{}…", info.stage.label());
    painter.text(
        Pos2::new(c.x, step_y + 26.0 + 6.0 * (1.0 - k)),
        Align2::CENTER_CENTER,
        label,
        FontId::proportional(12.5),
        theme::text_dim().gamma_multiply(eased * k),
    );
    if info.elapsed > SLOW_AFTER_SECS {
        let slow = ((info.elapsed - SLOW_AFTER_SECS) as f32 / 0.4).clamp(0.0, 1.0) * eased;
        let msg = format!("Still trying · {}s", info.elapsed as u64);
        painter.text(Pos2::new(c.x, step_y + 46.0), Align2::CENTER_CENTER, msg, FontId::proportional(11.0), theme::text_faint().gamma_multiply(slow));
    }
    ctx.request_repaint();
}

/// Material-style arc: rotates steadily while its length breathes; the tail fades out.
fn orbit(p: &egui::Painter, c: Pos2, r: f32, color: Color32, time: f32, alpha: f32) {
    p.circle_stroke(c, r, Stroke::new(2.0, color.gamma_multiply(0.12 * alpha)));
    let breathe = 0.5 - 0.5 * (time * 2.1).cos();
    let sweep = PI * (0.25 + 0.95 * breathe);
    let head = time * 4.2 + breathe * PI * 0.6;
    const SEGS: usize = 28;
    let point = |a: f32| c + Vec2::new(a.cos(), a.sin()) * r;
    for i in 0..SEGS {
        let t0 = i as f32 / SEGS as f32;
        let t1 = (i + 1) as f32 / SEGS as f32;
        let a0 = head - sweep * (1.0 - t0);
        let a1 = head - sweep * (1.0 - t1);
        let k = t1 * t1;
        p.line_segment([point(a0), point(a1)], Stroke::new(2.4, color.gamma_multiply(k * alpha)));
    }
    let tip = point(head);
    p.circle_filled(tip, 3.4, color.gamma_multiply(alpha));
    p.circle_filled(tip, 6.5, color.gamma_multiply(0.18 * alpha));
}

/// Dots for each stage joined by a track that fills as stages complete.
fn stepper(ui: &Ui, centre: Pos2, info: &ConnectInfo, accent: Color32, time: f32, alpha: f32) {
    let n = info.stages.len().max(1);
    let gap = 34.0;
    let width = gap * (n - 1) as f32;
    let left = centre.x - width / 2.0;
    let current = info.stages.iter().position(|s| *s == info.stage).unwrap_or(0);
    let id = ui.id().with(("connect_progress", info.address));
    let filled = ui.ctx().animate_value_with_time(id, current as f32, theme::anim_time(0.35));
    let p = ui.painter();
    let track = Stroke::new(2.0, theme::border().gamma_multiply(alpha));
    p.line_segment([Pos2::new(left, centre.y), Pos2::new(left + width, centre.y)], track);
    if filled > 0.0 {
        p.line_segment([Pos2::new(left, centre.y), Pos2::new(left + gap * filled, centre.y)], Stroke::new(2.0, accent.gamma_multiply(alpha)));
    }
    for i in 0..n {
        let x = left + gap * i as f32;
        let at = Pos2::new(x, centre.y);
        if i < current {
            p.circle_filled(at, 4.0, accent.gamma_multiply(alpha));
        } else if i == current {
            let pulse = 0.5 + 0.5 * (time * 4.0).sin();
            p.circle_filled(at, 7.0 + 2.0 * pulse, accent.gamma_multiply(0.16 * alpha));
            p.circle_filled(at, 4.5, theme::bg().gamma_multiply(alpha));
            p.circle_stroke(at, 4.5, Stroke::new(2.0, accent.gamma_multiply(alpha)));
        } else {
            p.circle_filled(at, 4.0, theme::bg().gamma_multiply(alpha));
            p.circle_stroke(at, 4.0, Stroke::new(1.5, theme::border().gamma_multiply(alpha)));
        }
    }
}

/// Placeholder rows for a listing in flight: icon square plus text bars of varied width,
/// swept by a soft highlight wave. `fade` is the overall opacity.
pub fn skeleton_rows(ui: &Ui, rect: Rect, row_h: f32, fade: f32) {
    if fade <= 0.0 {
        return;
    }
    let p = ui.painter_at(rect);
    let time = ui.input(|i| i.time) as f32;
    let base = theme::surface_2();
    let rows = ((rect.height() / row_h).ceil() as usize).min(40);
    for i in 0..rows {
        let y = rect.top() + i as f32 * row_h + row_h / 2.0;
        // Rows further down fade out so the placeholder doesn't look like a wall.
        let depth = 1.0 - (i as f32 / rows.max(1) as f32) * 0.7;
        let wave = 0.55 + 0.45 * ((time * 2.6 - i as f32 * 0.45).sin() * 0.5 + 0.5);
        let col = base.gamma_multiply(fade * depth * wave);
        let icon = Rect::from_center_size(Pos2::new(rect.left() + 22.0, y), Vec2::splat(14.0));
        p.rect_filled(icon, CornerRadius::same(4), col);
        let name_w = 70.0 + ((i * 47 + 13) % 9) as f32 * 18.0;
        let name = Rect::from_min_size(Pos2::new(rect.left() + 40.0, y - 4.0), Vec2::new(name_w.min(rect.width() * 0.5), 8.0));
        p.rect_filled(name, CornerRadius::same(4), col);
        let meta = Rect::from_min_size(Pos2::new(rect.right() - 92.0, y - 3.5), Vec2::new(56.0, 7.0));
        if meta.left() > name.right() + 20.0 {
            p.rect_filled(meta, CornerRadius::same(4), col.gamma_multiply(0.7));
        }
    }
    ui.ctx().request_repaint();
}

/// Small spinning arc for inline busy states (buttons, chips).
pub fn mini_spinner(ui: &Ui, centre: Pos2, r: f32, color: Color32) {
    let time = ui.input(|i| i.time) as f32;
    orbit_small(ui.painter(), centre, r, color, time);
    ui.ctx().request_repaint();
}

fn orbit_small(p: &egui::Painter, c: Pos2, r: f32, color: Color32, time: f32) {
    let head = time * 6.0;
    const SEGS: usize = 14;
    for i in 0..SEGS {
        let t0 = i as f32 / SEGS as f32;
        let t1 = (i + 1) as f32 / SEGS as f32;
        let a = |t: f32| head - TAU * 0.7 * (1.0 - t);
        let pt = |a: f32| c + Vec2::new(a.cos(), a.sin()) * r;
        p.line_segment([pt(a(t0)), pt(a(t1))], Stroke::new(1.6, color.gamma_multiply(t1)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paints_headless_for_every_stage() {
        let ctx = egui::Context::default();
        for (stage, elapsed) in [(Stage::Connecting, 0.2), (Stage::Authenticating, 5.0), (Stage::Loading, 1.0)] {
            let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0))), ..Default::default() };
            let mut out = ctx.run_ui(input, |ui| {
                let rect = ui.max_rect();
                let info = ConnectInfo { name: "web", address: "root@web", hue: 120.0, stage, stages: SFTP_STAGES, elapsed };
                connect_screen(ui, rect, &info, 1.0);
                skeleton_rows(ui, rect, 30.0, 1.0);
                mini_spinner(ui, rect.center(), 6.0, theme::accent());
            });
            out.textures_delta.clear();
        }
    }

    /// Renders the connecting screen and skeleton rows to `target/ui-shots/`.
    /// Needs a GPU adapter: `cargo test --lib loading_screens -- --ignored`.
    #[test]
    #[ignore]
    fn loading_screens() {
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/ui-shots");
        std::fs::create_dir_all(&dir).unwrap();
        let mut h = egui_kittest::Harness::builder().with_size(Vec2::new(900.0, 560.0)).wgpu().build_ui(|ui| {
            let rect = ui.max_rect();
            ui.painter().rect_filled(rect, 0.0, theme::term_bg());
            let (left, right) = rect.split_left_right_at_fraction(0.55);
            let info =
                ConnectInfo { name: "api-prod-01", address: "deploy@10.0.4.21", hue: 210.0, stage: Stage::Authenticating, stages: SFTP_STAGES, elapsed: 5.2 };
            connect_screen(ui, left, &info, 1.0);
            ui.painter().rect_filled(right, 0.0, theme::surface());
            skeleton_rows(ui, right.shrink(8.0), 30.0, 1.0);
        });
        for _ in 0..20 {
            h.step();
        }
        h.render().expect("render").save(dir.join("loading.png")).unwrap();
    }
}
