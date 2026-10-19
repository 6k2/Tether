//! Pure geometry for detaching tabs into native windows and docking them back.
//!
//! All rects are in screen space, in egui points (as reported by `ViewportInfo`).

use eframe::egui::{Pos2, Rect, Vec2, pos2, vec2};

/// A floating dock window this far outside the app's content area becomes a native window.
pub const DETACH_OUTSIDE_FRACTION: f32 = 0.4;
/// The title bar of a detached window this close to the app window counts as "over the app".
pub const DOCK_NEAR: f32 = 24.0;
/// A detached window must first move this far away from the app before it may dock again.
pub const ARM_AWAY: f32 = 64.0;
/// How long a detached window has to rest over the app (after moving there) before it docks.
pub const DOCK_STILL_SECS: f64 = 0.3;
/// Fraction of the app width at each side that docks as a split instead of a new tab.
const SIDE_BAND: f32 = 0.2;
/// Distance from the window top to the middle of a typical OS title bar.
const TITLE_PROBE_DY: f32 = 16.0;
/// Where the pointer ends up inside a freshly detached window (roughly on its title bar).
const GRAB_OFFSET: Vec2 = vec2(70.0, 14.0);
pub const MIN_WINDOW: Vec2 = vec2(420.0, 260.0);
/// How far past the app edge in-app floating dock windows may be dragged. egui_dock
/// clamps them to its window bounds (the app by default), so they could never leave.
const FLOAT_SLACK: f32 = 8192.0;
/// A pointer released this close to a floating window was dragging it.
const GRAB_SLOP: f32 = 32.0;

/// Bounds for the main dock's floating windows: far beyond the app, so they can be
/// dragged out of it (and then become native windows).
pub fn floating_bounds(content: Rect) -> Rect {
    content.expand(FLOAT_SLACK)
}

/// What to do with an in-app floating dock window once no drag is in progress.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FloatVerdict {
    Keep,
    /// Turn it into a native window.
    Detach,
    /// Partly off the app but not enough to detach: move it back to this position.
    PullBack(Pos2),
}

/// `rect` is the floating window and `content` the app area (same space); `released_at`
/// is the pointer position when the button was released this frame.
pub fn floating_verdict(rect: Rect, content: Rect, released_at: Option<Pos2>) -> FloatVerdict {
    if !rect.is_positive() || !rect.is_finite() {
        return FloatVerdict::Keep;
    }
    let dropped_outside = released_at.is_some_and(|p| !content.contains(p) && rect.expand(GRAB_SLOP).contains(p));
    if dropped_outside || outside_fraction(rect, content) >= DETACH_OUTSIDE_FRACTION {
        return FloatVerdict::Detach;
    }
    if content.contains_rect(rect) {
        return FloatVerdict::Keep;
    }
    // A window bigger than the app aligns with its top-left corner.
    let clamp = |v: f32, lo: f32, hi: f32| if hi < lo { lo } else { v.clamp(lo, hi) };
    FloatVerdict::PullBack(pos2(
        clamp(rect.min.x, content.min.x, content.max.x - rect.width()),
        clamp(rect.min.y, content.min.y, content.max.y - rect.height()),
    ))
}

/// Share of `rect` (0..=1) that lies outside `bounds`. Degenerate rects count as inside.
pub fn outside_fraction(rect: Rect, bounds: Rect) -> f32 {
    let area = rect.area();
    if !area.is_finite() || area <= 0.0 {
        return 0.0;
    }
    let overlap = rect.intersect(bounds);
    let inside = if overlap.is_positive() { overlap.area() } else { 0.0 };
    (1.0 - inside / area).clamp(0.0, 1.0)
}

/// The point the user is most likely holding while moving a native window: its title bar.
pub fn title_probe(outer: Rect) -> Pos2 {
    pos2(outer.center().x, (outer.min.y + TITLE_PROBE_DY).min(outer.max.y))
}

pub fn in_dock_zone(probe: Pos2, main: Rect) -> bool {
    main.expand(DOCK_NEAR).contains(probe)
}

pub fn is_away(probe: Pos2, main: Rect) -> bool {
    !main.expand(ARM_AWAY).contains(probe)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DockSide {
    Center,
    Left,
    Right,
}

/// Which part of the app a window dropped at `probe` docks into.
pub fn dock_side(probe: Pos2, main: Rect) -> DockSide {
    if main.width() <= 0.0 {
        return DockSide::Center;
    }
    let t = (probe.x - main.min.x) / main.width();
    if t < SIDE_BAND {
        DockSide::Left
    } else if t > 1.0 - SIDE_BAND {
        DockSide::Right
    } else {
        DockSide::Center
    }
}

/// The highlighted area (in the same space as `content`) for a dock side.
pub fn zone_rect(side: DockSide, content: Rect) -> Rect {
    let w = content.width() * 0.4;
    match side {
        DockSide::Center => content,
        DockSide::Left => Rect::from_min_max(content.min, pos2(content.min.x + w, content.max.y)),
        DockSide::Right => Rect::from_min_max(pos2(content.max.x - w, content.min.y), content.max),
    }
}

/// Outer rect of a new native window so the pointer sits on its title bar.
pub fn spawn_rect(screen_pointer: Pos2, size: Vec2) -> Rect {
    Rect::from_min_size(screen_pointer - GRAB_OFFSET, size.max(MIN_WINDOW))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GateState {
    /// Nothing to show.
    Idle,
    /// Over the app and moving: show the dock highlight.
    Hover,
    /// Rested over the app long enough: dock now.
    Dock,
}

/// Decides when a detached window docks back, without flicker or ping-pong:
/// it must have been away from the app once (armed), then be moved over it,
/// then rest there for [`DOCK_STILL_SECS`].
#[derive(Debug, Default)]
pub struct DockGate {
    armed: bool,
    last: Option<Rect>,
    moved_at: Option<f64>,
}

impl DockGate {
    pub fn update(&mut self, outer: Rect, main: Rect, now: f64) -> GateState {
        let moved = self.last.is_some_and(|r| r != outer);
        self.last = Some(outer);
        let probe = title_probe(outer);
        if !self.armed {
            self.armed = is_away(probe, main);
            self.moved_at = None;
            return GateState::Idle;
        }
        if !in_dock_zone(probe, main) {
            self.moved_at = None;
            return GateState::Idle;
        }
        if moved {
            self.moved_at = Some(now);
        }
        match self.moved_at {
            None => GateState::Idle,
            Some(t) if now - t >= DOCK_STILL_SECS => GateState::Dock,
            Some(_) => GateState::Hover,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn main_rect() -> Rect {
        Rect::from_min_size(pos2(100.0, 100.0), vec2(1000.0, 700.0))
    }

    /// A 400x300 window whose title probe lands on `probe`.
    fn window_at(probe: Pos2) -> Rect {
        Rect::from_min_size(pos2(probe.x - 200.0, probe.y - TITLE_PROBE_DY), vec2(400.0, 300.0))
    }

    #[test]
    fn outside_fraction_inside_partial_and_outside() {
        let b = Rect::from_min_size(Pos2::ZERO, vec2(100.0, 100.0));
        assert_eq!(outside_fraction(Rect::from_min_size(pos2(10.0, 10.0), vec2(20.0, 20.0)), b), 0.0);
        let half = outside_fraction(Rect::from_min_size(pos2(80.0, 0.0), vec2(40.0, 10.0)), b);
        assert!((half - 0.5).abs() < 1e-5);
        assert_eq!(outside_fraction(Rect::from_min_size(pos2(200.0, 0.0), vec2(10.0, 10.0)), b), 1.0);
        assert_eq!(outside_fraction(Rect::NOTHING, b), 0.0);
        assert!(outside_fraction(Rect::from_min_size(pos2(75.0, 0.0), vec2(50.0, 10.0)), b) >= DETACH_OUTSIDE_FRACTION);
    }

    #[test]
    fn zone_and_away_have_hysteresis() {
        let m = main_rect();
        assert!(in_dock_zone(pos2(500.0, 400.0), m));
        assert!(in_dock_zone(pos2(m.max.x + DOCK_NEAR - 1.0, 400.0), m));
        let between = pos2(m.max.x + (DOCK_NEAR + ARM_AWAY) / 2.0, 400.0);
        assert!(!in_dock_zone(between, m) && !is_away(between, m));
        assert!(is_away(pos2(m.max.x + ARM_AWAY + 1.0, 400.0), m));
    }

    #[test]
    fn dock_side_bands() {
        let m = main_rect();
        assert_eq!(dock_side(pos2(150.0, 300.0), m), DockSide::Left);
        assert_eq!(dock_side(pos2(600.0, 300.0), m), DockSide::Center);
        assert_eq!(dock_side(pos2(1050.0, 300.0), m), DockSide::Right);
        let z = zone_rect(DockSide::Right, m);
        assert_eq!(z.max, m.max);
        assert!((z.width() - 400.0).abs() < 1e-3);
    }

    #[test]
    fn spawn_puts_pointer_on_title_bar_and_enforces_min_size() {
        let p = pos2(500.0, 500.0);
        let r = spawn_rect(p, vec2(100.0, 100.0));
        assert!(r.contains(p));
        assert_eq!(r.size(), MIN_WINDOW);
        assert!(p.y - r.min.y < 30.0);
    }

    #[test]
    fn floating_window_dropped_outside_detaches() {
        let app = Rect::from_min_size(Pos2::ZERO, vec2(1000.0, 700.0));
        let w = Rect::from_min_size(pos2(800.0, 100.0), vec2(300.0, 200.0));
        assert!(outside_fraction(w, app) < DETACH_OUTSIDE_FRACTION);
        // Released with the pointer on it past the right edge: detach, though only a third is out.
        assert_eq!(floating_verdict(w, app, Some(pos2(1050.0, 110.0))), FloatVerdict::Detach);
        // Mostly outside: detach even without a release this frame.
        let far = Rect::from_min_size(pos2(950.0, 100.0), vec2(300.0, 200.0));
        assert_eq!(floating_verdict(far, app, None), FloatVerdict::Detach);
    }

    #[test]
    fn floating_window_inside_stays_and_unrelated_releases_are_ignored() {
        let app = Rect::from_min_size(Pos2::ZERO, vec2(1000.0, 700.0));
        let w = Rect::from_min_size(pos2(100.0, 100.0), vec2(300.0, 200.0));
        assert_eq!(floating_verdict(w, app, None), FloatVerdict::Keep);
        // A release outside the app far from the window says nothing about it.
        assert_eq!(floating_verdict(w, app, Some(pos2(1200.0, 300.0))), FloatVerdict::Keep);
        assert_eq!(floating_verdict(Rect::NOTHING, app, Some(pos2(-5.0, 0.0))), FloatVerdict::Keep);
    }

    #[test]
    fn floating_window_slightly_out_is_pulled_back() {
        let app = Rect::from_min_size(Pos2::ZERO, vec2(1000.0, 700.0));
        let w = Rect::from_min_size(pos2(-40.0, 550.0), vec2(300.0, 200.0));
        assert!(outside_fraction(w, app) < DETACH_OUTSIDE_FRACTION);
        assert_eq!(floating_verdict(w, app, None), FloatVerdict::PullBack(pos2(0.0, 500.0)));
        let huge = Rect::from_min_size(pos2(-10.0, 0.0), vec2(1100.0, 200.0));
        assert_eq!(floating_verdict(huge, app, None), FloatVerdict::PullBack(pos2(0.0, 0.0)));
        assert!(floating_bounds(app).contains_rect(w.translate(vec2(2000.0, 0.0))));
    }

    #[test]
    fn gate_needs_to_be_armed_first() {
        let m = main_rect();
        let mut g = DockGate::default();
        // Spawned over the app (dragged out only a little): never docks while it stays there.
        for i in 0..10 {
            let w = window_at(pos2(500.0 + i as f32, 300.0));
            assert_eq!(g.update(w, m, i as f64), GateState::Idle);
        }
    }

    #[test]
    fn gate_docks_after_moving_over_app_and_resting() {
        let m = main_rect();
        let mut g = DockGate::default();
        assert_eq!(g.update(window_at(pos2(1500.0, 300.0)), m, 0.0), GateState::Idle);
        assert_eq!(g.update(window_at(pos2(900.0, 300.0)), m, 1.0), GateState::Hover);
        assert_eq!(g.update(window_at(pos2(800.0, 300.0)), m, 1.1), GateState::Hover);
        assert_eq!(g.update(window_at(pos2(800.0, 300.0)), m, 1.1 + DOCK_STILL_SECS / 2.0), GateState::Hover);
        assert_eq!(g.update(window_at(pos2(800.0, 300.0)), m, 1.1 + DOCK_STILL_SECS), GateState::Dock);
    }

    #[test]
    fn gate_resets_when_leaving_zone() {
        let m = main_rect();
        let mut g = DockGate::default();
        g.update(window_at(pos2(1500.0, 300.0)), m, 0.0);
        assert_eq!(g.update(window_at(pos2(900.0, 300.0)), m, 1.0), GateState::Hover);
        assert_eq!(g.update(window_at(pos2(1300.0, 300.0)), m, 1.1), GateState::Idle);
        assert_eq!(g.update(window_at(pos2(1300.0, 300.0)), m, 5.0), GateState::Idle);
    }

    #[test]
    fn gate_ignores_app_moving_under_a_still_window() {
        let mut g = DockGate::default();
        let w = window_at(pos2(1500.0, 300.0));
        g.update(w, main_rect(), 0.0);
        let moved_main = Rect::from_min_size(pos2(1200.0, 100.0), vec2(1000.0, 700.0));
        for i in 1..10 {
            assert_eq!(g.update(w, moved_main, i as f64), GateState::Idle);
        }
    }
}
