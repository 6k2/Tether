//! The "Agents" side panel: every AI agent session across tabs and windows, most urgent
//! first, with what it is doing, for how long, its last message and its CPU / memory.

use std::time::{Duration, Instant};

use eframe::egui::{self, Align2, CornerRadius, FontId, Rect, RichText, Sense, Vec2};

use super::agent_badge;
use crate::agents::{AgentKind, AgentState, SessionInfo, launch, snippet};
use crate::procmon::human_mem;
use crate::theme;

pub const WIDTH: f32 = 300.0;
const ROW_H: f32 = 64.0;

pub enum PanelAction {
    Focus(u64),
    Close,
}

/// An agent picked from a "+" menu.
#[derive(Debug, Clone, Copy)]
pub struct LaunchPick {
    pub kind: AgentKind,
    pub command: &'static str,
    /// Ask for the folder to start in first.
    pub pick_folder: bool,
}

/// "Claude Code" / "Codex" entries for a "+" menu: only agents found on PATH.
pub fn launch_menu(ui: &mut egui::Ui) -> Option<LaunchPick> {
    let found = launch::available();
    if found.is_empty() {
        return None;
    }
    ui.separator();
    let mut pick = None;
    for l in found {
        ui.horizontal(|ui| {
            let text = RichText::new(format!("{}  {}", l.kind.glyph(), l.kind.name())).color(theme::hue_color(l.kind.hue()));
            if ui.button(text).on_hover_text(format!("Run {} in a new default shell\n{}", l.command, l.path.display())).clicked() {
                pick = Some(LaunchPick { kind: l.kind, command: l.command, pick_folder: false });
            }
            if ui.small_button("in folder…").on_hover_text("Choose the folder to start in").clicked() {
                pick = Some(LaunchPick { kind: l.kind, command: l.command, pick_folder: true });
            }
        });
    }
    if pick.is_some() {
        ui.close();
    }
    pick
}

/// `42s`, `5m`, `2h 07m`.
pub fn elapsed(d: Duration) -> String {
    let s = d.as_secs();
    match s {
        0..60 => format!("{s}s"),
        60..3600 => format!("{}m", s / 60),
        _ => format!("{}h {:02}m", s / 3600, (s % 3600) / 60),
    }
}

pub fn show(ui: &mut egui::Ui, sessions: &[SessionInfo], now: Instant) -> Option<PanelAction> {
    let mut action = None;
    ui.horizontal(|ui| {
        ui.label(RichText::new("Agents").size(14.0).strong());
        let waiting = sessions.iter().filter(|s| s.state.needs_attention() && s.state != AgentState::Done).count();
        if waiting > 0 {
            ui.label(RichText::new(format!("{waiting} waiting")).size(11.0).color(theme::warn()));
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.add(egui::Button::new(RichText::new("×").size(15.0)).frame(false)).on_hover_text("Hide the agents panel").clicked() {
                action = Some(PanelAction::Close);
            }
        });
    });
    ui.add_space(6.0);
    if sessions.is_empty() {
        ui.add_space(12.0);
        ui.label(RichText::new("No agent sessions").color(theme::text_dim()));
        ui.label(
            RichText::new(
                "Run claude, codex, gemini, opencode or aider in a local terminal and it shows up here. Settings → AI agents installs hooks for exact states.",
            )
            .size(11.5)
            .color(theme::text_faint()),
        );
        return action;
    }
    let time = ui.input(|i| i.time);
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        for s in sessions {
            if row(ui, s, now, time) {
                action = Some(PanelAction::Focus(s.uid));
            }
            ui.add_space(4.0);
        }
    });
    action
}

/// One session card. Returns true when clicked.
fn row(ui: &mut egui::Ui, s: &SessionInfo, now: Instant, time: f64) -> bool {
    let width = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, ROW_H), Sense::click());
    let resp = resp.on_hover_text(hover_text(s));
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let p = ui.painter();
    let attention = s.state.needs_attention() && s.state != AgentState::Done;
    let fill = if resp.hovered() { theme::surface_2() } else { theme::surface() };
    p.rect_filled(rect, CornerRadius::same(theme::RADIUS), fill);
    if attention {
        let bar = Rect::from_min_size(rect.min, Vec2::new(3.0, rect.height()));
        p.rect_filled(bar, CornerRadius { nw: theme::RADIUS, sw: theme::RADIUS, ne: 0, se: 0 }, agent_badge::color(s.state));
    }
    let left = rect.left() + 14.0;
    agent_badge::paint(p, egui::pos2(left + 6.0, rect.top() + 18.0), 6.0, s.state, time);
    let kind_colour = theme::hue_color(s.kind.hue());
    let title_pos = egui::pos2(left + 20.0, rect.top() + 18.0);
    let glyph = p.text(title_pos, Align2::LEFT_CENTER, s.kind.glyph(), FontId::proportional(12.0), kind_colour);
    let name_w = rect.right() - glyph.right() - 80.0;
    let project = clip(&s.project, name_w, 12.5);
    p.text(egui::pos2(glyph.right() + 6.0, title_pos.y), Align2::LEFT_CENTER, project, FontId::proportional(12.5), theme::text());
    let age = elapsed(now.saturating_duration_since(s.since));
    p.text(egui::pos2(rect.right() - 10.0, title_pos.y), Align2::RIGHT_CENTER, age, FontId::proportional(10.5), theme::text_faint());

    let line2 = egui::pos2(left + 20.0, rect.top() + 36.0);
    p.text(line2, Align2::LEFT_CENTER, s.state.label(), FontId::proportional(11.0), agent_badge::color(s.state));
    if let Some(u) = &s.usage {
        let usage = format!("{:.0}% · {}", u.cpu, human_mem(u.mem));
        p.text(egui::pos2(rect.right() - 10.0, line2.y), Align2::RIGHT_CENTER, usage, FontId::proportional(10.5), theme::text_faint());
    }
    if let Some(m) = &s.message {
        let text = clip(&snippet(m, 120), rect.width() - 44.0, 11.0);
        p.text(egui::pos2(left + 20.0, rect.top() + 52.0), Align2::LEFT_CENTER, text, FontId::proportional(11.0), theme::text_dim());
    }
    resp.clicked()
}

fn hover_text(s: &SessionInfo) -> String {
    let mut t = format!("{} · {}", s.kind.name(), s.state.label());
    if let Some(cwd) = &s.cwd {
        t.push_str(&format!("\n{cwd}"));
    }
    if let Some(m) = &s.message {
        t.push_str(&format!("\n\n{}", snippet(m, 400)));
    }
    t.push_str(if s.hooked { "\n\nState from hooks" } else { "\n\nState guessed from the terminal (install hooks in Settings → AI agents)" });
    t.push_str("\nClick to focus the tab");
    t
}

/// Cuts `text` to roughly `width` points (proportional font estimate).
fn clip(text: &str, width: f32, size: f32) -> String {
    let max = ((width / (size * 0.55)).floor() as usize).max(4);
    snippet(text, max)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elapsed_formats() {
        assert_eq!(elapsed(Duration::from_secs(42)), "42s");
        assert_eq!(elapsed(Duration::from_secs(300)), "5m");
        assert_eq!(elapsed(Duration::from_secs(7620)), "2h 07m");
    }
}
