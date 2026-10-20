//! Chips on the right of a vertical tab row: the AI agent state (same colours and
//! glyphs as the dock tab titles, `agent_badge`) and the CPU / memory of the tab's
//! process tree (`usage_chip`). Add more chips here.
//!
//! `ui` is a right-to-left layout of the row's height; each widget lands left of the
//! previous one. The row hides the chips while it shows its close button.

use eframe::egui::{self, RichText, Sense, Ui, Vec2};

use super::tabs::Tab;
use super::{agent_badge, usage_chip};
use crate::theme;

#[derive(Default)]
pub struct TabBadges;

impl TabBadges {
    pub fn row_badges(&self, tab: &Tab, ui: &mut Ui) {
        ui.spacing_mut().item_spacing.x = 6.0;
        if crate::settings::snapshot().agents.show_usage
            && let Some(u) = crate::procmon::usage_for(tab.uid)
        {
            let colour = if u.cpu >= 50.0 { theme::warn().gamma_multiply(0.85) } else { theme::text_faint() };
            let chip = ui.add(egui::Label::new(RichText::new(usage_chip::label(&u)).size(10.5).color(colour)).sense(Sense::hover()));
            chip.on_hover_ui(|ui| usage_chip::breakdown(ui, &u));
        }
        if let Some(b) = agent_badge::for_tab(tab.uid) {
            let time = ui.input(|i| i.time);
            let (rect, resp) = ui.allocate_exact_size(Vec2::splat(14.0), Sense::hover());
            agent_badge::paint(ui.painter(), rect.center(), 5.0, b.state, time);
            resp.on_hover_text(b.state.label());
        }
    }
}
