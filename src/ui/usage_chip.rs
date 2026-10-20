//! CPU / memory of a local terminal's process tree: a faint suffix in its tab title, a
//! per-process tooltip, and the app total for the top bar. Numbers come from the last
//! `procmon` sample; nothing here waits on the sampler.

use eframe::egui::{self, RichText};

use crate::procmon::{self, Usage, human_mem};
use crate::theme;

fn enabled() -> bool {
    crate::settings::snapshot().agents.show_usage
}

/// `12% · 340 MB`
pub fn label(u: &Usage) -> String {
    format!("{:.0}% · {}", u.cpu, human_mem(u.mem))
}

/// Appends the usage of tab `uid` (if sampled) to its title.
pub fn append_to_title(job: &mut egui::text::LayoutJob, uid: u64) {
    if !enabled() {
        return;
    }
    let Some(u) = procmon::usage_for(uid) else { return };
    // Busy trees catch the eye a little more.
    let colour = if u.cpu >= 50.0 { theme::warn().gamma_multiply(0.85) } else { theme::text_faint() };
    RichText::new(format!("   {}", label(&u))).size(10.5).color(colour).append_to(
        job,
        &egui::Style::default(),
        egui::FontSelection::Default,
        egui::Align::Center,
    );
}

/// Tooltip on a tab button: the busiest processes of the tree.
pub fn tab_tooltip(uid: u64, response: &egui::Response) {
    if !enabled() || !response.hovered() {
        return;
    }
    let Some(u) = procmon::usage_for(uid) else { return };
    response.clone().on_hover_ui(|ui| breakdown(ui, &u));
}

pub fn breakdown(ui: &mut egui::Ui, u: &Usage) {
    ui.label(RichText::new(format!("CPU {:.1}%  ·  {}  ·  {} process{}", u.cpu, human_mem(u.mem), u.procs, if u.procs == 1 { "" } else { "es" })).strong());
    egui::Grid::new("usage_breakdown").num_columns(3).spacing([14.0, 2.0]).show(ui, |ui| {
        for p in &u.top {
            ui.label(RichText::new(&p.name).size(11.5));
            ui.label(RichText::new(format!("{:.1}%", p.cpu)).size(11.5).color(theme::text_dim()));
            ui.label(RichText::new(human_mem(p.mem)).size(11.5).color(theme::text_dim()));
            ui.end_row();
        }
    });
}

/// Top-bar total: tether and every process it started.
pub fn total(ui: &mut egui::Ui) {
    if !enabled() {
        return;
    }
    let Some(snap) = procmon::latest() else { return };
    if snap.total.procs == 0 {
        return;
    }
    let r = ui.label(RichText::new(format!("CPU {:.0}% · {}", snap.total.cpu, human_mem(snap.total.mem))).size(11.0).color(theme::text_faint()));
    r.on_hover_ui(|ui| {
        ui.label("tether and every process it started");
        breakdown(ui, &snap.total);
    });
}
