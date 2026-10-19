//! Painted glyphs and small buttons shared by the sidebar's tree, menus and workspaces.

use eframe::egui::{self, Align2, Color32, CornerRadius, FontId, Id, Pos2, Rect, Sense, Stroke, Ui, Vec2};

use crate::theme;

#[derive(Clone, Copy)]
pub enum Glyph {
    Plus,
    More,
    /// "?" in a ring.
    Help,
    /// A 2×2 grid of panes (a saved layout).
    Layout,
    /// A clock face ("Last session").
    Clock,
    /// "▾" for a dropdown.
    Caret,
}

pub fn paint_glyph(p: &egui::Painter, c: Pos2, g: Glyph, col: Color32) {
    let s = Stroke::new(1.5, col);
    match g {
        Glyph::Plus => plus(p, c, 5.0, s),
        Glyph::More => {
            for dx in [-5.0, 0.0, 5.0] {
                p.circle_filled(c + Vec2::new(dx, 0.0), 1.5, col);
            }
        }
        Glyph::Help => {
            p.circle_stroke(c, 6.5, Stroke::new(1.2, col));
            p.text(c + Vec2::new(0.0, 0.5), Align2::CENTER_CENTER, "?", FontId::proportional(10.0), col);
        }
        Glyph::Layout => {
            let r = Rect::from_center_size(c, Vec2::splat(13.0));
            p.rect_stroke(r, CornerRadius::same(3), Stroke::new(1.3, col), egui::StrokeKind::Inside);
            p.line_segment([Pos2::new(r.center().x, r.top() + 1.5), Pos2::new(r.center().x, r.bottom() - 1.5)], Stroke::new(1.1, col));
            p.line_segment([Pos2::new(r.center().x, r.center().y), Pos2::new(r.right() - 1.5, r.center().y)], Stroke::new(1.1, col));
        }
        Glyph::Clock => {
            p.circle_stroke(c, 6.5, Stroke::new(1.3, col));
            p.line_segment([c, c + Vec2::new(0.0, -4.0)], Stroke::new(1.3, col));
            p.line_segment([c, c + Vec2::new(3.0, 1.5)], Stroke::new(1.3, col));
        }
        Glyph::Caret => {
            let pts = vec![c + Vec2::new(-3.5, -1.5), c + Vec2::new(0.0, 2.0), c + Vec2::new(3.5, -1.5)];
            p.add(egui::Shape::line(pts, s));
        }
    }
}

/// A 24px square icon button with a hover tile.
pub fn glyph_button(ui: &mut Ui, g: Glyph, tip: &str) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(24.0), Sense::click());
    let h = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), theme::anim_time(0.1));
    if h > 0.0 {
        ui.painter().rect_filled(rect, CornerRadius::same(6), theme::surface_3().gamma_multiply(h));
    }
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    paint_glyph(ui.painter(), rect.center(), g, theme::text_dim().lerp_to_gamma(theme::text(), h));
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), tip));
    resp.on_hover_text(tip)
}

/// The header's "+ New | ▾" pill: the left half creates a host, the caret opens a menu.
/// Laid out right-to-left, so it allocates as one rect.
pub fn new_split_button(ui: &mut Ui) -> (egui::Response, egui::Response) {
    const MAIN_W: f32 = 54.0;
    const CARET_W: f32 = 22.0;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(MAIN_W + CARET_W, 24.0), Sense::hover());
    let main_rect = Rect::from_min_size(rect.min, Vec2::new(MAIN_W, rect.height()));
    let caret_rect = Rect::from_min_max(Pos2::new(main_rect.right(), rect.top()), rect.max);
    let main = ui.interact(main_rect, ui.id().with("sidebar_new"), Sense::click());
    let caret = ui.interact(caret_rect, ui.id().with("sidebar_new_menu"), Sense::click()).on_hover_text("More ways to add");
    main.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "New"));
    caret.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "New menu"));
    let ctx = ui.ctx().clone();
    let hm = ctx.animate_bool_with_time(main.id, main.hovered(), theme::anim_time(0.1));
    let hc = ctx.animate_bool_with_time(caret.id, caret.hovered() || egui::Popup::is_id_open(&ctx, caret.id.with("popup")), theme::anim_time(0.1));
    if main.hovered() || caret.hovered() {
        ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let p = ui.painter();
    let r = CornerRadius::same(7);
    p.rect_filled(rect, r, theme::accent_soft());
    let lift = |h: f32| theme::accent().gamma_multiply(0.16 * h);
    p.rect_filled(main_rect, CornerRadius { nw: 7, sw: 7, ne: 0, se: 0 }, lift(hm));
    p.rect_filled(caret_rect, CornerRadius { nw: 0, sw: 0, ne: 7, se: 7 }, lift(hc));
    p.line_segment(
        [Pos2::new(caret_rect.left(), rect.top() + 5.0), Pos2::new(caret_rect.left(), rect.bottom() - 5.0)],
        Stroke::new(1.0, theme::accent().gamma_multiply(0.35)),
    );
    let col = theme::accent();
    plus(p, Pos2::new(main_rect.left() + 13.0, rect.center().y), 4.0, Stroke::new(1.6, col));
    p.text(Pos2::new(main_rect.left() + 22.0, rect.center().y), Align2::LEFT_CENTER, "New", FontId::proportional(12.5), col);
    paint_glyph(p, caret_rect.center(), Glyph::Caret, col);
    (main, caret)
}

/// A 20px button layered on a row (shown while the row is hot).
pub fn mini_button(ui: &mut Ui, id: Id, c: Pos2, g: Glyph, alpha: f32, tip: &str) -> egui::Response {
    let rect = Rect::from_center_size(c, Vec2::splat(20.0));
    let resp = ui.interact(rect, id, Sense::click()).on_hover_text(tip);
    let bg = if resp.hovered() { theme::surface_3() } else { theme::surface_2() };
    ui.painter().rect_filled(rect, CornerRadius::same(5), bg.gamma_multiply(alpha));
    paint_glyph(ui.painter(), c, g, theme::text().gamma_multiply(alpha));
    resp
}

pub fn plus(p: &egui::Painter, c: Pos2, r: f32, s: Stroke) {
    p.line_segment([c - Vec2::new(r, 0.0), c + Vec2::new(r, 0.0)], s);
    p.line_segment([c - Vec2::new(0.0, r), c + Vec2::new(0.0, r)], s);
}

/// Chevron rotating from "▸" (0) to "▾" (1).
pub fn chevron(p: &egui::Painter, c: Pos2, openness: f32, col: Color32) {
    let a = openness * std::f32::consts::FRAC_PI_2;
    let rot = |v: Vec2| Vec2::new(v.x * a.cos() - v.y * a.sin(), v.x * a.sin() + v.y * a.cos());
    let pts = [Vec2::new(-1.8, -3.6), Vec2::new(2.2, 0.0), Vec2::new(-1.8, 3.6)].map(|v| c + rot(v));
    p.add(egui::Shape::line(pts.to_vec(), Stroke::new(1.5, col)));
}

/// Folder outline; the flap lifts as it opens.
pub fn folder_icon(p: &egui::Painter, r: Rect, openness: f32, col: Color32) {
    let at = |x: f32, y: f32| Pos2::new(r.left() + x * r.width(), r.top() + y * r.height());
    let s = Stroke::new(1.4, col);
    p.rect_filled(Rect::from_min_max(at(0.0, 0.25), at(1.0, 1.0)), CornerRadius::same(2), col.gamma_multiply(0.16));
    p.add(egui::Shape::line(vec![at(0.0, 0.95), at(0.0, 0.05), at(0.38, 0.05), at(0.5, 0.22), at(1.0, 0.22), at(1.0, 0.95), at(0.0, 0.95)], s));
    if openness > 0.0 {
        let lift = 0.25 * openness;
        p.add(egui::Shape::line(
            vec![at(0.0, 0.95), at(0.12 + lift * 0.2, 0.45 - lift * 0.3), at(1.0 + lift * 0.1, 0.45 - lift * 0.3), at(1.0, 0.95)],
            Stroke::new(1.4, col.gamma_multiply(openness)),
        ));
    }
}

pub fn badge(p: &egui::Painter, right_center: Pos2, n: usize, alpha: f32) {
    let text = n.to_string();
    let galley = p.layout_no_wrap(text, FontId::proportional(10.5), theme::text_faint());
    let w = galley.size().x + 10.0;
    let rect = Rect::from_center_size(right_center - Vec2::new(w / 2.0 - 4.0, 0.0), Vec2::new(w, 16.0));
    p.rect_filled(rect, CornerRadius::same(8), theme::surface_3().gamma_multiply(0.7 * alpha));
    p.galley(rect.center() - galley.size() / 2.0, galley, theme::text_faint().gamma_multiply(alpha));
}

/// The tether mark: two linked rounded diamonds.
pub fn logo(ui: &mut Ui, size: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    let p = ui.painter();
    let c = rect.center();
    let r = size * 0.32;
    for (dx, color) in [(-size * 0.14, theme::accent()), (size * 0.14, theme::blue())] {
        let center = c + Vec2::new(dx, 0.0);
        let pts = vec![center + Vec2::new(0.0, -r), center + Vec2::new(r, 0.0), center + Vec2::new(0.0, r), center + Vec2::new(-r, 0.0)];
        p.add(egui::Shape::convex_polygon(pts, color.gamma_multiply(0.18), Stroke::new(size * 0.08, color)));
    }
}

/// "HOSTS · 12" row; with `openness` it is a collapsible header with a chevron. Always
/// clickable, so it can carry a context menu.
pub fn section_header(ui: &mut Ui, title: &str, count: usize, openness: Option<f32>) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 22.0), Sense::click());
    let p = ui.painter();
    let mut x = rect.left() + 2.0;
    if let Some(o) = openness {
        let hover = ui.ctx().animate_bool_with_time(resp.id.with("h"), resp.hovered(), theme::anim_time(0.12));
        chevron(p, Pos2::new(x + 5.0, rect.center().y), o, theme::text_faint().lerp_to_gamma(theme::text(), hover));
        x += 14.0;
    }
    let galley = p.layout_no_wrap(title.to_string(), FontId::proportional(10.5), theme::text_faint());
    let w = galley.size().x;
    p.galley(Pos2::new(x, rect.center().y - galley.size().y / 2.0), galley, theme::text_faint());
    if count > 0 {
        p.text(
            Pos2::new(x + w + 6.0, rect.center().y),
            Align2::LEFT_CENTER,
            count.to_string(),
            FontId::proportional(10.5),
            theme::text_faint().gamma_multiply(0.7),
        );
    }
    resp
}
