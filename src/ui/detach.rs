//! Tabs dragged outside the app window become native OS windows (egui immediate
//! viewports), each hosting a small dock of its own so tabs, splits and drags keep
//! working there. Moving such a window back over the app docks its tabs again.
//!
//! Tabs are moved, never rebuilt: SSH sessions survive detach and re-attach.

use std::time::Duration;

use eframe::egui::{self, CornerRadius, FontId, Id, LayerId, Order, Pos2, Rect, Stroke, StrokeKind, Vec2, ViewportBuilder, ViewportCommand, ViewportId, vec2};
use egui_dock::{DockArea, DockState, Node, NodeIndex, NodePath, Split, Surface, SurfaceIndex, TabPath, TabViewer, Tree};

use super::detach_geom::{self as geom, DockGate, DockSide, GateState};
use super::tabs::{Tab, Viewer};
use crate::theme;

/// `DockArea`'s default id, used by the main window's dock.
const MAIN_DOCK_ID: &str = "egui_dock::DockArea";
const DEFAULT_SIZE: Vec2 = vec2(760.0, 460.0);
const ZONE_FADE_SECS: f32 = 0.18;
/// Share of the old content kept when a window docks as a side split.
const SIDE_SPLIT_FRACTION: f32 = 0.6;

/// Follows one tab drag inside one viewport.
#[derive(Default)]
struct DragTrack {
    uid: Option<u64>,
    pos: Option<Pos2>,
    size: Option<Vec2>,
}

/// A tab released outside the viewport it was dragged in, in screen points.
struct OutDrop {
    uid: u64,
    screen: Pos2,
    size: Vec2,
}

struct DetachedWindow {
    key: u64,
    viewport: ViewportId,
    dock_id: Id,
    dock: DockState<Tab>,
    /// Initial outer rect; kept constant so egui never re-sends position/size commands.
    spawn: Rect,
    gate: DockGate,
    track: DragTrack,
    outer: Option<Rect>,
}

struct WindowOut {
    close_requested: bool,
    drop: Option<OutDrop>,
}

/// Owns every detached window and the tabs living in them.
#[derive(Default)]
pub struct Detacher {
    windows: Vec<DetachedWindow>,
    main_track: DragTrack,
    next_key: u64,
    zone: Option<DockSide>,
    last_zone: Option<DockSide>,
}

impl Detacher {
    /// Every tab living in a detached window.
    pub fn tabs(&self) -> impl Iterator<Item = &Tab> {
        self.windows.iter().flat_map(|w| w.dock.iter_all_tabs().map(|(_, t)| t))
    }

    pub fn tabs_mut(&mut self) -> impl Iterator<Item = &mut Tab> {
        self.windows.iter_mut().flat_map(|w| w.dock.iter_all_tabs_mut().map(|(_, t)| t))
    }

    pub fn docks_mut(&mut self) -> impl Iterator<Item = &mut DockState<Tab>> {
        self.windows.iter_mut().map(|w| &mut w.dock)
    }

    /// Runs once per frame of the main window, after the main dock was shown.
    /// Renders the detached windows (their tab output is collected into `viewer`),
    /// moves tabs between windows, and docks windows that rest over the app.
    pub fn frame(&mut self, ctx: &egui::Context, main: &mut DockState<Tab>, viewer: &mut Viewer) {
        let (main_inner, main_outer, content, now) = ctx.input(|i| (i.viewport().inner_rect, i.viewport().outer_rect, i.content_rect(), i.time));
        let mut drops: Vec<(Option<u64>, OutDrop)> = Vec::new();
        if let Some(d) = track_drag(ctx, main, Id::new(MAIN_DOCK_ID), &mut self.main_track) {
            drops.push((None, d));
        }
        if let Some(inner) = main_inner {
            for (tabs, rect) in take_outside_surfaces(main, content) {
                self.spawn(tabs, rect.translate(inner.min.to_vec2()));
            }
        }
        let mut closed = Vec::new();
        for w in &mut self.windows {
            let out = show_window(ctx, w, viewer);
            if out.close_requested {
                closed.push(w.key);
            }
            if let Some(d) = out.drop {
                drops.push((Some(w.key), d));
            }
        }
        for key in closed {
            if let Some(w) = self.remove(key) {
                for mut tab in dock_tabs(w.dock) {
                    // Same as the dock's ✕: terminal sessions are closed.
                    let _ = viewer.on_close(&mut tab);
                }
            }
        }
        for (src, d) in drops {
            self.apply_drop(main, src, d, main_inner);
        }
        self.windows.retain(|w| w.dock.iter_all_tabs().next().is_some());
        self.zone = None;
        if let Some(main_outer) = main_outer {
            self.update_gates(ctx, main, main_outer, now);
        }
        self.paint_zone(ctx);
    }

    fn spawn(&mut self, tabs: Vec<Tab>, outer: Rect) {
        if tabs.is_empty() {
            return;
        }
        self.next_key += 1;
        let key = self.next_key;
        self.windows.push(DetachedWindow {
            key,
            viewport: ViewportId::from_hash_of(("tether_detached", key)),
            dock_id: Id::new(("tether_detached_dock", key)),
            dock: DockState::new(tabs),
            spawn: outer,
            gate: DockGate::default(),
            track: DragTrack::default(),
            outer: None,
        });
    }

    fn remove(&mut self, key: u64) -> Option<DetachedWindow> {
        let i = self.windows.iter().position(|w| w.key == key)?;
        Some(self.windows.remove(i))
    }

    fn apply_drop(&mut self, main: &mut DockState<Tab>, src: Option<u64>, d: OutDrop, main_inner: Option<Rect>) {
        let over_window = self.windows.iter().find(|w| Some(w.key) != src && w.outer.is_some_and(|r| r.contains(d.screen))).map(|w| w.key);
        let over_main = over_window.is_none() && main_inner.is_some_and(|r| r.contains(d.screen));
        if over_main && src.is_none() {
            return;
        }
        let lone_in_own_window = src.is_some_and(|k| self.windows.iter().any(|w| w.key == k && w.dock.iter_all_tabs().count() <= 1));
        if !over_main && over_window.is_none() && lone_in_own_window {
            return; // the window itself is what should move; nothing to split off
        }
        let Some(tab) = self.take_tab(main, src, d.uid) else { return };
        if over_main {
            main.push_to_focused_leaf(tab);
        } else if let Some(w) = over_window.and_then(|k| self.windows.iter_mut().find(|w| w.key == k)) {
            w.dock.push_to_focused_leaf(tab);
        } else {
            self.spawn(vec![tab], geom::spawn_rect(d.screen, d.size));
        }
    }

    fn take_tab(&mut self, main: &mut DockState<Tab>, src: Option<u64>, uid: u64) -> Option<Tab> {
        let dock = match src {
            None => main,
            Some(k) => &mut self.windows.iter_mut().find(|w| w.key == k)?.dock,
        };
        let path = dock.find_tab_from(|t| t.uid == uid)?;
        dock.remove_tab(path)
    }

    fn update_gates(&mut self, ctx: &egui::Context, main: &mut DockState<Tab>, main_outer: Rect, now: f64) {
        let mut docking = None;
        for w in &mut self.windows {
            let Some(outer) = w.outer else { continue };
            let side = geom::dock_side(geom::title_probe(outer), main_outer);
            match w.gate.update(outer, main_outer, now) {
                GateState::Idle => {}
                GateState::Hover => {
                    self.zone = Some(side);
                    ctx.request_repaint_after(Duration::from_secs_f64(geom::DOCK_STILL_SECS / 3.0));
                }
                GateState::Dock => {
                    docking = Some((w.key, side));
                    break;
                }
            }
        }
        let Some((key, side)) = docking else { return };
        let Some(w) = self.remove(key) else { return };
        redock(main, dock_tabs(w.dock), side);
        self.zone = None;
        ctx.send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::Focus);
    }

    /// Accent glow + "Release to dock" pill over the part of the app a window would dock into.
    fn paint_zone(&mut self, ctx: &egui::Context) {
        if self.zone.is_some() {
            self.last_zone = self.zone;
        }
        let t = ctx.animate_bool_with_time(Id::new("tether_dock_zone"), self.zone.is_some(), ZONE_FADE_SECS);
        let Some(side) = self.last_zone.filter(|_| t > 0.0) else { return };
        let rect = geom::zone_rect(side, ctx.content_rect().shrink(6.0));
        let painter = ctx.layer_painter(LayerId::new(Order::Foreground, Id::new("tether_dock_zone")));
        let accent = theme::accent();
        let radius = CornerRadius::same(theme::RADIUS);
        let pulse = ((ctx.input(|i| i.time) * 4.0).sin() * 0.5 + 0.5) as f32;
        painter.rect_filled(rect, radius, accent.gamma_multiply((0.07 + 0.04 * pulse) * t));
        for (grow, alpha) in [(0.0, 0.9), (3.0, 0.3), (7.0, 0.12)] {
            painter.rect_stroke(rect.expand(grow), radius, Stroke::new(2.0, accent.gamma_multiply(alpha * t)), StrokeKind::Inside);
        }
        let galley = painter.layout_no_wrap("⬇  Release to dock".into(), FontId::proportional(13.0), theme::bg());
        let pill = Rect::from_center_size(rect.center() + vec2(0.0, (1.0 - t) * 10.0), galley.size() + vec2(28.0, 14.0));
        painter.rect_filled(pill, CornerRadius::same((pill.height() / 2.0) as u8), accent.gamma_multiply(t));
        painter.galley(pill.center() - galley.size() / 2.0, galley, theme::bg().gamma_multiply(t));
        if self.zone.is_some() {
            ctx.request_repaint_after(Duration::from_millis(33));
        }
    }
}

fn redock(main: &mut DockState<Tab>, tabs: Vec<Tab>, side: DockSide) {
    if tabs.is_empty() {
        return;
    }
    let split = match side {
        DockSide::Left => Some(Split::Left),
        DockSide::Right => Some(Split::Right),
        DockSide::Center => None,
    };
    match split {
        Some(split) if !main.main_surface().is_empty() => {
            let root = NodePath { surface: SurfaceIndex::main(), node: NodeIndex::root() };
            main.split(root, split, SIDE_SPLIT_FRACTION, Node::leaf_with(tabs));
        }
        _ => {
            for tab in tabs {
                main.push_to_focused_leaf(tab);
            }
        }
    }
}

fn show_window(ctx: &egui::Context, w: &mut DetachedWindow, viewer: &mut Viewer) -> WindowOut {
    let builder = ViewportBuilder::default()
        .with_title(window_title(&mut w.dock, viewer))
        .with_position(w.spawn.min)
        .with_inner_size(w.spawn.size())
        .with_min_inner_size(geom::MIN_WINDOW * 0.75);
    ctx.show_viewport_immediate(w.viewport, builder, |ui, _class| {
        let ctx = ui.ctx().clone();
        let (outer, close_requested) = ctx.input(|i| (i.viewport().outer_rect, i.viewport().close_requested()));
        w.outer = outer;
        let frame = egui::Frame::new().fill(theme::bg()).inner_margin(egui::Margin::same(0));
        egui::CentralPanel::default().frame(frame).show(ui, |ui| {
            let style = theme::dock_style(ui.style());
            DockArea::new(&mut w.dock)
                .id(w.dock_id)
                .style(style)
                .show_add_buttons(false)
                .show_leaf_collapse_buttons(false)
                .show_leaf_close_all_buttons(false)
                .show_inside(ui, viewer);
        });
        let drop = track_drag(&ctx, &w.dock, w.dock_id, &mut w.track);
        WindowOut { close_requested, drop }
    })
}

/// "<tab name> — tether", from the window's active tab.
fn window_title(dock: &mut DockState<Tab>, viewer: &mut Viewer) -> String {
    let tab = if dock.find_active_focused().is_some() { dock.find_active_focused().map(|(_, t)| t) } else { dock.iter_all_tabs_mut().next().map(|(_, t)| t) };
    let Some(tab) = tab else { return "tether".into() };
    let fallback = tab.profile.display_name().to_string();
    let text = viewer.title(tab).text().to_string();
    // The dock title is "• <icon>  <name>".
    let name = text.split_once("  ").map(|(_, n)| n.trim().to_string()).filter(|n| !n.is_empty()).unwrap_or(fallback);
    format!("{name} — tether")
}

fn tab_drag_id(dock_id: Id, path: TabPath) -> Id {
    dock_id.with((path.surface, "surface")).with((path.node, "node")).with((path.tab.0, "tab"))
}

/// The tab of `dock` egui is dragging right now, with the size of its leaf.
fn dragged_tab(ctx: &egui::Context, dock: &DockState<Tab>, dock_id: Id) -> Option<(u64, Option<Vec2>)> {
    let dragged = ctx.dragged_id()?;
    let (path, tab) = dock.iter_all_tabs().find(|(path, _)| {
        let id = tab_drag_id(dock_id, *path);
        dragged == id || dragged == id.with("dragged")
    })?;
    Some((tab.uid, dock.leaf(path.node_path()).ok().map(|l| l.rect.size()).filter(|s| s.x > 0.0 && s.y > 0.0)))
}

/// Follows a tab drag in the current viewport; returns it when released outside the viewport.
fn track_drag(ctx: &egui::Context, dock: &DockState<Tab>, dock_id: Id, track: &mut DragTrack) -> Option<OutDrop> {
    let (down, released, pos, decided, content, inner) = ctx.input(|i| {
        (
            i.pointer.primary_down(),
            i.pointer.primary_released(),
            i.pointer.latest_pos(),
            i.pointer.is_decidedly_dragging(),
            i.content_rect(),
            i.viewport().inner_rect,
        )
    });
    if released {
        let t = std::mem::take(track);
        let (uid, pos, inner) = (t.uid?, pos.or(t.pos)?, inner?);
        if content.contains(pos) {
            return None;
        }
        return Some(OutDrop { uid, screen: inner.min + pos.to_vec2(), size: t.size.unwrap_or(DEFAULT_SIZE) });
    }
    if !down {
        *track = DragTrack::default();
        return None;
    }
    if decided && let Some((uid, size)) = dragged_tab(ctx, dock, dock_id) {
        track.uid = Some(uid);
        track.size = size.or(track.size);
    }
    if track.uid.is_some() && pos.is_some() {
        track.pos = pos;
    }
    None
}

/// Floating dock windows of the main dock that sit mostly outside the app, with their rects.
fn take_outside_surfaces(main: &mut DockState<Tab>, content: Rect) -> Vec<(Vec<Tab>, Rect)> {
    let outside: Vec<(SurfaceIndex, Rect)> = main
        .iter_surfaces_indexed()
        .filter_map(|(i, s)| match s {
            Surface::Window(_, state) if !state.dragged() && geom::outside_fraction(state.rect(), content) >= geom::DETACH_OUTSIDE_FRACTION => {
                Some((i, state.rect()))
            }
            _ => None,
        })
        .collect();
    outside
        .into_iter()
        .rev()
        .filter_map(|(i, rect)| match main.remove_surface(i)? {
            Surface::Window(mut tree, _) => Some((tree_tabs(&mut tree), rect)),
            _ => None,
        })
        .collect()
}

fn tree_tabs(tree: &mut Tree<Tab>) -> Vec<Tab> {
    let mut out = Vec::new();
    for node in tree.iter_mut() {
        if let Node::Leaf(leaf) = node {
            out.append(&mut leaf.tabs);
        }
    }
    out
}

fn dock_tabs(mut dock: DockState<Tab>) -> Vec<Tab> {
    let mut out = Vec::new();
    for surface in dock.iter_surfaces_mut() {
        if let Some(tree) = surface.node_tree_mut() {
            out.append(&mut tree_tabs(tree));
        }
    }
    out
}
