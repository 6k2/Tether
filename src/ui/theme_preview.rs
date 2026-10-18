//! Miniature "app window" painted from an arbitrary `Palette` (not the global one),
//! used by the theme picker so every card previews its own colours.

use eframe::egui::{self, Color32, CornerRadius, Pos2, Rect, Stroke, Vec2};

use crate::theme::Palette;

/// Paints a fake tether window into `rect`: chrome, sidebar with hosts, a tab and
/// a few coloured terminal lines with a cursor.
pub fn paint(painter: &egui::Painter, rect: Rect, p: &Palette) {
    let r = |v: f32| CornerRadius::same(v.round().clamp(0.0, 255.0) as u8);
    let s = rect.height() / 100.0; // scale unit
    painter.rect_filled(rect, r(8.0 * s.min(1.2)), p.bg);

    // Title-bar dots.
    let dot_y = rect.top() + 7.0 * s;
    for (i, c) in [p.danger, p.warn, p.success].into_iter().enumerate() {
        painter.circle_filled(Pos2::new(rect.left() + (8.0 + i as f32 * 6.5) * s, dot_y), 1.9 * s, c);
    }

    // Sidebar: host rows, one selected with the accent dot.
    let side =
        Rect::from_min_max(Pos2::new(rect.left() + 5.0 * s, rect.top() + 14.0 * s), Pos2::new(rect.left() + rect.width() * 0.27, rect.bottom() - 5.0 * s));
    for i in 0..4 {
        let y = side.top() + i as f32 * 11.0 * s;
        let row = Rect::from_min_size(Pos2::new(side.left(), y), Vec2::new(side.width(), 8.5 * s));
        if i == 1 {
            painter.rect_filled(row, r(2.5 * s), p.surface_2);
        }
        painter.circle_filled(Pos2::new(row.left() + 4.0 * s, row.center().y), 1.7 * s, if i == 1 { p.accent } else { p.text_faint });
        let bar_w = row.width() * [0.62, 0.7, 0.5, 0.58][i] - 9.0 * s;
        let bar = Rect::from_min_size(Pos2::new(row.left() + 8.0 * s, row.center().y - 1.3 * s), Vec2::new(bar_w.max(2.0), 2.6 * s));
        painter.rect_filled(bar, r(1.3 * s), if i == 1 { p.text } else { p.text_dim.gamma_multiply(0.8) });
    }

    // Main area: tab strip + body.
    let main = Rect::from_min_max(Pos2::new(side.right() + 5.0 * s, rect.top() + 4.0 * s), Pos2::new(rect.right() - 5.0 * s, rect.bottom() - 5.0 * s));
    let tab = Rect::from_min_size(main.min, Vec2::new(main.width() * 0.36, 9.0 * s));
    painter.rect_filled(tab, r(2.5 * s), p.surface_2);
    painter.rect_filled(
        Rect::from_min_size(Pos2::new(tab.left() + 4.0 * s, tab.center().y - 1.2 * s), Vec2::new(tab.width() * 0.55, 2.4 * s)),
        r(1.2 * s),
        p.text,
    );
    let ghost = Rect::from_min_size(Pos2::new(tab.right() + 4.0 * s, tab.center().y - 1.2 * s), Vec2::new(main.width() * 0.2, 2.4 * s));
    painter.rect_filled(ghost, r(1.2 * s), p.text_faint.gamma_multiply(0.7));

    let body = Rect::from_min_max(Pos2::new(main.left(), tab.bottom() + 3.0 * s), main.max);
    painter.rect(body, r(4.0 * s), p.term_bg, Stroke::new(1.0, p.border.gamma_multiply(0.6)), egui::StrokeKind::Inside);

    // Terminal lines as coloured runs: (colour, width fraction).
    let a = &p.ansi;
    let lines: [&[(Color32, f32)]; 6] = [
        &[(a[2], 0.22), (p.term_fg, 0.06), (a[4], 0.14), (p.term_fg, 0.24)],
        &[(p.term_fg, 0.5), (a[3], 0.18)],
        &[(a[4], 0.16), (a[6], 0.2), (a[5], 0.14), (p.term_fg, 0.12)],
        &[(a[1], 0.12), (p.term_fg, 0.42)],
        &[(p.term_fg, 0.3), (a[3], 0.1), (a[2], 0.16)],
        &[(a[2], 0.22), (p.term_fg, 0.06), (a[4], 0.14)],
    ];
    let pad = 5.0 * s;
    let line_h = ((body.height() - pad * 2.0) / lines.len() as f32).min(9.0 * s);
    let bar_h = (line_h * 0.36).max(1.5);
    let usable = body.width() - pad * 2.0;
    let mut cursor_x = body.left() + pad;
    let mut last_y = body.top() + pad;
    for (li, runs) in lines.iter().enumerate() {
        let y = body.top() + pad + li as f32 * line_h + (line_h - bar_h) * 0.5;
        let mut x = body.left() + pad;
        for (colour, frac) in runs.iter() {
            let w = usable * frac;
            painter.rect_filled(Rect::from_min_size(Pos2::new(x, y), Vec2::new((w - 2.0 * s).max(1.0), bar_h)), r(bar_h * 0.5), *colour);
            x += w;
        }
        cursor_x = x;
        last_y = y;
    }
    let cursor = Rect::from_min_size(Pos2::new(cursor_x + 1.0 * s, last_y - bar_h * 0.6), Vec2::new(3.6 * s, bar_h * 2.2));
    painter.rect_filled(cursor, r(0.8 * s), p.term_cursor);

    // Accent badge in the corner.
    let badge = Pos2::new(rect.right() - 9.0 * s, rect.top() + 7.0 * s);
    painter.circle_filled(badge, 3.2 * s, p.accent);
    painter.circle_filled(badge, 1.2 * s, p.on_accent);
}
