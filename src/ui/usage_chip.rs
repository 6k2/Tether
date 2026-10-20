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

/// CPU share above which a tab's usage shows in its title without hovering.
const HIGH_CPU: f32 = 50.0;
/// Memory above which a tab's usage shows in its title without hovering.
const HIGH_MEM: u64 = 1 << 30;

/// Whether usage is worth showing in a tab title unasked.
pub fn is_high(u: &Usage) -> bool {
    u.cpu >= HIGH_CPU || u.mem >= HIGH_MEM
}

/// Appends the usage of tab `uid` (if sampled) to its title: only while the tab is
/// `hovered` or the usage is high, to keep tab titles quiet.
pub fn append_to_title(job: &mut egui::text::LayoutJob, uid: u64, hovered: bool) {
    if !enabled() {
        return;
    }
    let Some(u) = procmon::usage_for(uid) else { return };
    if !hovered && !is_high(&u) {
        return;
    }
    // Busy trees catch the eye a little more.
    let colour = if is_high(&u) { theme::warn().gamma_multiply(0.85) } else { theme::text_faint() };
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

/// The app total (tether and every process it started), e.g. `CPU 4% · 512 MB`.
pub fn total() -> Option<(String, Usage)> {
    if !enabled() {
        return None;
    }
    let snap = procmon::latest()?;
    if snap.total.procs == 0 {
        return None;
    }
    Some((format!("CPU {:.0}% · {}", snap.total.cpu, human_mem(snap.total.mem)), snap.total.clone()))
}
