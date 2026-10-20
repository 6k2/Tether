//! Coalesced UI wake-ups for output producers (SSH reader, local PTY, ...).
//!
//! A busy TUI can deliver hundreds of small chunks per second. Asking egui for a repaint
//! on each one wakes the event loop that often for nothing; the throttle turns that into
//! at most one repaint per [`MIN_INTERVAL`] while the terminal is on screen, and a slow
//! trickle ([`HIDDEN_INTERVAL`], for tab titles/status dots) while it is not painted.
//! Hidden views share one schedule, so any number of background tabs wakes the UI at
//! most ~2 times a second in total.
//!
//! The handle's creator calls [`RepaintThrottle::attach`] (a view that was never drawn
//! still has to wake the UI: agent alerts, tab status), the view calls
//! [`RepaintThrottle::mark_painted`] whenever it draws, and the producer calls
//! [`RepaintThrottle::notify`] after it changes what the view shows.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use eframe::egui::{self, ViewportId};

/// Minimum spacing of repaints for a visible view (~120 Hz cap, below any display's frame time).
pub const MIN_INTERVAL: Duration = Duration::from_millis(8);
/// Repaint spacing for all views not being painted (hidden tabs, minimised window) together.
pub const HIDDEN_INTERVAL: Duration = Duration::from_millis(500);
/// A view painted within this many passes of its viewport counts as visible.
const VISIBLE_PASSES: u64 = 2;
const NONE: u64 = u64::MAX;

/// The one schedule every hidden view shares.
static HIDDEN: Slot = Slot::new();

/// A pending repaint: when it is due and at which UI pass it was requested.
struct Slot {
    /// Nanoseconds since [`epoch`] of the repaint already scheduled (`NONE` if none yet).
    at: AtomicU64,
    /// The root viewport's pass number when it was requested.
    pass: AtomicU64,
}

impl Slot {
    const fn new() -> Self {
        Self { at: AtomicU64::new(NONE), pass: AtomicU64::new(0) }
    }

    /// Decides whether a new repaint must be requested at `now` (UI at pass `pass`), and
    /// after what delay. `None` means the one already scheduled is still in the future
    /// and will show this output.
    fn schedule(&self, now: u64, pass: u64, interval: Duration) -> Option<Duration> {
        let prev = self.at.load(Ordering::Acquire);
        let interval = interval.as_nanos() as u64;
        let at = match prev {
            NONE => now,
            // Already pending soon enough: that repaint will show this output too...
            p if p > now && p - now <= interval => {
                if self.pass.load(Ordering::Acquire) == pass {
                    return None;
                }
                // ...unless the UI ran a pass since: eframe may have folded the delayed
                // request into it (an immediate repaint replaces the scheduled time) or
                // dropped it as outdated. Ask again for the same time.
                p
            }
            // Pending, but slower than allowed now (a hidden tab just became visible).
            p if p > now => now,
            // Keep repaints `interval` apart; output after a quiet spell repaints immediately.
            p => now.max(p.saturating_add(interval)),
        };
        self.at.store(at, Ordering::Release);
        self.pass.store(pass, Ordering::Release);
        Some(Duration::from_nanos(at - now))
    }
}

fn epoch() -> Instant {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    *EPOCH.get_or_init(Instant::now)
}

fn now_nanos() -> u64 {
    Instant::now().saturating_duration_since(epoch()).as_nanos() as u64
}

pub struct RepaintThrottle {
    /// The UI to wake: set by [`Self::attach`] at creation, or by the first paint.
    ctx: OnceLock<egui::Context>,
    /// Viewport and pass number of the latest paint (`None`: never drawn, i.e. hidden).
    painted: Mutex<Option<(ViewportId, u64)>>,
    /// Schedule while visible (hidden views use the shared [`HIDDEN`] one).
    visible: Slot,
}

impl Default for RepaintThrottle {
    fn default() -> Self {
        Self::new()
    }
}

impl RepaintThrottle {
    pub fn new() -> Self {
        Self { ctx: OnceLock::new(), painted: Mutex::new(None), visible: Slot::new() }
    }

    /// Creator side: the UI this producer wakes, before the view is ever drawn.
    pub fn attach(&self, ctx: &egui::Context) {
        let _ = self.ctx.set(ctx.clone());
    }

    /// UI side: the view was drawn this pass.
    pub fn mark_painted(&self, ctx: &egui::Context) {
        self.attach(ctx);
        let viewport = ctx.viewport_id();
        let pass = ctx.cumulative_pass_nr();
        if let Ok(mut p) = self.painted.lock() {
            *p = Some((viewport, pass));
        }
    }

    /// Whether the view was drawn in one of the latest passes of its viewport.
    pub fn is_visible(&self) -> bool {
        let Some(ctx) = self.ctx.get() else { return false };
        let Ok(painted) = self.painted.lock().map(|p| *p) else { return true };
        painted.is_some_and(|(viewport, pass)| pass + VISIBLE_PASSES >= ctx.cumulative_pass_nr_for(viewport))
    }

    /// Producer side: new output is ready. Cheap when a repaint is already pending.
    pub fn notify(&self) {
        let Some(ctx) = self.ctx.get() else { return };
        let (slot, interval) = if self.is_visible() { (&self.visible, MIN_INTERVAL) } else { (&HIDDEN, HIDDEN_INTERVAL) };
        // Detached windows are immediate viewports: they paint during the root's pass.
        let pass = ctx.cumulative_pass_nr_for(ViewportId::ROOT);
        if let Some(delay) = slot.schedule(now_nanos(), pass, interval) {
            ctx.request_repaint_after_for(delay, ViewportId::ROOT);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: u64 = 1_000_000;

    #[test]
    fn first_output_repaints_immediately_then_coalesces() {
        let t = Slot::new();
        assert_eq!(t.schedule(100 * MS, 1, MIN_INTERVAL), Some(Duration::ZERO));
        // Right after: the next repaint is pushed one interval later...
        assert_eq!(t.schedule(101 * MS, 2, MIN_INTERVAL), Some(Duration::from_millis(7)));
        // ...and everything before it rides along.
        assert_eq!(t.schedule(103 * MS, 2, MIN_INTERVAL), None);
        assert_eq!(t.schedule(107 * MS, 2, MIN_INTERVAL), None);
        // Once that repaint is due, a new one is scheduled.
        assert_eq!(t.schedule(110 * MS, 3, MIN_INTERVAL), Some(Duration::from_millis(6)));
    }

    #[test]
    fn quiet_spell_resets_the_spacing() {
        let t = Slot::new();
        assert!(t.schedule(0, 0, MIN_INTERVAL).is_some());
        assert_eq!(t.schedule(500 * MS, 0, MIN_INTERVAL), Some(Duration::ZERO));
    }

    #[test]
    fn hidden_views_repaint_slowly() {
        let t = Slot::new();
        assert_eq!(t.schedule(0, 0, HIDDEN_INTERVAL), Some(Duration::ZERO));
        assert_eq!(t.schedule(MS, 0, HIDDEN_INTERVAL), Some(Duration::from_millis(499)));
        assert_eq!(t.schedule(200 * MS, 0, HIDDEN_INTERVAL), None);
        // Becoming visible must not wait out the slow schedule.
        assert_eq!(t.schedule(201 * MS, 0, MIN_INTERVAL), Some(Duration::ZERO));
    }

    #[test]
    fn a_pass_in_between_re_requests_the_pending_repaint() {
        let t = Slot::new();
        assert_eq!(t.schedule(0, 5, HIDDEN_INTERVAL), Some(Duration::ZERO));
        assert_eq!(t.schedule(MS, 5, HIDDEN_INTERVAL), Some(Duration::from_millis(499)));
        assert_eq!(t.schedule(2 * MS, 5, HIDDEN_INTERVAL), None);
        // Input made the UI run a pass, which may have swallowed the delayed request:
        // the same deadline is requested again (not an immediate repaint).
        assert_eq!(t.schedule(100 * MS, 6, HIDDEN_INTERVAL), Some(Duration::from_millis(400)));
        assert_eq!(t.schedule(101 * MS, 6, HIDDEN_INTERVAL), None);
    }

    fn paint(ctx: &egui::Context, t: Option<&RepaintThrottle>) {
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            if let Some(t) = t {
                t.mark_painted(ui.ctx());
            }
        });
        out.textures_delta.clear();
    }

    #[test]
    fn visibility_follows_painting() {
        let t = RepaintThrottle::new();
        assert!(!t.is_visible(), "never painted");
        t.notify(); // no context yet: a no-op, not a panic
        let ctx = egui::Context::default();
        paint(&ctx, Some(&t));
        assert!(t.is_visible());
        for _ in 0..4 {
            paint(&ctx, None);
        }
        assert!(!t.is_visible(), "not drawn for several passes: hidden");
        paint(&ctx, Some(&t));
        assert!(t.is_visible());
    }

    #[test]
    fn attached_but_never_painted_views_are_hidden_and_still_wake_the_ui() {
        let ctx = egui::Context::default();
        let woken = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let w = woken.clone();
        ctx.set_request_repaint_callback(move |_| {
            w.fetch_add(1, Ordering::Relaxed);
        });
        // Past egui's start-up repaints, so a new request reaches the callback.
        for _ in 0..3 {
            paint(&ctx, None);
        }
        let t = RepaintThrottle::new();
        t.attach(&ctx);
        assert!(!t.is_visible(), "attached only: counts as hidden");
        let before = woken.load(Ordering::Relaxed);
        // Force a fresh hidden schedule so other tests sharing it can't swallow this one.
        HIDDEN.at.store(NONE, Ordering::Release);
        t.notify();
        assert!(woken.load(Ordering::Relaxed) > before, "a background tab's output must wake the UI");
    }
}
