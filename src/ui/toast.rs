//! Bottom-right notifications that slide and fade in, then expire.

use std::sync::{Arc, Mutex};

use eframe::egui::{self, Color32, CornerRadius, FontId, Id, Order, Rect, Stroke, Vec2};

use crate::theme;

const LIFETIME: f64 = 5.0;
const FADE: f64 = 0.25;
const WIDTH: f32 = 340.0;

#[derive(Clone, Copy, PartialEq)]
pub enum Kind {
    Info,
    Success,
    Error,
}

struct Toast {
    id: u64,
    kind: Kind,
    text: String,
    born: Option<f64>,
}

/// Thread-safe so background tasks can post notices.
#[derive(Clone, Default)]
pub struct Toasts {
    inner: Arc<Mutex<(u64, Vec<Toast>)>>,
}

impl Toasts {
    pub fn push(&self, kind: Kind, text: impl Into<String>) {
        if let Ok(mut g) = self.inner.lock() {
            g.0 += 1;
            let id = g.0;
            g.1.push(Toast { id, kind, text: text.into(), born: None });
        }
    }

    pub fn info(&self, t: impl Into<String>) {
        self.push(Kind::Info, t);
    }
    pub fn success(&self, t: impl Into<String>) {
        self.push(Kind::Success, t);
    }
    pub fn error(&self, t: impl Into<String>) {
        self.push(Kind::Error, t);
    }

    pub fn show(&self, ctx: &egui::Context) {
        let Ok(mut g) = self.inner.lock() else { return };
        let now = ctx.input(|i| i.time);
        g.1.retain(|t| t.born.is_none_or(|b| now - b < LIFETIME));
        let screen = ctx.content_rect();
        let mut y = screen.bottom() - 16.0;
        for toast in g.1.iter_mut().rev() {
            let born = *toast.born.get_or_insert(now);
            let age = now - born;
            let appear = (age / FADE).clamp(0.0, 1.0) as f32;
            let vanish = ((LIFETIME - age) / FADE).clamp(0.0, 1.0) as f32;
            let t = ease_out(appear.min(vanish));
            let color = match toast.kind {
                Kind::Info => theme::blue(),
                Kind::Success => theme::accent(),
                Kind::Error => theme::danger(),
            };
            let painter = ctx.layer_painter(egui::LayerId::new(Order::Foreground, Id::new(("toast", toast.id))));
            let galley = painter.layout(toast.text.clone(), FontId::proportional(12.5), theme::text().gamma_multiply(t), WIDTH - 40.0);
            let h = galley.size().y + 22.0;
            let x = screen.right() - 16.0 - WIDTH + (1.0 - t) * 40.0;
            let rect = Rect::from_min_size(egui::pos2(x, y - h), Vec2::new(WIDTH, h));
            painter.rect(
                rect,
                CornerRadius::same(10),
                theme::surface_2().gamma_multiply(t),
                Stroke::new(1.0, theme::border().gamma_multiply(t)),
                egui::StrokeKind::Inside,
            );
            let bar = Rect::from_min_size(rect.min + Vec2::new(10.0, 11.0), Vec2::new(3.0, h - 22.0));
            painter.rect_filled(bar, CornerRadius::same(2), color.gamma_multiply(t));
            painter.galley(rect.min + Vec2::new(22.0, 11.0), galley, Color32::WHITE);
            y -= (h + 8.0) * t.max(0.2);
        }
        if !g.1.is_empty() {
            ctx.request_repaint();
        }
    }
}

fn ease_out(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(3)
}
