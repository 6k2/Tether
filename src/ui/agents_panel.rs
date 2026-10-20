//! The "Agents" sidebar section: every AI agent session across tabs and windows, most urgent
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
    /// Start an agent from the empty state.
    Launch(LaunchPick),
    /// Settings › AI agents (hook installer).
    InstallHooks,
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

/// Sessions waiting for the user (the section header shows the count).
pub fn waiting(sessions: &[SessionInfo]) -> usize {
    sessions.iter().filter(|s| s.state.needs_attention() && s.state != AgentState::Done).count()
}

/// Height the session list wants (the sidebar section sizes itself with it).
pub fn list_height(sessions: &[SessionInfo]) -> f32 {
    if sessions.is_empty() { EMPTY_H } else { sessions.len() as f32 * (ROW_H + 4.0) }
}

const EMPTY_H: f32 = 136.0;

/// The sessions without a header (the sidebar's "Agents" section draws its own).
pub fn list(ui: &mut egui::Ui, sessions: &[SessionInfo], now: Instant) -> Option<PanelAction> {
    let mut action = None;
    if sessions.is_empty() {
        return empty_state(ui);
    }
    let time = ui.input(|i| i.time);
    egui::ScrollArea::vertical().id_salt("agents_list").auto_shrink([false, false]).show(ui, |ui| {
        for s in sessions {
            if row(ui, s, now, time) {
                action = Some(PanelAction::Focus(s.uid));
            }
            ui.add_space(4.0);
        }
    });
    action
}

/// A card instead of the list: what shows up here, and the two ways to get started.
fn empty_state(ui: &mut egui::Ui) -> Option<PanelAction> {
    use super::settings_widgets::{Icon, paint_icon};
    let mut action = None;
    ui.add_space(4.0);
    egui::Frame::new()
        .fill(theme::surface_2().gamma_multiply(0.7))
        .stroke(egui::Stroke::new(1.0, theme::border().gamma_multiply(0.6)))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(egui::Margin::same(12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let (tile, _) = ui.allocate_exact_size(Vec2::splat(30.0), Sense::hover());
                ui.painter().rect_filled(tile, CornerRadius::same(8), theme::accent_soft());
                paint_icon(ui.painter(), tile.shrink(7.0), Icon::Sparkle, theme::accent());
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.label(RichText::new("No agent sessions").size(13.0).color(theme::text()));
                    ui.label(RichText::new("Agents in local terminals show up here").size(11.0).color(theme::text_faint()));
                });
            });
            ui.add_space(10.0);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 6.0;
                let width = ui.available_width();
                let wide = |text: RichText, fill| egui::Button::new(text).fill(fill).corner_radius(CornerRadius::same(7)).min_size(Vec2::new(width, 26.0));
                // Claude Code first when several agents are installed. Never waits on the PATH scan.
                let found = launch::found().unwrap_or_default();
                let first = found.iter().find(|l| l.kind == AgentKind::Claude).or_else(|| found.first());
                if let Some(l) = first {
                    let text = RichText::new(format!("{}  Launch {}", l.kind.glyph(), l.kind.name())).size(12.0).color(theme::text());
                    let tip = format!("Run {} in a new default shell", l.command);
                    if ui.add(wide(text, theme::accent_soft())).on_hover_text(tip).clicked() {
                        action = Some(PanelAction::Launch(LaunchPick { kind: l.kind, command: l.command, pick_folder: false }));
                    }
                }
                let hooks = RichText::new("Install hooks…").size(12.0).color(theme::text_dim());
                let tip = "Settings › AI agents: exact states (working, waiting, done) from the agents' hooks";
                if ui.add(wide(hooks, theme::surface_3())).on_hover_text(tip).clicked() {
                    action = Some(PanelAction::InstallHooks);
                }
            });
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
    let title_pos = egui::pos2(left + 20.0, rect.top() + 18.0);
    let icon = Rect::from_center_size(egui::pos2(title_pos.x + 7.0, title_pos.y), Vec2::splat(14.0));
    super::agent_icons::paint(p, icon, s.kind, 1.0);
    let name_w = rect.right() - icon.right() - 80.0;
    let title = clip(&s.title, name_w, 12.5);
    p.text(egui::pos2(icon.right() + 6.0, title_pos.y), Align2::LEFT_CENTER, title, FontId::proportional(12.5), theme::text());
    let age = elapsed(now.saturating_duration_since(s.since));
    p.text(egui::pos2(rect.right() - 10.0, title_pos.y), Align2::RIGHT_CENTER, age, FontId::proportional(10.5), theme::text_faint());

    let line2 = egui::pos2(left + 20.0, rect.top() + 36.0);
    let state = p.text(line2, Align2::LEFT_CENTER, s.state.label(), FontId::proportional(11.0), agent_badge::color(s.state));
    if s.project != s.title {
        let project = clip(&s.project, rect.right() - state.right() - 100.0, 11.0);
        p.text(egui::pos2(state.right() + 6.0, line2.y), Align2::LEFT_CENTER, format!("· {project}"), FontId::proportional(11.0), theme::text_faint());
    }
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
