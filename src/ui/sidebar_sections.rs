//! The sidebar's frame: collapsible, drag-to-reorder sections stacked top to bottom
//! (or, with the activity bar, one section at a time), and the activity bar itself.
//! What each section contains comes from a [`SectionHost`].

use eframe::egui::{self, Align2, CornerRadius, FontId, Id, Pos2, Rect, Sense, Stroke, Ui, UiBuilder, Vec2};

use super::settings_widgets::{Icon, paint_icon};
use crate::layout::{self, SectionEntry, SidebarSection};
use crate::theme;

const HEADER_H: f32 = 30.0;
const GAP: f32 = 4.0;
/// A flexible section never gets less than this.
const FLEX_MIN: f32 = 96.0;
pub const ACTIVITY_W: f32 = 44.0;
const ACTIVITY_ITEM: f32 = 36.0;

/// How tall a section's body wants to be.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BodySize {
    Fixed(f32),
    /// Shares whatever height is left.
    Flex,
}

pub trait SectionHost {
    /// Body height of `kind` given the sidebar's total height.
    fn size(&self, kind: SidebarSection, avail: f32) -> BodySize;
    /// Shown faintly after the title.
    fn count(&self, kind: SidebarSection) -> Option<usize>;
    /// Buttons on the right of the header (laid out right to left).
    fn header_extra(&mut self, _kind: SidebarSection, _ui: &mut Ui) {}
    fn body(&mut self, kind: SidebarSection, ui: &mut Ui);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SectionEvent {
    SetCollapsed(SidebarSection, bool),
    /// Move the section at `from` before the one at `to` (see `layout::move_item`).
    Move { from: usize, to: usize },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SectionDrag(usize);

/// Heights of each section's body (0 for collapsed ones), sharing the space left by
/// headers and fixed bodies among the flexible ones.
pub fn plan_heights(sizes: &[(BodySize, bool)], avail: f32) -> Vec<f32> {
    let chrome = sizes.len() as f32 * (HEADER_H + GAP);
    let fixed: f32 = sizes.iter().map(|(s, collapsed)| if *collapsed { 0.0 } else if let BodySize::Fixed(h) = s { *h } else { 0.0 }).sum();
    let flex_n = sizes.iter().filter(|(s, c)| !c && *s == BodySize::Flex).count();
    let flex_h = if flex_n == 0 { 0.0 } else { ((avail - chrome - fixed) / flex_n as f32).max(FLEX_MIN) };
    sizes
        .iter()
        .map(|(s, collapsed)| match (collapsed, s) {
            (true, _) => 0.0,
            (false, BodySize::Fixed(h)) => *h,
            (false, BodySize::Flex) => flex_h,
        })
        .collect()
}

/// Every section stacked; headers collapse their section on click and reorder on drag.
pub fn stacked(ui: &mut Ui, entries: &[SectionEntry], host: &mut dyn SectionHost) -> Vec<SectionEvent> {
    let mut events = Vec::new();
    let avail = ui.available_height();
    let sizes: Vec<(BodySize, bool)> = entries.iter().map(|e| (host.size(e.kind, avail), e.collapsed)).collect();
    let heights = plan_heights(&sizes, avail);
    let mut headers: Vec<Rect> = Vec::with_capacity(entries.len());
    let area = ui.max_rect();
    ui.spacing_mut().item_spacing.y = 0.0;
    for (i, e) in entries.iter().enumerate() {
        let id = Id::new(("sidebar_section", e.kind));
        let open = ui.ctx().animate_bool_with_time(id.with("open"), !e.collapsed, theme::anim_time(0.18));
        let header = section_header(ui, id, e.kind, host.count(e.kind), Some(open), i, entries.len(), &mut events);
        headers.push(header.rect);
        if header.drag_started() {
            header.dnd_set_drag_payload(SectionDrag(i));
        }
        let mut extra = ui.new_child(UiBuilder::new().max_rect(header.rect.shrink2(Vec2::new(4.0, 0.0))).layout(egui::Layout::right_to_left(egui::Align::Center)));
        if !e.collapsed {
            host.header_extra(e.kind, &mut extra);
        }
        // Use the animated height for the body; the full layout is used by the next frame.
        let target = if e.collapsed { open_height(&sizes, i, avail) } else { heights[i] };
        let h = (target * open).floor();
        if h >= 1.0 {
            let rect = Rect::from_min_size(ui.cursor().min, Vec2::new(ui.available_width(), h));
            let mut child = ui.new_child(UiBuilder::new().max_rect(rect).id_salt(("section_body", e.kind)));
            child.set_clip_rect(rect.intersect(ui.clip_rect()));
            if open < 1.0 {
                child.multiply_opacity(open);
            }
            host.body(e.kind, &mut child);
            ui.advance_cursor_after_rect(rect);
        }
        ui.add_space(GAP);
    }
    reorder_drop(ui, area, &headers, &mut events);
    events
}

/// The height section `i` would get if it were open (its collapse animation runs on it).
fn open_height(sizes: &[(BodySize, bool)], i: usize, avail: f32) -> f32 {
    let mut opened = sizes.to_vec();
    opened[i].1 = false;
    plan_heights(&opened, avail)[i]
}

/// One section filling the sidebar (activity bar mode): a title header, then its body.
pub fn single(ui: &mut Ui, kind: SidebarSection, host: &mut dyn SectionHost) {
    let id = Id::new(("sidebar_single", kind));
    let mut events = Vec::new();
    let header = section_header(ui, id, kind, host.count(kind), None, 0, 1, &mut events);
    let mut extra = ui.new_child(UiBuilder::new().max_rect(header.rect.shrink2(Vec2::new(4.0, 0.0))).layout(egui::Layout::right_to_left(egui::Align::Center)));
    host.header_extra(kind, &mut extra);
    let rect = Rect::from_min_max(ui.cursor().min, ui.max_rect().max);
    let mut child = ui.new_child(UiBuilder::new().max_rect(rect).id_salt(("section_body", kind)));
    host.body(kind, &mut child);
    ui.advance_cursor_after_rect(rect);
}

#[allow(clippy::too_many_arguments)]
fn section_header(ui: &mut Ui, id: Id, kind: SidebarSection, count: Option<usize>, openness: Option<f32>, index: usize, len: usize, events: &mut Vec<SectionEvent>) -> egui::Response {
    let sense = if openness.is_some() { Sense::click_and_drag() } else { Sense::hover() };
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), HEADER_H), Sense::hover());
    // A stable id (not the position), so a drag survives the layout changing under it.
    let resp = ui.interact(rect, id.with("header"), sense);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::CollapsingHeader, true, kind.title()));
    let ctx = ui.ctx().clone();
    let hover = ctx.animate_bool_with_time(id.with("hover"), ui.rect_contains_pointer(rect) && openness.is_some(), theme::anim_time(0.12));
    let p = ui.painter();
    if hover > 0.0 {
        p.rect_filled(rect.shrink2(Vec2::new(0.0, 2.0)), CornerRadius::same(theme::RADIUS_SM), theme::surface_2().gamma_multiply(0.6 * hover));
    }
    let mut x = rect.left() + 6.0;
    let col = theme::text_faint().lerp_to_gamma(theme::text_dim(), hover);
    if let Some(o) = openness {
        chevron(p, Pos2::new(x + 4.0, rect.center().y), o, col);
        x += 14.0;
    }
    let title = kind.title().to_uppercase();
    let galley = p.layout_no_wrap(title, FontId::proportional(10.5), col);
    let w = galley.size().x;
    p.galley(Pos2::new(x, rect.center().y - galley.size().y / 2.0), galley, col);
    if let Some(n) = count.filter(|n| *n > 0) {
        p.text(Pos2::new(x + w + 6.0, rect.center().y), Align2::LEFT_CENTER, n.to_string(), FontId::proportional(10.5), theme::text_faint().gamma_multiply(0.7));
    }
    if openness.is_some() {
        if resp.hovered() && !resp.dragged() {
            ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        let collapsed = openness.is_some_and(|o| o < 0.5);
        if resp.clicked() {
            events.push(SectionEvent::SetCollapsed(kind, !collapsed));
        }
        resp.context_menu(|ui| {
            if ui.button(if collapsed { "Expand" } else { "Collapse" }).clicked() {
                events.push(SectionEvent::SetCollapsed(kind, !collapsed));
                ui.close();
            }
            if ui.add_enabled(index > 0, egui::Button::new("Move up")).clicked() {
                events.push(SectionEvent::Move { from: index, to: index - 1 });
                ui.close();
            }
            if ui.add_enabled(index + 1 < len, egui::Button::new("Move down")).clicked() {
                events.push(SectionEvent::Move { from: index, to: index + 2 });
                ui.close();
            }
            ui.separator();
            ui.label(egui::RichText::new("Drag a header to reorder").size(11.0).color(theme::text_faint()));
        });
    }
    resp
}

/// Insertion line while a header is dragged; the move on release.
fn reorder_drop(ui: &Ui, area: Rect, headers: &[Rect], events: &mut Vec<SectionEvent>) {
    let Some(drag) = egui::DragAndDrop::payload::<SectionDrag>(ui.ctx()).map(|d| d.0) else { return };
    let Some(pos) = ui.ctx().pointer_latest_pos().filter(|p| area.expand(24.0).contains(*p)) else { return };
    let centres: Vec<f32> = headers.iter().map(|r| r.center().y).collect();
    let to = layout::insertion_index(&centres, pos.y);
    let y = match headers.get(to) {
        Some(r) => r.top() - GAP / 2.0,
        None => headers.last().map_or(area.top(), |_| ui.min_rect().bottom()),
    };
    let p = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Foreground, Id::new("section_reorder")));
    if to != drag && to != drag + 1 {
        p.hline(area.x_range().shrink(4.0), y, Stroke::new(2.0, theme::accent()));
        p.circle_filled(Pos2::new(area.left() + 4.0, y), 3.0, theme::accent());
    }
    if let Some(r) = headers.get(drag) {
        p.rect_stroke(*r, CornerRadius::same(theme::RADIUS_SM), Stroke::new(1.0, theme::accent().gamma_multiply(0.6)), egui::StrokeKind::Inside);
    }
    if ui.input(|i| i.pointer.any_released()) {
        events.push(SectionEvent::Move { from: drag, to });
    }
}

fn chevron(p: &egui::Painter, c: Pos2, openness: f32, col: egui::Color32) {
    let a = openness * std::f32::consts::FRAC_PI_2;
    let rot = |v: Vec2| Vec2::new(v.x * a.cos() - v.y * a.sin(), v.x * a.sin() + v.y * a.cos());
    let pts = [Vec2::new(-1.8, -3.6), Vec2::new(2.2, 0.0), Vec2::new(-1.8, 3.6)].map(|v| c + rot(v));
    p.add(egui::Shape::line(pts.to_vec(), Stroke::new(1.5, col)));
}

pub fn section_icon(kind: SidebarSection) -> Icon {
    match kind {
        SidebarSection::Hosts => Icon::Server,
        SidebarSection::Sessions => Icon::Rows,
        SidebarSection::Workspaces => Icon::Layout,
        SidebarSection::Agents => Icon::Sparkle,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivityEvent {
    /// Show this section (opens the sidebar if hidden).
    Show(SidebarSection),
    /// The active section was clicked again: hide / show the sidebar.
    ToggleSidebar,
    OpenSettings,
    OpenPalette,
}

/// The icon strip. `active` is highlighted while the sidebar is open.
pub fn activity_bar(ui: &mut Ui, entries: &[SectionEntry], active: SidebarSection, open: bool, badge: &dyn Fn(SidebarSection) -> Option<usize>, on_right: bool) -> Option<ActivityEvent> {
    let mut out = None;
    let rect = ui.max_rect();
    let mut y = rect.top() + 8.0;
    for e in entries {
        let r = Rect::from_center_size(Pos2::new(rect.center().x, y + ACTIVITY_ITEM / 2.0), Vec2::splat(ACTIVITY_ITEM));
        let selected = open && e.kind == active;
        let tip = if selected { format!("{} (click to hide the sidebar)", e.kind.title()) } else { e.kind.title().to_string() };
        if activity_item(ui, r, section_icon(e.kind), selected, badge(e.kind), on_right, &tip, e.kind.title()).clicked() {
            out = Some(if selected { ActivityEvent::ToggleSidebar } else { ActivityEvent::Show(e.kind) });
        }
        y += ACTIVITY_ITEM + 4.0;
    }
    let bottom = |k: f32| Rect::from_center_size(Pos2::new(rect.center().x, rect.bottom() - 8.0 - ACTIVITY_ITEM * (k + 0.5) - 4.0 * k), Vec2::splat(ACTIVITY_ITEM));
    let gear_tip = crate::keybindings::hint("Settings", crate::keybindings::Action::OpenSettings);
    if activity_item(ui, bottom(0.0), Icon::Gear, false, None, on_right, &gear_tip, "Activity settings").clicked() {
        out = Some(ActivityEvent::OpenSettings);
    }
    let palette_tip = crate::keybindings::hint("Command palette", crate::keybindings::Action::CommandPalette);
    if activity_item(ui, bottom(1.0), Icon::Search, false, None, on_right, &palette_tip, "Activity palette").clicked() {
        out = Some(ActivityEvent::OpenPalette);
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn activity_item(ui: &mut Ui, r: Rect, icon: Icon, selected: bool, badge: Option<usize>, on_right: bool, tip: &str, label: &str) -> egui::Response {
    let resp = ui.interact(r, Id::new(("activity", label)), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
    let ctx = ui.ctx().clone();
    let h = ctx.animate_bool_with_time(resp.id.with("h"), resp.hovered(), theme::anim_time(0.1));
    let s = ctx.animate_bool_with_time(resp.id.with("s"), selected, theme::anim_time(0.16));
    if resp.hovered() {
        ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let p = ui.painter();
    let tile = r.shrink(2.0);
    if h > 0.0 || s > 0.0 {
        p.rect_filled(tile, CornerRadius::same(8), theme::surface_2().gamma_multiply((0.7 * h).max(s)));
    }
    if s > 0.0 {
        let x = if on_right { r.right() + 4.0 } else { r.left() - 4.0 };
        let bar = Rect::from_center_size(Pos2::new(x, r.center().y), Vec2::new(3.0, 18.0 * s));
        p.rect_filled(bar, CornerRadius::same(2), theme::accent());
    }
    let col = if selected { theme::text() } else { theme::text_faint().lerp_to_gamma(theme::text(), h) };
    paint_icon(p, Rect::from_center_size(r.center(), Vec2::splat(17.0)), icon, col);
    if let Some(n) = badge.filter(|n| *n > 0) {
        let c = Pos2::new(r.right() - 9.0, r.top() + 9.0);
        let text = if n > 99 { "99+".to_string() } else { n.to_string() };
        let galley = p.layout_no_wrap(text, FontId::proportional(9.0), theme::on_accent());
        let pill = Rect::from_center_size(c, Vec2::new((galley.size().x + 7.0).max(14.0), 14.0));
        p.rect_filled(pill, CornerRadius::same(7), theme::accent());
        p.galley(pill.center() - galley.size() / 2.0, galley, theme::on_accent());
    }
    resp.on_hover_text(tip)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flex_sections_share_what_fixed_ones_leave() {
        let sizes = [(BodySize::Flex, false), (BodySize::Fixed(100.0), false), (BodySize::Fixed(50.0), true)];
        let h = plan_heights(&sizes, 600.0);
        assert_eq!(h[1], 100.0);
        assert_eq!(h[2], 0.0, "collapsed");
        assert_eq!(h[0], 600.0 - 3.0 * (HEADER_H + GAP) - 100.0);
        let two = plan_heights(&[(BodySize::Flex, false), (BodySize::Flex, false)], 400.0);
        assert_eq!(two[0], two[1]);
        let cramped = plan_heights(&[(BodySize::Flex, false), (BodySize::Fixed(900.0), false)], 400.0);
        assert_eq!(cramped[0], FLEX_MIN);
    }
}
