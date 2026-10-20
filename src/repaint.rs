//! Coalesced UI wake-ups for output producers (SSH reader, local PTY, ...).
//!
//! A busy TUI can deliver hundreds of small chunks per second. Asking egui for a repaint
//! on each one wakes the event loop that often for nothing; the throttle turns that into
//! at most one repaint per [`MIN_INTERVAL`] while the terminal is on screen, and a slow
//! trickle ([`HIDDEN_INTERVAL`], for tab titles/status dots) while it is not painted.
//!
//! The view calls [`RepaintThrottle::mark_painted`] whenever it draws; the producer calls
//! [`RepaintThrottle::notify`] after it changes what the view shows.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use eframe::egui::{self, ViewportId};

/// Minimum spacing of repaints for a visible view (~120 Hz cap, below any display's frame time).
pub const MIN_INTERVAL: Duration = Duration::from_millis(8);
/// Repaint spacing while the view is not being painted (hidden tab, minimised window).
pub const HIDDEN_INTERVAL: Duration = Duration::from_millis(500);
/// A view painted within this many passes of its viewport counts as visible.
const VISIBLE_PASSES: u64 = 2;
const NONE: u64 = u64::MAX;

pub struct RepaintThrottle {
    epoch: Instant,
    /// Set by the first paint: the producer can only wake a UI that exists.
    ctx: OnceLock<egui::Context>,
    /// Viewport and pass number of the latest paint.
    painted: Mutex<(ViewportId, u64)>,
    /// Nanoseconds since `epoch` of the repaint already scheduled (`NONE` if none yet).
    scheduled_at: AtomicU64,
}

impl Default for RepaintThrottle {
    fn default() -> Self {
        Self::new()
    }
}

impl RepaintThrottle {
    pub fn new() -> Self {
        Self { epoch: Instant::now(), ctx: OnceLock::new(), painted: Mutex::new((ViewportId::ROOT, 0)), scheduled_at: AtomicU64::new(NONE) }
    }

    /// UI side: the view was drawn this pass.
    pub fn mark_painted(&self, ctx: &egui::Context) {
        let _ = self.ctx.set(ctx.clone());
        let viewport = ctx.viewport_id();
        let pass = ctx.cumulative_pass_nr();
        if let Ok(mut p) = self.painted.lock() {
            *p = (viewport, pass);
        }
    }

    /// Whether the view was drawn in one of the latest passes of its viewport.
    pub fn is_visible(&self) -> bool {
        let Some(ctx) = self.ctx.get() else { return false };
        let Ok((viewport, pass)) = self.painted.lock().map(|p| *p) else { return true };
        pass + VISIBLE_PASSES >= ctx.cumulative_pass_nr_for(viewport)
    }

    /// Producer side: new output is ready. Cheap when a repaint is already pending.
    pub fn notify(&self) {
        let Some(ctx) = self.ctx.get() else { return };
        let interval = if self.is_visible() { MIN_INTERVAL } else { HIDDEN_INTERVAL };
        let now = self.nanos(Instant::now());
        if let Some(delay) = self.schedule(now, interval) {
            ctx.request_repaint_after(delay);
        }
    }

    /// Decides whether a new repaint must be requested at `now`, and after what delay.
    /// `None` means the one already scheduled is still in the future and will show this output.
    fn schedule(&self, now: u64, interval: Duration) -> Option<Duration> {
        let prev = self.scheduled_at.load(Ordering::Acquire);
        let interval = interval.as_nanos() as u64;
        let at = match prev {
            NONE => now,
            // Already pending soon enough: that repaint will show this output too.
            p if p > now && p - now <= interval => return None,
            // Pending, but slower than allowed now (a hidden tab just became visible).
            p if p > now => now,
            // Keep repaints `interval` apart; output after a quiet spell repaints immediately.
            p => now.max(p.saturating_add(interval)),
        };
        self.scheduled_at.store(at, Ordering::Release);
        Some(Duration::from_nanos(at - now))
    }

    fn nanos(&self, t: Instant) -> u64 {
        t.saturating_duration_since(self.epoch).as_nanos() as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: u64 = 1_000_000;

    #[test]
    fn first_output_repaints_immediately_then_coalesces() {
        let t = RepaintThrottle::new();
        assert_eq!(t.schedule(100 * MS, MIN_INTERVAL), Some(Duration::ZERO));
        // Right after: the next repaint is pushed one interval later...
        assert_eq!(t.schedule(101 * MS, MIN_INTERVAL), Some(Duration::from_millis(7)));
        // ...and everything before it rides along.
        assert_eq!(t.schedule(103 * MS, MIN_INTERVAL), None);
        assert_eq!(t.schedule(107 * MS, MIN_INTERVAL), None);
        // Once that repaint is due, a new one is scheduled.
        assert_eq!(t.schedule(110 * MS, MIN_INTERVAL), Some(Duration::from_millis(6)));
    }

    #[test]
    fn quiet_spell_resets_the_spacing() {
        let t = RepaintThrottle::new();
        assert!(t.schedule(0, MIN_INTERVAL).is_some());
        assert_eq!(t.schedule(500 * MS, MIN_INTERVAL), Some(Duration::ZERO));
    }

    #[test]
    fn hidden_views_repaint_slowly() {
        let t = RepaintThrottle::new();
        assert_eq!(t.schedule(0, HIDDEN_INTERVAL), Some(Duration::ZERO));
        assert_eq!(t.schedule(MS, HIDDEN_INTERVAL), Some(Duration::from_millis(499)));
        assert_eq!(t.schedule(200 * MS, HIDDEN_INTERVAL), None);
        // Becoming visible must not wait out the slow schedule.
        assert_eq!(t.schedule(201 * MS, MIN_INTERVAL), Some(Duration::ZERO));
    }

    #[test]
    fn visibility_follows_painting() {
        let t = RepaintThrottle::new();
        assert!(!t.is_visible(), "never painted");
        t.notify(); // no context yet: a no-op, not a panic
        let ctx = egui::Context::default();
        let paint = |mark: bool| {
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
                if mark {
                    t.mark_painted(ui.ctx());
                }
            });
            out.textures_delta.clear();
        };
        paint(true);
        assert!(t.is_visible());
        for _ in 0..4 {
            paint(false);
        }
        assert!(!t.is_visible(), "not drawn for several passes: hidden");
        paint(true);
        assert!(t.is_visible());
    }
}
