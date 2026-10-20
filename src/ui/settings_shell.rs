//! Settings → Terminal → Shell integration card.

use eframe::egui::Ui;

use super::settings_widgets::{self as w, SliderSpec};
use crate::shell_integration::ShellIntegrationSettings;

pub fn card(ui: &mut Ui, si: &mut ShellIntegrationSettings) {
    w::card(ui, Some("Shell integration"), |ui| {
        w::row(ui, "Shell integration", "Command blocks, exit codes, folder and git branch from PowerShell, bash, zsh and fish (new tabs; not cmd)", |ui| {
            w::toggle(ui, &mut si.enabled);
        });
        w::separator(ui);
        w::row(ui, "Block marks", "Green / red mark beside each command; hover for details, right-click to copy or rerun", |ui| {
            w::toggle(ui, &mut si.gutter_marks);
        });
        w::separator(ui);
        w::row(ui, "Block separators", "A faint line between commands", |ui| {
            w::toggle(ui, &mut si.separators);
        });
        w::separator(ui);
        w::row(ui, "Sticky command header", "Keeps the command in view while you read its output", |ui| {
            w::toggle(ui, &mut si.sticky_header);
        });
        w::separator(ui);
        w::row(ui, "Notify when long commands finish", "Only when the tab is hidden or tether is in the background", |ui| {
            w::toggle(ui, &mut si.notify_long_commands);
        });
        w::separator(ui);
        w::row(ui, "Long command threshold", "", |ui| {
            let spec = SliderSpec {
                min: ShellIntegrationSettings::NOTIFY_MIN_SECS as f32,
                max: ShellIntegrationSettings::NOTIFY_MAX_SECS as f32,
                step: 1.0,
                log: true,
                format: &|v| format!("{v:.0} s"),
            };
            let mut v = si.notify_after().as_secs() as f32;
            if w::slider_with_value(ui, &mut v, &spec).changed() {
                si.notify_after_secs = v.round() as u32;
            }
        });
    });
}
