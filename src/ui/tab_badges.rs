//! What a vertical tab row shows besides its title: the AI agent's mark and status pill
//! (same look as the dock tabs', `agent_status`) and, dimmed, the CPU / memory of the
//! tab's process tree (`usage_chip`). Add more chips here.

use eframe::egui::{self, Id, Pos2, Rect, RichText, Sense, Ui, Vec2};

use super::tabs::Tab;
use super::{agent_badge, agent_icons, agent_status, usage_chip};
use crate::theme;

#[derive(Default)]
pub struct TabBadges;

impl TabBadges {
    /// Chips on the right of the title line. `ui` is a right-to-left layout of the
    /// line's height; each widget lands left of the previous one. `compact` rows have
    /// no second line, so the agent's status shows here as a dot. Returns true when
    /// that dot was clicked (it opens the popover; the row should focus the tab).
    pub fn row_badges(&self, tab: &Tab, ui: &mut Ui, compact: bool) -> bool {
        ui.spacing_mut().item_spacing.x = 6.0;
        let mut clicked = false;
        if compact && let Some(b) = agent_badge::for_tab(tab.uid) {
            let (rect, _) = ui.allocate_exact_size(Vec2::new(agent_status::COMPACT_W, agent_status::PILL_H), Sense::hover());
            clicked = pill_button(ui, tab.uid, rect, b);
        }
        if crate::settings::snapshot().agents.show_usage
            && let Some(u) = crate::procmon::usage_for(tab.uid)
        {
            // Dimmer than the title: the numbers are secondary to the tab's name.
            let colour = if usage_chip::is_high(&u) { theme::warn().gamma_multiply(0.8) } else { theme::text_faint().gamma_multiply(0.6) };
            let chip = ui.add(egui::Label::new(RichText::new(usage_chip::label(&u)).size(10.0).color(colour)).sense(Sense::hover()));
            chip.on_hover_ui(|ui| usage_chip::breakdown(ui, &u));
        }
        clicked
    }
}

/// The status pill of `tab`'s agent with its left edge centred on `left_center` (second
/// line of a full row). Returns its rect and whether it was clicked.
pub fn row_pill(ui: &mut Ui, tab: &Tab, left_center: Pos2) -> Option<(Rect, bool)> {
    let b = agent_badge::for_tab(tab.uid)?;
    let w = agent_status::width(ui.painter(), b.state, false);
    let rect = Rect::from_min_size(Pos2::new(left_center.x, left_center.y - agent_status::PILL_H / 2.0), Vec2::new(w, agent_status::PILL_H));
    Some((rect, pill_button(ui, tab.uid, rect, b)))
}

fn pill_button(ui: &mut Ui, uid: u64, rect: Rect, b: crate::agents::Badge) -> bool {
    let resp = agent_status::button(ui, rect, Id::new(("row_pill", uid)), b.kind, b.state);
    if resp.clicked() {
        agent_status::toggle_popover(ui.ctx(), uid, rect);
    }
    resp.clicked()
}

/// Paints the agent's mark in a row's icon tile; true when an agent runs in `tab`.
pub fn paint_row_icon(p: &egui::Painter, tile: Rect, tab: &Tab, alpha: f32) -> bool {
    let Some(agent) = tab.agent() else { return false };
    p.rect_filled(tile, egui::CornerRadius::same(6), agent_icons::color(agent.kind).gamma_multiply(0.16 * alpha));
    agent_icons::paint(p, tile.shrink(tile.width() * 0.2), agent.kind, alpha);
    true
}
