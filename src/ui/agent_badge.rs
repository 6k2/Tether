//! The status mark of an agent session: in tab titles (a glyph in the title's text) and
//! in the agents panel (painted). Pulses while the agent works.

use eframe::egui::{self, Color32};

use crate::agents::{AgentState, Badge, hub};
use crate::theme;

/// Seconds per pulse while working.
const PULSE_PERIOD: f64 = 1.4;

pub fn color(state: AgentState) -> Color32 {
    match state {
        AgentState::Idle => theme::text_faint(),
        AgentState::Working => theme::accent(),
        AgentState::NeedsInput | AgentState::NeedsPermission => theme::warn(),
        AgentState::Done => theme::success(),
        AgentState::Error => theme::danger(),
    }
}

pub fn glyph(state: AgentState) -> &'static str {
    match state {
        // Glyphs the bundled fonts have (`●`, `▲` are missing): dots differ by colour.
        AgentState::Idle | AgentState::Working | AgentState::NeedsInput => "•",
        AgentState::NeedsPermission => "⚠",
        AgentState::Done => "✔",
        AgentState::Error => "✖",
    }
}

/// 0.45..=1.0 brightness of the working pulse at `time` (1.0 when animations are off).
pub fn pulse(time: f64) -> f32 {
    if theme::anim_time(1.0) <= 0.0 {
        return 1.0;
    }
    let phase = (time / PULSE_PERIOD * std::f64::consts::TAU).sin() * 0.5 + 0.5;
    0.45 + 0.55 * phase as f32
}

/// The badge of tab `uid` when badges are on and an agent runs there.
pub fn for_tab(uid: u64) -> Option<Badge> {
    if !crate::settings::snapshot().agents.badges {
        return None;
    }
    hub::badge_for(uid)
}

/// Glyph and colour for the start of a tab title: the agent badge, else `fallback`
/// (the tab's connection dot).
pub fn title_mark(uid: u64, fallback: Color32, time: f64) -> (&'static str, Color32, f32) {
    match for_tab(uid) {
        Some(b) => {
            let mut c = color(b.state);
            if b.state == AgentState::Working {
                c = c.gamma_multiply(pulse(time));
            }
            let size = if glyph(b.state) == "•" { 16.0 } else { 12.0 };
            (glyph(b.state), c, size)
        }
        None => ("•", fallback, 16.0),
    }
}

/// Appends the tab-title mark (with its trailing space) to `job`.
pub fn append_title_mark(job: &mut egui::text::LayoutJob, uid: u64, fallback: Color32, time: f64) {
    let (glyph, colour, size) = title_mark(uid, fallback, time);
    egui::RichText::new(format!("{glyph} ")).color(colour).size(size).append_to(
        job,
        &egui::Style::default(),
        egui::FontSelection::Default,
        egui::Align::Center,
    );
}

/// Paints a round badge (panel rows).
pub fn paint(painter: &egui::Painter, center: egui::Pos2, radius: f32, state: AgentState, time: f64) {
    let c = color(state);
    match state {
        AgentState::Working => {
            let k = pulse(time);
            painter.circle_filled(center, radius * (1.0 + 0.6 * (1.0 - k)), c.gamma_multiply(0.25 * k));
            painter.circle_filled(center, radius, c);
        }
        AgentState::Idle => {
            painter.circle_stroke(center, radius - 0.5, egui::Stroke::new(1.2, c));
        }
        _ => {
            painter.circle_filled(center, radius, c);
            painter.text(center, egui::Align2::CENTER_CENTER, glyph_inner(state), egui::FontId::proportional(radius * 1.3), theme::bg());
        }
    }
}

fn glyph_inner(state: AgentState) -> &'static str {
    match state {
        AgentState::NeedsPermission => "!",
        AgentState::NeedsInput => "?",
        AgentState::Done => "✔",
        AgentState::Error => "✖",
        _ => "",
    }
}

/// Drops a leading spinner frame (braille) or Claude's `✳` from a window title: the
/// badge shows the state, and the fonts have no braille glyphs.
pub fn clean_title(title: &str) -> &str {
    let t = title.trim_start();
    match t.chars().next() {
        Some(c) if ('\u{2800}'..='\u{28FF}').contains(&c) || c == '✳' => t[c.len_utf8()..].trim_start(),
        _ => title,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_state_has_a_distinct_look() {
        let states = [AgentState::Idle, AgentState::Working, AgentState::NeedsInput, AgentState::NeedsPermission, AgentState::Done, AgentState::Error];
        for s in states {
            assert!(!glyph(s).is_empty());
        }
        assert_eq!(clean_title("\u{2810} Fix the build"), "Fix the build");
        assert_eq!(clean_title("✳ Claude Code"), "Claude Code");
        assert_eq!(clean_title("pwsh"), "pwsh");
        let p = pulse(0.3);
        assert!((0.45..=1.0).contains(&p));
    }
}
