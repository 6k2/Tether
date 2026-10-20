//! App glue for AI agent sessions: feeding the hub every frame, notifications, the
//! agents panel, jumping to sessions and the "+" menu launchers.
//! A child module of `app` so it can use the app's private state.

use std::time::Instant;

use eframe::egui::{self, RichText};
use egui_dock::DockState;

use super::TetherApp;
use crate::agents::hub::{Alert, TermInfo};
use crate::agents::{AgentState, notify};
use crate::keybindings::{self, Action};
use crate::theme;
use crate::ui::agents_panel::{self, LaunchPick, PanelAction};
use crate::ui::tabs::{Tab, TabBody};

/// Local terminals of `dock`, with whether each is the visible tab of its pane.
fn local_terms(dock: &DockState<Tab>, out: &mut Vec<TermInfo>) {
    for (_, leaf) in dock.iter_leaves() {
        for (i, tab) in leaf.tabs.iter().enumerate() {
            if let (TabBody::Terminal(view), true) = (&tab.body, tab.is_local()) {
                out.push(TermInfo { uid: tab.uid, handle: view.handle.clone(), visible: i == leaf.active.0, label: tab.profile.display_name().to_string() });
            }
        }
    }
}

/// Activates tab `uid` in `dock`; true when found.
fn activate(dock: &mut DockState<Tab>, uid: u64) -> bool {
    let Some(path) = dock.find_tab_from(|t| t.uid == uid) else { return false };
    let _ = dock.set_active_tab(path);
    dock.set_focused_node_and_surface(path.node_path());
    if let Some((_, Tab { body: TabBody::Terminal(view), .. })) = dock.iter_all_tabs_mut().find(|(_, t)| t.uid == uid) {
        view.want_focus = true;
    }
    true
}

impl TetherApp {
    pub(super) fn agents_frame(&mut self, ctx: &egui::Context) {
        let settings = crate::settings::snapshot();
        let window_focused = ctx.input(|i| i.focused);
        let monitor = self.agents.monitor();
        monitor.set_focused(window_focused);
        monitor.set_interval(settings.agents.monitor_interval());
        let mut terms = Vec::new();
        local_terms(&self.dock, &mut terms);
        for dock in self.detached.docks_mut() {
            local_terms(dock, &mut terms);
        }
        self.command_notices(ctx, &terms, window_focused);
        let alerts = self.agents.frame(ctx, terms, Instant::now());
        for alert in alerts {
            self.alert(ctx, alert, window_focused);
        }
    }

    fn alert(&self, ctx: &egui::Context, a: Alert, window_focused: bool) {
        let cfg = &crate::settings::snapshot().agents;
        if a.state == AgentState::Done && !cfg.notify_on_done {
            return;
        }
        let title = format!("{} · {}", a.kind.name(), a.state.label());
        let body = match &a.message {
            Some(m) => format!("{}\n{}", a.project, crate::agents::snippet(m, 160)),
            None => a.project.clone(),
        };
        if !window_focused {
            if cfg.desktop_notifications {
                notify::desktop(title, body);
            }
            if cfg.flash_taskbar {
                ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(egui::UserAttentionType::Informational));
            }
        } else if !a.visible {
            let hint = keybindings::label(Action::NextAttention);
            let jump = if hint.is_empty() { String::new() } else { format!("  ({hint} to jump)") };
            self.toasts.info(format!("{title}: {}{jump}", a.project));
        }
    }

    /// Shell integration: a long command finished in a hidden tab or while tether was in
    /// the background. Same channels as agent alerts (toast, or desktop + taskbar flash).
    fn command_notices(&self, ctx: &egui::Context, terms: &[TermInfo], window_focused: bool) {
        let settings = crate::settings::snapshot();
        for t in terms {
            // Busy terminals are looked at next frame (`take_finished` keeps the news).
            let Some(done) = t.handle.shared.try_lock().ok().and_then(|mut s| s.parser.callbacks_mut().shell.take_finished()) else { continue };
            if t.visible && window_focused {
                continue;
            }
            let Some((title, body)) = crate::ui::blocks::long_command_notice(&done, &settings.shell_integration) else { continue };
            if window_focused {
                self.toasts.info(format!("{title} in {}: {body}", t.label));
                continue;
            }
            if settings.agents.desktop_notifications {
                notify::desktop(format!("{title} · {}", t.label), body);
            }
            if settings.agents.flash_taskbar {
                ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(egui::UserAttentionType::Informational));
            }
        }
    }

    pub(super) fn note_agent_input(&mut self, uid: u64) {
        self.agents.note_input(uid, Instant::now());
    }

    /// Shows tab `uid` wherever it lives and gives it the keyboard.
    pub(super) fn focus_tab(&mut self, uid: u64, ctx: &egui::Context) {
        if activate(&mut self.dock, uid) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            return;
        }
        for dock in self.detached.docks_mut() {
            if activate(dock, uid) {
                return;
            }
        }
    }

    /// Ctrl+Shift+A: the next session waiting for the user (most urgent first).
    pub(super) fn focus_next_attention(&mut self, ctx: &egui::Context) {
        let current = self.focused_terminal.or_else(|| self.dock.find_active_focused().map(|(_, t)| t.uid));
        match self.agents.next_attention(current) {
            Some(uid) => self.focus_tab(uid, ctx),
            None => self.toasts.info("No agent session needs attention"),
        }
    }

    /// Top-bar entry: the agents panel toggle (with a count of waiting sessions) and
    /// the app's CPU / memory total.
    pub(super) fn agents_top_bar(&mut self, ui: &mut egui::Ui) {
        let waiting = self.agents.attention_count();
        let text = if waiting > 0 {
            RichText::new(format!("✳ Agents · {waiting}")).size(12.0).color(theme::warn())
        } else {
            RichText::new("✳ Agents").size(12.0).color(if self.agents_open { theme::text() } else { theme::text_dim() })
        };
        if ui.add(egui::Button::new(text).frame(false)).on_hover_text(keybindings::hint("Agent sessions", Action::ToggleAgents)).clicked() {
            self.agents_open = !self.agents_open;
        }
        crate::ui::usage_chip::total(ui);
    }

    pub(super) fn agents_side_panel(&mut self, ui: &mut egui::Ui) {
        if !self.agents_open {
            return;
        }
        let sessions = self.agents.sessions();
        let now = Instant::now();
        let frame = egui::Frame::new().fill(theme::bg()).inner_margin(egui::Margin { left: 8, right: 10, top: 6, bottom: 10 });
        let action = egui::Panel::right("agents_panel")
            .resizable(true)
            .default_size(agents_panel::WIDTH)
            .size_range(240.0..=460.0)
            .frame(frame)
            .show_separator_line(false)
            .show(ui, |ui| agents_panel::show(ui, &sessions, now))
            .inner;
        // Elapsed times tick while the panel is open.
        if !sessions.is_empty() {
            ui.ctx().request_repaint_after(std::time::Duration::from_secs(1));
        }
        match action {
            Some(PanelAction::Focus(uid)) => self.focus_tab(uid, ui.ctx()),
            Some(PanelAction::Close) => self.agents_open = false,
            None => {}
        }
    }

    /// Opens agents picked from panes' "+" menus: the default shell, then the command.
    pub(super) fn open_agents_in(&mut self, picks: Vec<(egui_dock::NodePath, LaunchPick)>, ctx: &egui::Context) {
        for (path, pick) in picks {
            let Some(shell) = crate::pty::shells::default_shell() else {
                self.toasts.error("No local shell was found on this machine");
                return;
            };
            let cwd = if pick.pick_folder {
                match rfd::FileDialog::new().set_title(format!("Start {} in", pick.kind.name())).pick_folder() {
                    Some(dir) => Some(dir),
                    None => continue,
                }
            } else {
                None
            };
            let tab = self.local_tab(shell, cwd, ctx);
            if let TabBody::Terminal(view) = &tab.body {
                // The pty buffers this until the shell reads its first line.
                view.handle.send(format!("{}\r", pick.command).into_bytes());
            }
            match self.dock.leaf_mut(path) {
                Ok(leaf) => {
                    leaf.append_tab(tab);
                    self.dock.set_focused_node_and_surface(path);
                }
                Err(_) => self.dock.push_to_focused_leaf(tab),
            }
        }
    }
}
