//! Settings → Layout: where tabs and the sidebar go, sidebar sections, pane behaviour.
//! A schematic of the window at the top follows every change as it is made (and the
//! app behind the dimmed settings screen rearranges live too).

use eframe::egui::{self, Align2, Color32, CornerRadius, FontId, Pos2, Rect, Sense, Stroke, Ui, Vec2};

use super::settings_widgets::{self as w, Icon, SliderSpec, paint_icon};
use super::sidebar_sections::section_icon;
use crate::layout::{LayoutSettings, Side, SidebarSection, TabPosition};
use crate::settings::Settings;
use crate::theme;

const PREVIEW_H: f32 = 190.0;
const SECTION_ROW_H: f32 = 38.0;

pub fn show(ui: &mut Ui, s: &mut Settings, gap: f32) {
    let l = &mut s.layout;
    preview(ui, l);
    ui.add_space(gap);
    w::card(ui, Some("Tabs"), |ui| {
        w::row(ui, "Tab position", "A tab strip on each pane, or a vertical list of sessions beside them", |ui| {
            w::segmented(ui, &mut l.tab_position, &[(TabPosition::Top, "Top"), (TabPosition::Left, "Left"), (TabPosition::Right, "Right")]);
        });
        ui.add_enabled_ui(l.tab_position.is_vertical(), |ui| {
            w::separator(ui);
            w::row(ui, "Group by pane", "Vertical tabs: a header for each split pane", |ui| {
                w::toggle(ui, &mut l.group_by_pane);
            });
            w::separator(ui);
            w::row(ui, "Compact rows", "Vertical tabs: one line per tab, no folder or address", |ui| {
                w::toggle(ui, &mut l.compact_tabs);
            });
        });
    });
    ui.add_space(gap);
    w::card(ui, Some("Sidebar"), |ui| {
        w::row(ui, "Show sidebar", &crate::keybindings::hint("Hosts, workspaces and more", crate::keybindings::Action::ToggleSidebar), |ui| {
            w::toggle(ui, &mut l.sidebar_visible);
        });
        w::separator(ui);
        w::row(ui, "Side", "Dock the sidebar on the left or the right of the window", |ui| {
            w::segmented(ui, &mut l.sidebar_side, &[(Side::Left, "Left"), (Side::Right, "Right")]);
        });
        w::separator(ui);
        w::row(ui, "Activity bar", "An icon strip that shows one section at a time", |ui| {
            w::toggle(ui, &mut l.activity_bar);
        });
        w::separator(ui);
        ui.label(egui::RichText::new("Sections").size(13.5).color(theme::text()));
        ui.label(egui::RichText::new("Order from top to bottom; collapsed sections show just their header").size(11.5).color(theme::text_dim()));
        ui.add_space(6.0);
        sections_list(ui, l);
    });
    ui.add_space(gap);
    w::card(ui, Some("Panes"), |ui| {
        w::row(ui, "Dim inactive panes", "Fade terminals that don't have focus when the window is split", |ui| {
            w::toggle(ui, &mut l.dim_inactive_panes);
        });
        ui.add_enabled_ui(l.dim_inactive_panes, |ui| {
            w::separator(ui);
            w::row(ui, "Dim amount", "", |ui| {
                let spec = SliderSpec { min: 0.1, max: LayoutSettings::DIM_MAX, step: 0.05, log: false, format: &|v| format!("{:.0}%", v * 100.0) };
                let mut v = l.dim_amount();
                if w::slider_with_value(ui, &mut v, &spec).changed() {
                    l.dim_amount = v;
                }
            });
        });
        w::separator(ui);
        w::row(ui, "Focus follows mouse", "Hovering a pane gives it the keyboard", |ui| {
            w::toggle(ui, &mut l.focus_follows_mouse);
        });
        w::separator(ui);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            ui.label(egui::RichText::new("Shortcuts:").size(11.5).color(theme::text_dim()));
            for (what, action) in [
                ("split", crate::keybindings::Action::SplitRight),
                ("split down", crate::keybindings::Action::SplitDown),
                ("maximize", crate::keybindings::Action::ZoomPane),
                ("focus left", crate::keybindings::Action::FocusPaneLeft),
            ] {
                if let Some(b) = crate::keybindings::binding(action) {
                    for cap in b.caps() {
                        w::key_chip(ui, &cap);
                    }
                    ui.label(egui::RichText::new(what).size(11.5).color(theme::text_faint()));
                    ui.add_space(8.0);
                }
            }
        });
    });
}

/// Rows for each section with ↑ / ↓ and a collapsed switch; drag a row to reorder.
fn sections_list(ui: &mut Ui, l: &mut LayoutSettings) {
    let list = l.sections();
    let mut moved = None;
    let mut collapse = None;
    let mut centres = Vec::with_capacity(list.len());
    let top = ui.cursor().top();
    for (i, e) in list.iter().enumerate() {
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), SECTION_ROW_H), Sense::click_and_drag());
        centres.push(rect.center().y);
        let id = resp.id;
        if resp.drag_started() {
            resp.dnd_set_drag_payload(SettingsSectionDrag(i));
        }
        let hidden_here = e.kind == SidebarSection::Sessions && !l.sessions_in_sidebar();
        let p = ui.painter();
        let h = ui.ctx().animate_bool_with_time(id, resp.hovered(), theme::anim_time(0.1));
        p.rect_filled(rect.shrink2(Vec2::new(0.0, 2.0)), CornerRadius::same(8), theme::surface_3().gamma_multiply(0.45 + 0.4 * h));
        // Grip.
        for dy in [-4.0, 0.0, 4.0] {
            for dx in [0.0, 4.0] {
                p.circle_filled(Pos2::new(rect.left() + 12.0 + dx, rect.center().y + dy), 1.2, theme::text_faint());
            }
        }
        if resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        }
        paint_icon(p, Rect::from_center_size(Pos2::new(rect.left() + 36.0, rect.center().y), Vec2::splat(15.0)), section_icon(e.kind), theme::text_dim());
        p.text(Pos2::new(rect.left() + 54.0, rect.center().y), Align2::LEFT_CENTER, e.kind.title(), FontId::proportional(13.0), theme::text());
        if hidden_here {
            p.text(
                Pos2::new(rect.left() + 130.0, rect.center().y),
                Align2::LEFT_CENTER,
                "in its own panel while tabs are vertical",
                FontId::proportional(11.0),
                theme::text_faint(),
            );
        }
        let mut controls = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink2(Vec2::new(8.0, 0.0))).layout(egui::Layout::right_to_left(egui::Align::Center)));
        controls.spacing_mut().item_spacing.x = 4.0;
        let mut expanded = !e.collapsed;
        if w::toggle(&mut controls, &mut expanded).on_hover_text("Expanded").changed() {
            collapse = Some((e.kind, !expanded));
        }
        controls.add_space(8.0);
        if controls.add_enabled(i + 1 < list.len(), egui::Button::new("↓").small()).on_hover_text("Move down").clicked() {
            moved = Some((i, i + 2));
        }
        if controls.add_enabled(i > 0, egui::Button::new("↑").small()).on_hover_text("Move up").clicked() {
            moved = Some((i, i - 1));
        }
    }
    // Drag to reorder.
    if let Some(drag) = egui::DragAndDrop::payload::<SettingsSectionDrag>(ui.ctx()).map(|d| d.0)
        && let Some(pos) = ui.ctx().pointer_latest_pos()
    {
        let to = crate::layout::insertion_index(&centres, pos.y);
        let y = if to < centres.len() { centres[to] - SECTION_ROW_H / 2.0 } else { top + centres.len() as f32 * SECTION_ROW_H };
        let x = ui.min_rect().x_range();
        ui.painter().hline(x, y, Stroke::new(2.0, theme::accent()));
        if ui.input(|i| i.pointer.any_released()) {
            moved = Some((drag, to));
        }
    }
    if let Some((from, to)) = moved {
        l.move_section(from, to);
    }
    if let Some((kind, c)) = collapse {
        l.set_collapsed(kind, c);
    }
}

#[derive(Clone, Copy)]
struct SettingsSectionDrag(usize);

/// A small drawing of the window as the settings arrange it.
fn preview(ui: &mut Ui, l: &LayoutSettings) {
    let (outer, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), PREVIEW_H), Sense::hover());
    let p = ui.painter();
    p.rect(outer, CornerRadius::same(12), theme::bg(), Stroke::new(1.0, theme::border().gamma_multiply(0.6)), egui::StrokeKind::Inside);
    let win = Rect::from_center_size(outer.center(), Vec2::new((outer.width() - 40.0).min(520.0), PREVIEW_H - 36.0));
    p.rect_filled(win.translate(Vec2::new(0.0, 4.0)), CornerRadius::same(8), theme::shadow().gamma_multiply(0.4));
    p.rect(win, CornerRadius::same(8), theme::surface(), Stroke::new(1.0, theme::border()), egui::StrokeKind::Inside);
    let ctx = ui.ctx().clone();
    let anim = |name: &str, v: f32| ctx.animate_value_with_time(egui::Id::new(("layout_preview", name)), v, theme::anim_time(0.22));

    // Title bar.
    let bar = Rect::from_min_size(win.min, Vec2::new(win.width(), 16.0));
    p.rect_filled(bar, CornerRadius { nw: 8, ne: 8, sw: 0, se: 0 }, theme::surface_2());
    for (k, c) in [theme::danger(), theme::warn(), theme::accent()].into_iter().enumerate() {
        p.circle_filled(Pos2::new(bar.left() + 10.0 + k as f32 * 9.0, bar.center().y), 2.6, c.gamma_multiply(0.8));
    }
    let mut body = Rect::from_min_max(Pos2::new(win.left() + 4.0, bar.bottom() + 4.0), win.max - Vec2::splat(4.0));

    let right_side = l.sidebar_side == Side::Right;
    // Activity bar.
    let act_w = anim("act", if l.activity_bar { 16.0 } else { 0.0 });
    if act_w > 0.5 {
        let r = take(&mut body, right_side, act_w);
        p.rect_filled(r, CornerRadius::same(4), theme::surface_2().gamma_multiply(0.7));
        for k in 0..4 {
            let c = Pos2::new(r.center().x, r.top() + 10.0 + k as f32 * 14.0);
            let col = if k == 0 && l.sidebar_visible { theme::accent() } else { theme::text_faint() };
            p.rect_filled(Rect::from_center_size(c, Vec2::splat(7.0)), CornerRadius::same(2), col.gamma_multiply(act_w / 16.0));
        }
        take(&mut body, right_side, 3.0);
    }
    // Sidebar.
    let side_w = anim("side", if l.sidebar_visible { body.width() * 0.24 } else { 0.0 });
    if side_w > 0.5 {
        let r = take(&mut body, right_side, side_w);
        p.rect_filled(r, CornerRadius::same(4), theme::surface_2());
        let shown: Vec<SidebarSection> = if l.activity_bar {
            vec![l.active_view]
        } else {
            l.sections().iter().filter(|e| e.kind != SidebarSection::Sessions || l.sessions_in_sidebar()).map(|e| e.kind).collect()
        };
        let mut y = r.top() + 5.0;
        let row_h = ((r.height() - 10.0) / shown.len().max(1) as f32).min(40.0);
        for kind in shown {
            let collapsed = !l.activity_bar && l.is_collapsed(kind);
            p.text(Pos2::new(r.left() + 5.0, y + 4.0), Align2::LEFT_CENTER, kind.title(), FontId::proportional(7.5), theme::text_faint());
            if !collapsed {
                for k in 0..2 {
                    let line = Rect::from_min_size(Pos2::new(r.left() + 5.0, y + 10.0 + k as f32 * 7.0), Vec2::new((r.width() - 10.0) * (0.85 - 0.25 * k as f32), 3.5));
                    p.rect_filled(line, CornerRadius::same(2), theme::text_faint().gamma_multiply(0.35));
                }
            }
            y += if collapsed { 12.0 } else { row_h };
        }
        take(&mut body, right_side, 3.0);
    }
    // Vertical tabs panel.
    let vertical_right = l.tab_position == TabPosition::Right;
    let tabs_w = anim("tabs", if l.tab_position.is_vertical() { body.width() * 0.22 } else { 0.0 });
    if tabs_w > 0.5 {
        let r = take(&mut body, vertical_right, tabs_w);
        p.rect_filled(r, CornerRadius::same(4), theme::surface_2().gamma_multiply(0.8));
        let row_h = if l.compact_tabs { 9.0 } else { 14.0 };
        let mut y = r.top() + 5.0;
        for k in 0..4 {
            if l.group_by_pane && (k == 0 || k == 2) {
                p.text(Pos2::new(r.left() + 5.0, y + 3.0), Align2::LEFT_CENTER, format!("PANE {}", k / 2 + 1), FontId::proportional(6.5), theme::text_faint());
                y += 8.0;
            }
            let row = Rect::from_min_size(Pos2::new(r.left() + 3.0, y), Vec2::new(r.width() - 6.0, row_h - 2.0));
            if k == 0 {
                p.rect_filled(row, CornerRadius::same(3), theme::surface_3());
                p.rect_filled(Rect::from_min_size(row.min, Vec2::new(2.0, row.height())), CornerRadius::same(1), theme::accent());
            }
            p.rect_filled(Rect::from_center_size(Pos2::new(row.left() + 7.0, row.center().y), Vec2::splat(5.0)), CornerRadius::same(1), theme::hue_color(60.0 * k as f32 + 140.0));
            p.rect_filled(Rect::from_min_size(Pos2::new(row.left() + 13.0, row.center().y - 1.5), Vec2::new(row.width() * 0.5, 3.0)), CornerRadius::same(1), theme::text_faint().gamma_multiply(0.6));
            y += row_h;
        }
        take(&mut body, vertical_right, 3.0);
    }
    // Two panes, the left focused.
    let panes = [Rect::from_min_max(body.min, Pos2::new(body.center().x - 1.5, body.bottom())), Rect::from_min_max(Pos2::new(body.center().x + 1.5, body.top()), body.max)];
    let dim = anim("dim", if l.dim_inactive_panes { l.dim_amount() } else { 0.0 });
    for (k, pane) in panes.into_iter().enumerate() {
        let mut content = pane;
        if !l.tab_position.is_vertical() {
            let strip = Rect::from_min_size(pane.min, Vec2::new(pane.width(), 11.0));
            let tab = Rect::from_min_size(strip.min + Vec2::new(3.0, 2.0), Vec2::new(34.0, 8.0));
            p.rect_filled(tab, CornerRadius::same(2), theme::surface_3());
            p.rect_filled(tab.translate(Vec2::new(37.0, 0.0)), CornerRadius::same(2), theme::surface_2());
            content = Rect::from_min_max(Pos2::new(pane.left(), strip.bottom() + 2.0), pane.max);
        }
        let term_bg = theme::palette().term_bg;
        p.rect_filled(content, CornerRadius::same(4), term_bg);
        for line in 0..4 {
            let y = content.top() + 8.0 + line as f32 * 8.0;
            if y + 3.0 > content.bottom() {
                break;
            }
            let wfrac = [0.5, 0.75, 0.35, 0.6][line];
            let col = if line == 0 { theme::accent() } else { theme::text_dim() };
            p.rect_filled(Rect::from_min_size(Pos2::new(content.left() + 6.0, y), Vec2::new(content.width() * wfrac * 0.8, 3.0)), CornerRadius::same(1), col.gamma_multiply(0.6));
        }
        if k == 0 {
            p.rect_stroke(content, CornerRadius::same(4), Stroke::new(1.0, theme::accent().gamma_multiply(0.7)), egui::StrokeKind::Inside);
        } else if dim > 0.0 {
            p.rect_filled(content, CornerRadius::same(4), theme::bg().gamma_multiply(dim));
        }
    }
    if l.focus_follows_mouse {
        // A pointer resting on the right pane.
        let tip = panes[1].center();
        let pts = vec![tip, tip + Vec2::new(0.0, 11.0), tip + Vec2::new(3.0, 8.0), tip + Vec2::new(7.5, 8.0)];
        p.add(egui::Shape::convex_polygon(pts, Color32::WHITE, Stroke::new(1.0, Color32::BLACK)));
    }
    paint_icon(p, Rect::from_min_size(outer.left_top() + Vec2::new(12.0, 10.0), Vec2::splat(12.0)), Icon::Layout, theme::text_faint());
    p.text(outer.left_top() + Vec2::new(30.0, 16.0), Align2::LEFT_CENTER, "Preview", FontId::proportional(11.0), theme::text_faint());
}

/// Cuts `w` points off the left (or right) of `r` and returns that strip.
fn take(r: &mut Rect, right: bool, w: f32) -> Rect {
    let w = w.min(r.width());
    if right {
        let strip = Rect::from_min_max(Pos2::new(r.right() - w, r.top()), r.max);
        r.max.x -= w;
        strip
    } else {
        let strip = Rect::from_min_max(r.min, Pos2::new(r.left() + w, r.bottom()));
        r.min.x += w;
        strip
    }
}
