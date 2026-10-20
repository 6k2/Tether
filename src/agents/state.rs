//! The per-session state machine. Pure: time is passed in, so every rule is unit tested.
//!
//! Explicit signals (hook events, OSC notifications, bells, the user typing) move the
//! state at once. Heuristics (window title spinners, output activity) only fill the gaps:
//! a session with hooks installed ignores output activity, since hooks say more.

use std::time::{Duration, Instant};

use super::AgentKind;

/// Output must keep flowing this long (with no typing) before it counts as work.
pub const BURST_MIN: Duration = Duration::from_millis(1000);
/// A gap longer than this ends an output burst.
pub const BURST_GAP: Duration = Duration::from_millis(1500);
/// Typing echoes output: activity this soon after a key press is the user's, not the agent's.
pub const INPUT_QUIET: Duration = Duration::from_millis(1000);
/// Silence after activity-detected work that marks the turn finished.
pub const QUIET_DONE: Duration = Duration::from_millis(3000);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AgentState {
    Idle,
    Working,
    NeedsInput,
    NeedsPermission,
    Done,
    Error,
}

impl AgentState {
    pub fn label(self) -> &'static str {
        match self {
            AgentState::Idle => "Idle",
            AgentState::Working => "Working",
            AgentState::NeedsInput => "Waiting for input",
            AgentState::NeedsPermission => "Needs permission",
            AgentState::Done => "Done",
            AgentState::Error => "Error",
        }
    }

    /// States the user should look at (notifications, Ctrl+Shift+A).
    pub fn needs_attention(self) -> bool {
        matches!(self, AgentState::NeedsInput | AgentState::NeedsPermission | AgentState::Done | AgentState::Error)
    }

    /// Sort key: most urgent first.
    pub fn urgency(self) -> u8 {
        match self {
            AgentState::NeedsPermission => 0,
            AgentState::NeedsInput => 1,
            AgentState::Error => 2,
            AgentState::Done => 3,
            AgentState::Working => 4,
            AgentState::Idle => 5,
        }
    }
}

/// What a hook (or Codex' `notify`) reported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookKind {
    SessionStart,
    PromptSubmit,
    /// Claude's Notification hook: a permission prompt, or idle waiting for input.
    Notification {
        permission: bool,
    },
    Stop,
    SubagentStop,
    SessionEnd,
    /// Codex `agent-turn-complete`.
    TurnComplete,
}

impl HookKind {
    /// Maps a hook event name (Claude's `hook_event_name`, our CLI argument, Codex' `type`).
    pub fn parse(event: &str, notification_type: Option<&str>, message: Option<&str>) -> Option<Self> {
        let e = event.to_ascii_lowercase().replace(['-', '_'], "");
        Some(match e.as_str() {
            "sessionstart" => HookKind::SessionStart,
            "userpromptsubmit" | "promptsubmit" => HookKind::PromptSubmit,
            "notification" => HookKind::Notification { permission: is_permission(notification_type, message) },
            "permissionrequest" => HookKind::Notification { permission: true },
            "stop" => HookKind::Stop,
            "subagentstop" => HookKind::SubagentStop,
            "sessionend" => HookKind::SessionEnd,
            "agentturncomplete" | "turncomplete" => HookKind::TurnComplete,
            _ => return None,
        })
    }
}

fn is_permission(notification_type: Option<&str>, message: Option<&str>) -> bool {
    if let Some(t) = notification_type {
        return t.contains("permission");
    }
    message.is_some_and(|m| {
        let m = m.to_ascii_lowercase();
        m.contains("permission") || m.contains("approve") || m.contains("allow ")
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Hook {
        kind: HookKind,
        message: Option<String>,
    },
    /// An OSC 9 / OSC 777 desktop notification printed by the program.
    Notify(String),
    Bell,
    /// The user typed into the terminal.
    Input,
    /// The shell ended; `Some(code)` when it reported one.
    Exited(Option<u32>),
}

/// What the hub sees of the terminal each frame.
#[derive(Debug, Clone, Copy, Default)]
pub struct Observation {
    /// From the window title: `Some(true)` = a busy spinner, `Some(false)` = the idle mark.
    pub title_busy: Option<bool>,
    /// When the terminal last printed anything.
    pub last_output: Option<Instant>,
}

/// `(from, to)` whenever the state changes.
pub type Transition = (AgentState, AgentState);

#[derive(Debug, Clone)]
pub struct Tracker {
    pub state: AgentState,
    /// When `state` was entered.
    pub since: Instant,
    /// Hook events have arrived for this session (activity heuristics are then off).
    pub hooked: bool,
    /// Last notification / prompt / message, for the agents panel.
    pub message: Option<String>,
    /// `message` came from terminal output (OSC 9/777/99), not from the agent's hooks.
    pub message_from_terminal: bool,
    last_input: Option<Instant>,
    burst_start: Option<Instant>,
    prev_output: Option<Instant>,
    prev_title_busy: Option<bool>,
    /// The current `Working` came from output activity (so silence can end it).
    by_activity: bool,
}

impl Tracker {
    pub fn new(now: Instant) -> Self {
        Self {
            state: AgentState::Idle,
            since: now,
            hooked: false,
            message: None,
            message_from_terminal: false,
            last_input: None,
            burst_start: None,
            prev_output: None,
            prev_title_busy: None,
            by_activity: false,
        }
    }

    fn set(&mut self, to: AgentState, now: Instant) -> Option<Transition> {
        self.by_activity = false;
        if self.state == to {
            return None;
        }
        let from = std::mem::replace(&mut self.state, to);
        self.since = now;
        Some((from, to))
    }

    pub fn apply(&mut self, event: Event, now: Instant) -> Option<Transition> {
        match event {
            Event::Input => {
                self.last_input = Some(now);
                // Typing answers whatever was asked: the attention is dealt with.
                if self.state.needs_attention() { self.set(AgentState::Idle, now) } else { None }
            }
            Event::Bell => match self.state {
                AgentState::Working | AgentState::Idle => self.set(AgentState::NeedsInput, now),
                _ => None,
            },
            Event::Notify(text) => {
                let lower = text.to_ascii_lowercase();
                let to = if is_permission(None, Some(&text)) {
                    AgentState::NeedsPermission
                } else if ["complete", "finished", "done"].iter().any(|w| lower.contains(w)) {
                    AgentState::Done
                } else {
                    AgentState::NeedsInput
                };
                self.message = Some(text);
                self.message_from_terminal = true;
                self.set(to, now)
            }
            Event::Hook { kind, message } => {
                self.hooked = !matches!(kind, HookKind::SessionEnd);
                if let Some(m) = message.filter(|m| !m.trim().is_empty()) {
                    self.message = Some(m);
                    self.message_from_terminal = false;
                }
                match kind {
                    HookKind::SessionStart | HookKind::SessionEnd => self.set(AgentState::Idle, now),
                    HookKind::PromptSubmit => self.set(AgentState::Working, now),
                    HookKind::Notification { permission: true } => self.set(AgentState::NeedsPermission, now),
                    HookKind::Notification { permission: false } => self.set(AgentState::NeedsInput, now),
                    HookKind::Stop | HookKind::TurnComplete => self.set(AgentState::Done, now),
                    HookKind::SubagentStop => None,
                }
            }
            Event::Exited(code) => match code {
                Some(0) => self.set(AgentState::Done, now),
                _ => self.set(AgentState::Error, now),
            },
        }
    }

    /// Applies the heuristics to what the terminal shows now.
    pub fn observe(&mut self, obs: Observation, now: Instant) -> Option<Transition> {
        if let Some(t) = obs.last_output
            && self.prev_output != Some(t)
        {
            if self.prev_output.is_none_or(|p| t.saturating_duration_since(p) > BURST_GAP) {
                self.burst_start = Some(t);
            }
            self.prev_output = Some(t);
        }
        let prev_busy = std::mem::replace(&mut self.prev_title_busy, obs.title_busy);
        match obs.title_busy {
            Some(true) if self.state == AgentState::Idle => return self.set(AgentState::Working, now),
            Some(false) if prev_busy == Some(true) && self.state == AgentState::Working => return self.set(AgentState::Done, now),
            _ => {}
        }
        // The title speaks for programs that set one; hooks speak for hooked sessions.
        if self.hooked || obs.title_busy.is_some() {
            return None;
        }
        match self.state {
            AgentState::Idle if self.output_is_work(now) => {
                let t = self.set(AgentState::Working, now);
                self.by_activity = true;
                t
            }
            AgentState::Working if self.by_activity && self.quiet_for(now) >= QUIET_DONE => self.set(AgentState::Done, now),
            _ => None,
        }
    }

    fn quiet_for(&self, now: Instant) -> Duration {
        self.prev_output.map_or(Duration::MAX, |t| now.saturating_duration_since(t))
    }

    fn output_is_work(&self, now: Instant) -> bool {
        let (Some(start), Some(last)) = (self.burst_start, self.prev_output) else { return false };
        let typing = self.last_input.is_some_and(|i| now.saturating_duration_since(i) < INPUT_QUIET || i >= start);
        !typing && self.quiet_for(now) < BURST_GAP && last.saturating_duration_since(start) >= BURST_MIN
    }

    /// How soon `observe` should run again even if nothing is printed (a silence can
    /// end a turn). `None` when only new output or events can change anything.
    pub fn recheck_in(&self, now: Instant) -> Option<Duration> {
        if self.hooked {
            return None;
        }
        match self.state {
            AgentState::Working if self.by_activity => Some(QUIET_DONE.saturating_sub(self.quiet_for(now)).max(Duration::from_millis(100))),
            AgentState::Idle if self.quiet_for(now) < BURST_GAP => Some(Duration::from_millis(250)),
            _ => None,
        }
    }
}

/// Reads a busy / idle hint from a window title. Claude Code prefixes its title with a
/// braille spinner frame while it works and with `✳` when it waits; other CLIs that
/// animate a braille spinner count as busy too.
pub fn title_activity(kind: Option<AgentKind>, title: &str) -> Option<bool> {
    let first = title.trim_start().chars().next()?;
    if ('\u{2801}'..='\u{28FF}').contains(&first) {
        return Some(true);
    }
    (kind == Some(AgentKind::Claude) && first == '✳').then_some(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    /// Streams output every 100 ms over `[from, to)` and observes each step.
    fn stream(t: &mut Tracker, start: Instant, from: u64, to: u64) -> Vec<Transition> {
        let mut out = Vec::new();
        let mut at = from;
        while at < to {
            let now = start + ms(at);
            out.extend(t.observe(Observation { title_busy: None, last_output: Some(now) }, now));
            at += 100;
        }
        out
    }

    fn quiet(t: &mut Tracker, start: Instant, at: u64) -> Option<Transition> {
        let last = t.prev_output;
        t.observe(Observation { title_busy: None, last_output: last }, start + ms(at))
    }

    #[test]
    fn sustained_output_is_work_and_silence_finishes_it() {
        let t0 = Instant::now();
        let mut t = Tracker::new(t0);
        let changes = stream(&mut t, t0, 0, 1500);
        assert_eq!(changes, vec![(AgentState::Idle, AgentState::Working)]);
        assert_eq!(quiet(&mut t, t0, 3000), None, "a short pause is still work");
        assert!(t.recheck_in(t0 + ms(3000)).is_some());
        assert_eq!(quiet(&mut t, t0, 4500), Some((AgentState::Working, AgentState::Done)));
        assert_eq!(t.recheck_in(t0 + ms(4500)), None);
    }

    #[test]
    fn short_bursts_and_typing_echo_are_not_work() {
        let t0 = Instant::now();
        let mut t = Tracker::new(t0);
        assert!(stream(&mut t, t0, 0, 600).is_empty(), "a redraw is not work");
        // The user types for two seconds: echo is not the agent working.
        let mut at = 5000;
        while at < 7000 {
            t.apply(Event::Input, t0 + ms(at));
            assert!(stream(&mut t, t0, at, at + 100).is_empty());
            at += 100;
        }
        assert_eq!(t.state, AgentState::Idle);
    }

    #[test]
    fn claude_title_spinner_drives_working_and_done() {
        let t0 = Instant::now();
        let mut t = Tracker::new(t0);
        let busy = |b| Observation { title_busy: Some(b), last_output: Some(t0) };
        assert_eq!(t.observe(busy(false), t0), None);
        assert_eq!(t.observe(busy(true), t0 + ms(10)), Some((AgentState::Idle, AgentState::Working)));
        assert_eq!(t.observe(busy(true), t0 + ms(20)), None);
        assert_eq!(t.observe(busy(false), t0 + ms(30)), Some((AgentState::Working, AgentState::Done)));
        assert_eq!(title_activity(Some(AgentKind::Claude), "⠐ Fix the build"), Some(true));
        assert_eq!(title_activity(Some(AgentKind::Claude), "✳ Fix the build"), Some(false));
        assert_eq!(title_activity(Some(AgentKind::Codex), "✳ x"), None);
        assert_eq!(title_activity(None, "pwsh"), None);
        assert_eq!(title_activity(None, ""), None);
    }

    #[test]
    fn hooks_are_authoritative() {
        let t0 = Instant::now();
        let mut t = Tracker::new(t0);
        let hook = |kind, message: Option<&str>| Event::Hook { kind, message: message.map(String::from) };
        assert_eq!(t.apply(hook(HookKind::SessionStart, None), t0), None);
        assert!(t.hooked);
        assert_eq!(t.apply(hook(HookKind::PromptSubmit, Some("fix it")), t0), Some((AgentState::Idle, AgentState::Working)));
        assert_eq!(t.message.as_deref(), Some("fix it"));
        // Activity heuristics are off once hooked: silence does not end the turn.
        stream(&mut t, t0, 0, 1500);
        assert_eq!(quiet(&mut t, t0, 9000), None);
        let perm = hook(HookKind::Notification { permission: true }, Some("Claude needs your permission to use Bash"));
        assert_eq!(t.apply(perm, t0 + ms(9000)), Some((AgentState::Working, AgentState::NeedsPermission)));
        assert_eq!(t.since, t0 + ms(9000));
        assert_eq!(t.apply(Event::Input, t0 + ms(9500)), Some((AgentState::NeedsPermission, AgentState::Idle)));
        assert_eq!(t.apply(hook(HookKind::SubagentStop, None), t0 + ms(9600)), None);
        assert_eq!(t.apply(hook(HookKind::Stop, None), t0 + ms(9700)), Some((AgentState::Idle, AgentState::Done)));
        assert_eq!(t.apply(hook(HookKind::SessionEnd, None), t0 + ms(9800)), Some((AgentState::Done, AgentState::Idle)));
        assert!(!t.hooked);
    }

    #[test]
    fn bells_notifications_and_exits() {
        let t0 = Instant::now();
        let mut t = Tracker::new(t0);
        assert_eq!(t.apply(Event::Bell, t0), Some((AgentState::Idle, AgentState::NeedsInput)));
        assert_eq!(t.apply(Event::Bell, t0), None);
        assert_eq!(t.apply(Event::Notify("Claude needs your permission to use Edit".into()), t0), Some((AgentState::NeedsInput, AgentState::NeedsPermission)));
        assert_eq!(t.apply(Event::Notify("Task complete".into()), t0), Some((AgentState::NeedsPermission, AgentState::Done)));
        assert_eq!(t.apply(Event::Notify("Claude is waiting for your input".into()), t0), Some((AgentState::Done, AgentState::NeedsInput)));
        assert_eq!(t.apply(Event::Exited(Some(1)), t0), Some((AgentState::NeedsInput, AgentState::Error)));
        assert_eq!(t.apply(Event::Exited(Some(0)), t0), Some((AgentState::Error, AgentState::Done)));
    }

    #[test]
    fn hook_names_parse() {
        assert_eq!(HookKind::parse("UserPromptSubmit", None, None), Some(HookKind::PromptSubmit));
        assert_eq!(HookKind::parse("Notification", Some("permission_prompt"), None), Some(HookKind::Notification { permission: true }));
        assert_eq!(HookKind::parse("Notification", Some("idle_prompt"), Some("permission")), Some(HookKind::Notification { permission: false }));
        assert_eq!(HookKind::parse("Notification", None, Some("Claude needs your permission")), Some(HookKind::Notification { permission: true }));
        assert_eq!(HookKind::parse("agent-turn-complete", None, None), Some(HookKind::TurnComplete));
        assert_eq!(HookKind::parse("PreToolUse", None, None), None);
    }

    #[test]
    fn urgency_orders_attention_first() {
        let mut v = [AgentState::Idle, AgentState::Done, AgentState::Working, AgentState::NeedsPermission, AgentState::NeedsInput];
        v.sort_by_key(|s| s.urgency());
        assert_eq!(v[0], AgentState::NeedsPermission);
        assert_eq!(v[4], AgentState::Idle);
        assert!(AgentState::Done.needs_attention() && !AgentState::Working.needs_attention());
    }
}
