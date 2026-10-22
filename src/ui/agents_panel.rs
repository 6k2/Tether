//! The "Agents" sidebar section: every AI agent session across tabs and windows, most urgent
//! first, with what it is doing, for how long, its last message and its CPU / memory.

use std::time::{Duration, Instant};

use eframe::egui::{self, Align2, CornerRadius, FontId, Rect, RichText, Sense, Vec2};

use super::agent_badge;
use crate::agents::{AgentKind, AgentState, SessionInfo, launch, snippet};
use crate::procmon::human_mem;
use crate::theme;

pub const WIDTH: f32 = 300.0;
pub fn default_command(kind: AgentKind) -> &'static str {
    kind.id()
}

pub struct CustomBoot {
    pub kind: AgentKind,
    command: String,
    focus: bool,
}

impl CustomBoot {
    pub fn new(kind: AgentKind) -> Self {
        Self { kind, command: default_command(kind).into(), focus: true }
    }

    pub fn show(&mut self, ctx: &egui::Context) -> (Option<String>, bool) {
        let mut command = None;
        let mut close = false;
        let modal = egui::Modal::new(egui::Id::new("custom_agent_boot")).show(ctx, |ui| {
            ui.set_width(440.0);
            ui.horizontal(|ui| {
                super::agent_icons::icon(ui, self.kind, 24.0);
                ui.heading(format!("{} · Custom Boot", self.kind.name()));
            });
            ui.label("Enter the command to run in a new terminal.");
            ui.add_space(8.0);
            let response = ui.add(egui::TextEdit::singleline(&mut self.command).font(egui::TextStyle::Monospace).desired_width(f32::INFINITY));
            if std::mem::take(&mut self.focus) {
                response.request_focus();
            }
            let valid = !self.command.trim().is_empty() && !self.command.contains(['\r', '\n', '\0']);
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let enter = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if ui.add_enabled(valid, egui::Button::new("Start terminal").fill(theme::accent_soft())).clicked() || (valid && enter) {
                    command = Some(self.command.trim().to_owned());
                    close = true;
                }
                if ui.button("Cancel").clicked() {
                    close = true;
                }
            });
        });
        (command, close || modal.should_close())
    }
}
const ROW_H: f32 = 64.0;

pub enum PanelAction {
    Focus(u64),
    Close,
    /// Start an agent from the empty state.
    Launch(LaunchPick),
    /// Settings › AI agents (hook installer).
    InstallHooks,
    CustomBoot(AgentKind),
}

/// An agent picked from a "+" menu.
#[derive(Debug, Clone)]
pub struct LaunchPick {
    pub kind: AgentKind,
    pub command: String,
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
                pick = Some(LaunchPick { kind: l.kind, command: l.command.into(), pick_folder: false });
            }
            if ui.small_button("in folder…").on_hover_text("Choose the folder to start in").clicked() {
                pick = Some(LaunchPick { kind: l.kind, command: l.command.into(), pick_folder: true });
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

const EMPTY_H: f32 = 172.0;

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
                for kind in [AgentKind::Claude, AgentKind::Codex] {
                    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, 30.0), Sense::click());
                    ui.painter().rect_filled(rect, CornerRadius::same(7), if resp.hovered() { theme::accent_soft() } else { theme::surface_3() });
                    super::agent_icons::paint(
                        ui.painter(),
                        Rect::from_center_size(egui::pos2(rect.left() + 16.0, rect.center().y), Vec2::splat(16.0)),
                        kind,
                        1.0,
                    );
                    ui.painter().text(
                        egui::pos2(rect.left() + 32.0, rect.center().y),
                        Align2::LEFT_CENTER,
                        format!("Launch {}", kind.name()),
                        FontId::proportional(12.0),
                        theme::text(),
                    );
                    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, format!("Launch {}", kind.name())));
                    if resp.clicked() {
                        action = Some(PanelAction::Launch(LaunchPick { kind, command: default_command(kind).into(), pick_folder: false }));
                    }
                    resp.context_menu(|ui| {
                        if ui.button("Custom Boot…").clicked() {
                            action = Some(PanelAction::CustomBoot(kind));
                            ui.close();
                        }
                    });
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
