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
use crate::layout::SidebarSection;
use crate::theme;
use crate::ui::agents_panel::{LaunchPick, PanelAction};
use crate::ui::tabs::{Tab, TabBody};

/// Local terminals of `dock`, with whether each is the visible tab of its pane.
fn local_terms<'a>(dock: &'a DockState<Tab>, out: &mut Vec<TermInfo<'a>>) {
    for (_, leaf) in dock.iter_leaves() {
        for (i, tab) in leaf.tabs.iter().enumerate() {
            if let (TabBody::Terminal(view), true) = (&tab.body, tab.is_local()) {
                out.push(TermInfo { uid: tab.uid, handle: &view.handle, visible: i == leaf.active.0, label: tab.profile.display_name() });
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
            // Text a program printed could pose as anything: say where it came from.
            Some(m) if a.from_terminal => format!("{}\nTerminal: {}", a.project, crate::agents::snippet(m, 160)),
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
            RichText::new("✳ Agents").size(12.0).color(if self.agents_shown() { theme::text() } else { theme::text_dim() })
        };
        if ui.add(egui::Button::new(text).frame(false)).on_hover_text(keybindings::hint("Agent sessions", Action::ToggleAgents)).clicked() {
            self.toggle_agents();
        }
        crate::ui::usage_chip::total(ui);
    }

    /// Whether the "Agents" sidebar section is on screen (sidebar open, section shown).
    pub(super) fn agents_shown(&self) -> bool {
        let l = &crate::settings::snapshot().layout;
        l.sidebar_visible && if l.activity_bar { l.active_view == SidebarSection::Agents } else { !l.is_collapsed(SidebarSection::Agents) }
    }

    /// Ctrl+Shift+J / the top-bar button: show the "Agents" section of the sidebar
    /// (opening the sidebar if needed), or fold it away when it is already showing.
    pub(super) fn toggle_agents(&mut self) {
        let shown = self.agents_shown();
        let _ = crate::settings::update(|s| {
            let l = &mut s.layout;
            if shown {
                if l.activity_bar {
                    l.sidebar_visible = false;
                } else {
                    l.set_collapsed(SidebarSection::Agents, true);
                }
            } else {
                l.sidebar_visible = true;
                l.active_view = SidebarSection::Agents;
                l.set_collapsed(SidebarSection::Agents, false);
            }
        });
    }

    pub(super) fn apply_agents_panel(&mut self, action: Option<PanelAction>, ctx: &egui::Context) {
        match action {
            Some(PanelAction::Focus(uid)) => self.focus_tab(uid, ctx),
            Some(PanelAction::Close) => self.toggle_agents(),
            Some(PanelAction::Launch(pick)) => {
                let target = self.dock.focused_leaf().or_else(|| self.dock.iter_leaves().next().map(|(p, _)| p));
                let target = target.unwrap_or(egui_dock::NodePath { surface: egui_dock::SurfaceIndex::main(), node: egui_dock::NodeIndex::root() });
                self.open_agents_in(vec![(target, pick)], ctx);
            }
            Some(PanelAction::InstallHooks) => {
                self.settings_view = Some(crate::ui::settings_view::SettingsView::at(crate::ui::settings_view::Section::Agents));
            }
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
