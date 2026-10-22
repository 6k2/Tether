//! Painted marks for AI agents (no bitmap or trademark assets): Claude Code's orange
//! starburst, Codex's terminal mark and a sparkle for the rest. Drawn with the painter,
//! so they stay crisp at 14–16 px and in every theme.

use std::f32::consts::{PI, TAU};

use eframe::egui::{self, Color32, Painter, Pos2, Rect, Shape, Stroke, Vec2};

use crate::agents::AgentKind;
use crate::theme;

/// Claude's terracotta orange (#D97757).
pub const CLAUDE_ORANGE: Color32 = Color32::from_rgb(0xD9, 0x77, 0x57);
/// Rays of the Claude starburst (alternating long / short).
const CLAUDE_RAYS: usize = 10;

/// The colour of an agent's mark in the current theme.
pub fn color(kind: AgentKind) -> Color32 {
    match kind {
        AgentKind::Claude => CLAUDE_ORANGE,
        // Neutral like the brand mark; the text colour reads in light and dark themes.
        AgentKind::Codex => theme::text(),
        _ => theme::hue_color(kind.hue()),
    }
}

/// Paints the agent's mark centred in `rect` (square works best).
pub fn paint(p: &Painter, rect: Rect, kind: AgentKind, alpha: f32) {
    let c = color(kind).gamma_multiply(alpha);
    let size = rect.width().min(rect.height());
    let center = rect.center();
    match kind {
        AgentKind::Claude => starburst(p, center, size * 0.5, c),
        AgentKind::Codex => codex_mark(p, center, size * 0.46, c),
        _ => sparkle(p, center, size * 0.5, c),
    }
}

/// Tapered rays around a small hub: Claude Code's asterisk-like mark.
fn starburst(p: &Painter, center: Pos2, radius: f32, c: Color32) {
    let base_half = (radius * 0.13).max(0.7);
    for i in 0..CLAUDE_RAYS {
        let a = TAU * i as f32 / CLAUDE_RAYS as f32 - PI / 2.0;
        let dir = Vec2::angled(a);
        let perp = Vec2::new(-dir.y, dir.x);
        let len = if i % 2 == 0 { radius } else { radius * 0.78 };
        let inner = center + dir * radius * 0.18;
        let tip = center + dir * len;
        let pts = vec![inner + perp * base_half, tip + perp * base_half * 0.35, tip - perp * base_half * 0.35, inner - perp * base_half];
        p.add(Shape::convex_polygon(pts, c, Stroke::NONE));
    }
    p.circle_filled(center, radius * 0.2, c);
}

/// Codex's familiar >_ terminal mark, readable even in the activity strip.
fn codex_mark(p: &Painter, center: Pos2, radius: f32, c: Color32) {
    let stroke = Stroke::new((radius * 0.2).clamp(1.2, 2.0), c);
    let at = |x, y| center + Vec2::new(x * radius, y * radius);
    p.add(Shape::line(vec![at(-0.85, -0.6), at(-0.15, 0.0), at(-0.85, 0.6)], stroke));
    p.line_segment([at(0.15, 0.6), at(0.9, 0.6)], stroke);
}

/// A four-point star with a small companion.
fn sparkle(p: &Painter, center: Pos2, radius: f32, c: Color32) {
    // Two thin diamonds (each convex) make the four points.
    let star = |o: Pos2, r: f32| {
        let k = r * 0.3;
        let tall = vec![o + Vec2::new(0.0, -r), o + Vec2::new(k, 0.0), o + Vec2::new(0.0, r), o + Vec2::new(-k, 0.0)];
        let wide = vec![o + Vec2::new(-r, 0.0), o + Vec2::new(0.0, -k), o + Vec2::new(r, 0.0), o + Vec2::new(0.0, k)];
        p.add(Shape::convex_polygon(tall, c, Stroke::NONE));
        p.add(Shape::convex_polygon(wide, c, Stroke::NONE));
    };
    star(center + Vec2::new(-radius * 0.12, radius * 0.12), radius * 0.86);
    star(center + Vec2::new(radius * 0.62, -radius * 0.62), radius * 0.32);
}

/// An agent mark as a widget of `size` points (agents panel, menus).
pub fn icon(ui: &mut egui::Ui, kind: AgentKind, size: f32) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), egui::Sense::hover());
    paint(ui.painter(), rect, kind, 1.0);
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_agent_has_a_visible_colour() {
        for k in AgentKind::ALL {
            assert!(color(k).a() > 0, "{k:?}");
        }
        assert_eq!(color(AgentKind::Claude), CLAUDE_ORANGE);
    }
}
