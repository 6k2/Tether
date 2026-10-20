//! Opening local shells (`pty`): new tabs, the pane "+" menu, respawning on reconnect.
//! A child module of `app` so it can use the app's private state.

use std::path::PathBuf;

use eframe::egui;

use super::TetherApp;
use crate::pty::{self, ShellSpec};
use crate::ssh::terminal::{LinkState, TerminalHandle};
use crate::ui::tabs::{Tab, TabBody};
use crate::ui::terminal_view::TerminalView;

impl TetherApp {
    /// Starts `spec` on its own thread (the pty read/write loops block).
    pub(super) fn spawn_local(&self, spec: &ShellSpec, cwd: Option<PathBuf>, ctx: &egui::Context) -> TerminalView {
        let (handle, rx) = TerminalHandle::new(24, 80);
        let (spec_owned, h, ctx) = (spec.clone(), handle.clone(), ctx.clone());
        let started = std::thread::Builder::new().name("tether-pty".into()).spawn(move || {
            let repaint = {
                let ctx = ctx.clone();
                move |after| ctx.request_repaint_after(after)
            };
            if let Err(e) = pty::run_local_pty(&spec_owned, cwd, h.clone(), rx, repaint) {
                h.set_state(LinkState::Failed(format!("{e:#}")));
            }
            ctx.request_repaint();
        });
        if let Err(e) = started {
            handle.set_state(LinkState::Failed(format!("Could not start a thread for the shell: {e}")));
        }
        let mut view = TerminalView::new(handle);
        view.intercept_nano = false;
        view
    }

    pub(super) fn local_tab(&self, spec: &ShellSpec, cwd: Option<PathBuf>, ctx: &egui::Context) -> Tab {
        Tab::new_local(spec.clone(), self.spawn_local(spec, cwd, ctx))
    }

    /// Opens `spec` (or the default shell) as a tab of the focused pane.
    pub(super) fn open_local(&mut self, spec: Option<&'static ShellSpec>, ctx: &egui::Context) {
        let Some(spec) = spec.or_else(pty::shells::default_shell) else {
            self.toasts.error("No local shell was found on this machine");
            return;
        };
        let tab = self.local_tab(spec, None, ctx);
        self.dock.push_to_focused_leaf(tab);
    }

    /// Opens shells picked from panes' "+" menus in those panes.
    pub(super) fn open_local_in(&mut self, picks: Vec<(egui_dock::NodePath, &'static ShellSpec)>, ctx: &egui::Context) {
        for (path, spec) in picks {
            let tab = self.local_tab(spec, None, ctx);
            match self.dock.leaf_mut(path) {
                Ok(leaf) => {
                    leaf.append_tab(tab);
                    self.dock.set_focused_node_and_surface(path);
                }
                Err(_) => self.dock.push_to_focused_leaf(tab),
            }
        }
    }

    /// Respawns a local tab's shell in place (its "Reconnect" button).
    pub(super) fn respawn_local(&mut self, uid: u64, ctx: &egui::Context) {
        let Some(spec) = self.all_tabs().find(|t| t.uid == uid).and_then(|t| t.local.clone()) else { return };
        let view = self.spawn_local(&spec, None, ctx);
        let tab = self.dock.iter_all_tabs_mut().map(|(_, t)| t).chain(self.detached.tabs_mut()).find(|t| t.uid == uid);
        if let Some(Tab { body: TabBody::Terminal(old), .. }) = tab {
            *old = view;
        }
    }
}

/// The shell a local `tab` runs and the folder it last reported (OSC 7), to start
/// another one like it.
pub(super) fn origin(tab: &Tab) -> Option<(ShellSpec, Option<PathBuf>)> {
    let spec = tab.local.clone()?;
    let cwd = match &tab.body {
        TabBody::Terminal(t) => t.handle.shared.lock().ok().and_then(|s| s.parser.callbacks().cwd.clone()).map(PathBuf::from),
        _ => None,
    };
    Some((spec, cwd))
}
