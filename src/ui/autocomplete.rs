//! As-you-type suggestions for single-line text fields: a dim inline "ghost" after the
//! caret plus a dropdown. Tab / Right accept, Up / Down pick, Esc closes. Also hosts the
//! popup and ghost painting shared with the SFTP path field.

use eframe::egui::{self, Align2, CornerRadius, FontId, Id, Key, Modifiers, Rect, Sense, Stroke, Ui, Vec2};

use super::suggest;
use crate::theme;

const POPUP_MAX_H: f32 = 240.0;
const ROW_H: f32 = 24.0;

/// Whether the pointer is inside `rect`, on any layer. `Ui::rect_contains_pointer` is
/// layer-aware and reports `false` over a popup `Area`, which would close it mid-click.
pub fn pointer_in(ui: &Ui, rect: Rect) -> bool {
    ui.input(|i| i.pointer.hover_pos().or(i.pointer.interact_pos()).is_some_and(|p| rect.contains(p)))
}

/// Caret position (in chars) when nothing is selected.
fn caret(range: Option<egui::text::CCursorRange>) -> Option<usize> {
    range.filter(|r| r.primary == r.secondary).map(|r| r.primary.index.0)
}

/// Whether the caret of the text field `id` sits at the end of `text` with no selection.
pub fn caret_at_end(ctx: &egui::Context, id: Id, text: &str) -> bool {
    let range = egui::text_edit::TextEditState::load(ctx, id).and_then(|s| s.cursor.char_range());
    caret(range) == Some(text.chars().count())
}

/// Moves the caret of the text field `id` to the end of `text`.
pub fn move_caret_to_end(ctx: &egui::Context, id: Id, text: &str) {
    let mut state = egui::text_edit::TextEditState::load(ctx, id).unwrap_or_default();
    let end = egui::text::CCursor::new(text.chars().count());
    state.cursor.set_char_range(Some(egui::text::CCursorRange::one(end)));
    state.store(ctx, id);
}

/// Paints `ghost` right after the text of a shown field, in the field's own font.
pub fn paint_ghost(ui: &Ui, out: &egui::text_edit::TextEditOutput, text: &str, ghost: &str) {
    if ghost.is_empty() || caret(out.cursor_range) != Some(text.chars().count()) {
        return;
    }
    let font = out.galley.job.sections.first().map(|s| s.format.font_id.clone()).unwrap_or_else(|| egui::TextStyle::Body.resolve(ui.style()));
    let pos = out.galley_pos + Vec2::new(out.galley.rect.right(), 0.0);
    let painter = ui.painter().with_clip_rect(out.text_clip_rect.intersect(ui.clip_rect()));
    painter.text(pos, Align2::LEFT_TOP, ghost, font, theme::text_faint());
}

/// Dropdown under `anchor`; returns its rect.
pub fn popup(ui: &Ui, id: Id, anchor: Rect, fade: f32, order: egui::Order, body: impl FnOnce(&mut Ui)) -> Rect {
    egui::Area::new(id)
        .order(order)
        .fixed_pos(anchor.left_bottom() + Vec2::new(0.0, 4.0 - 4.0 * (1.0 - fade)))
        .show(ui.ctx(), |ui| {
            ui.set_opacity(fade);
            egui::Frame::new()
                .fill(theme::surface_2())
                .stroke(Stroke::new(1.0, theme::border()))
                .corner_radius(theme::RADIUS)
                .inner_margin(6.0)
                .shadow(egui::Shadow { offset: [0, 4], blur: 14, spread: 0, color: egui::Color32::from_black_alpha(90) })
                .show(ui, |ui| {
                    ui.set_width((anchor.width() - 12.0).max(120.0));
                    egui::ScrollArea::vertical().max_height(POPUP_MAX_H).show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 1.0;
                        body(ui);
                    });
                });
        })
        .response
        .rect
}

/// One dropdown row (optional icon, then text in `font`). Returns whether it was clicked.
pub fn popup_row(ui: &mut Ui, icon: &str, text: &str, color: egui::Color32, selected: bool, font: FontId) -> bool {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_H), Sense::click());
    let hover = ui.ctx().animate_bool_with_time(resp.id, resp.hovered() || selected, 0.1);
    let p = ui.painter();
    if hover > 0.0 {
        p.rect_filled(rect, CornerRadius::same(theme::RADIUS_SM), theme::surface_3().gamma_multiply(hover));
    }
    let text_x = if icon.is_empty() { 8.0 } else { 28.0 };
    if !icon.is_empty() {
        p.text(rect.left_center() + Vec2::new(8.0, 0.0), Align2::LEFT_CENTER, icon, FontId::proportional(12.0), theme::text_dim());
    }
    let clip = p.with_clip_rect(rect.shrink2(Vec2::new(4.0, 0.0)));
    clip.text(rect.left_center() + Vec2::new(text_x, 0.0), Align2::LEFT_CENTER, text, font, color);
    if selected {
        resp.scroll_to_me(None);
    }
    resp.clicked()
}

/// Index after pressing Down (`down`) or Up in a list of `len`; Up from the top deselects.
pub fn step(sel: Option<usize>, len: usize, down: bool) -> Option<usize> {
    if len == 0 {
        return None;
    }
    match (sel, down) {
        (None, true) => Some(0),
        (None, false) => None,
        (Some(i), true) => Some((i + 1).min(len - 1)),
        (Some(i), false) => i.checked_sub(1),
    }
}

/// Suggestion state of one plain text field.
pub struct AutoField {
    id: Id,
    list: Vec<String>,
    computed_for: Option<String>,
    sel: Option<usize>,
    open: bool,
    popup_rect: Option<Rect>,
}

impl AutoField {
    pub fn new(id: Id) -> Self {
        Self { id, list: Vec::new(), computed_for: None, sel: None, open: false, popup_rect: None }
    }

    /// True while the suggestion list is showing (Enter belongs to it then).
    pub fn is_open(&self) -> bool {
        self.open && !self.list.is_empty()
    }

    fn ghost(&self, text: &str) -> Option<String> {
        self.list.get(self.sel.unwrap_or(0)).and_then(|s| suggest::word_ghost(text, s))
    }

    fn accept(&mut self, ctx: &egui::Context, text: &mut String, value: String) {
        *text = value;
        self.open = false;
        self.sel = None;
        move_caret_to_end(ctx, self.id, text);
    }

    /// Shows the field built by `edit` (its id is set here) with suggestions from `rank`.
    /// Enter on a highlighted row fills it in and still reaches the caller's Enter handling.
    pub fn show(
        &mut self,
        ui: &mut Ui,
        text: &mut String,
        edit: impl FnOnce(&mut String) -> egui::TextEdit<'_>,
        rank: impl FnOnce(&str) -> Vec<String>,
        order: egui::Order,
    ) -> egui::Response {
        let ctx = ui.ctx().clone();
        let focused = ctx.memory(|m| m.has_focus(self.id));
        if focused && self.open {
            self.keys(&ctx, text);
        }
        let out = edit(text).id(self.id).show(ui);
        let resp = out.response.response.clone();
        if resp.changed() {
            self.open = !text.is_empty();
            self.sel = None;
        }
        if self.computed_for.as_deref() != Some(text.as_str()) {
            self.list = rank(text);
            self.computed_for = Some(text.clone());
            self.sel = self.sel.filter(|&i| i < self.list.len());
        }
        let hovered = self.popup_rect.is_some_and(|r| pointer_in(ui, r));
        if !focused && !hovered {
            self.open = false;
        }
        let show_popup = self.open && (focused || hovered) && !self.list.is_empty();
        if show_popup && let Some(g) = self.ghost(text) {
            paint_ghost(ui, &out, text, &g);
        }
        let fade = ctx.animate_bool_with_time(self.id.with("fade"), show_popup, 0.12);
        self.popup_rect = None;
        if fade > 0.0 && !self.list.is_empty() {
            let mut picked = None;
            let font = FontId::proportional(13.0);
            let rect = popup(ui, self.id.with("popup"), resp.rect, fade, order, |ui| {
                for (i, s) in self.list.iter().enumerate() {
                    if popup_row(ui, "", s, theme::text(), self.sel == Some(i), font.clone()) {
                        picked = Some(s.clone());
                    }
                }
            });
            self.popup_rect = Some(rect);
            if let Some(value) = picked {
                self.accept(&ctx, text, value);
                ctx.memory_mut(|m| m.request_focus(self.id));
            }
        }
        resp
    }

    fn keys(&mut self, ctx: &egui::Context, text: &mut String) {
        let len = self.list.len();
        let at_end = caret_at_end(ctx, self.id, text);
        let has_ghost = self.ghost(text).is_some();
        let (down, up, esc, accept, enter) = ctx.input_mut(|i| {
            let down = len > 0 && i.consume_key(Modifiers::NONE, Key::ArrowDown);
            let up = len > 0 && i.consume_key(Modifiers::NONE, Key::ArrowUp);
            let esc = i.consume_key(Modifiers::NONE, Key::Escape);
            let can_accept = has_ghost || self.sel.is_some();
            let accept = can_accept && (i.consume_key(Modifiers::NONE, Key::Tab) || (at_end && i.consume_key(Modifiers::NONE, Key::ArrowRight)));
            let enter = self.sel.is_some() && i.key_pressed(Key::Enter);
            (down, up, esc, accept, enter)
        });
        if down || up {
            self.sel = step(self.sel, len, down);
        }
        if esc {
            self.open = false;
            return;
        }
        if (accept || enter)
            && let Some(value) = self.sel.and_then(|i| self.list.get(i)).cloned().or_else(|| self.ghost(text).map(|g| format!("{text}{g}")))
        {
            self.accept(ctx, text, value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_moves_and_clamps() {
        assert_eq!(step(None, 3, true), Some(0));
        assert_eq!(step(Some(2), 3, true), Some(2));
        assert_eq!(step(Some(1), 3, false), Some(0));
        assert_eq!(step(Some(0), 3, false), None);
        assert_eq!(step(None, 3, false), None);
        assert_eq!(step(Some(0), 0, true), None);
    }
}
