//! Ties the signals together: once per frame the app hands over its local terminals,
//! the hub reads what each one shows (title, bells, notifications, last output), merges
//! hook messages from the pipe and agent detection from the resource monitor, and runs
//! every session's [`Tracker`]. Tabs read the result through [`badge_for`].

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui;

use super::hook::IpcMessage;
use super::state::{Event, HookKind, Observation, Tracker, title_activity};
use super::{AgentKind, AgentState, ipc};
use crate::procmon::{self, Monitor, Usage};
use crate::ssh::terminal::{LinkState, TerminalHandle};

/// Two alerts for the same session closer than this are one.
const ALERT_GAP: Duration = Duration::from_secs(2);
/// Frame rate of the "working" pulse on badges.
const PULSE_FRAME: Duration = Duration::from_millis(100);
/// A hook-reported agent counts as gone only once a process sample this much newer
/// than the hook no longer shows it.
const HOOK_GRACE: Duration = Duration::from_secs(3);

/// A local terminal, as the app sees it this frame.
pub struct TermInfo {
    pub uid: u64,
    pub handle: TerminalHandle,
    /// The tab is the active one of its pane.
    pub visible: bool,
    /// The tab's title, for the panel.
    pub label: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Badge {
    pub kind: AgentKind,
    pub state: AgentState,
}

/// One row of the agents panel.
#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub uid: u64,
    pub kind: AgentKind,
    pub state: AgentState,
    pub since: Instant,
    pub message: Option<String>,
    /// Folder name of the session's working directory, else the tab title.
    pub project: String,
    pub cwd: Option<String>,
    pub hooked: bool,
    pub usage: Option<Usage>,
    pub visible: bool,
}

/// A session just started needing the user.
#[derive(Debug, Clone)]
pub struct Alert {
    pub uid: u64,
    pub kind: AgentKind,
    pub state: AgentState,
    pub project: String,
    pub message: Option<String>,
    /// The tab was on screen when it happened.
    pub visible: bool,
}

static BADGES: Mutex<Option<HashMap<u64, Badge>>> = Mutex::new(None);

/// The badge of tab `uid`, if an agent runs in it.
pub fn badge_for(uid: u64) -> Option<Badge> {
    BADGES.lock().ok().and_then(|b| b.as_ref().and_then(|m| m.get(&uid).copied()))
}

struct Session {
    tag: String,
    pid: Option<u32>,
    tracker: Tracker,
    kind: Option<AgentKind>,
    hook_kind: Option<AgentKind>,
    hook_at: Option<Instant>,
    bells: u64,
    notices: u64,
    cwd: Option<String>,
    label: String,
    visible: bool,
    ended: bool,
    last_alert: Option<Instant>,
}

impl Session {
    fn new(tag: String, bells: u64, notices: u64, now: Instant) -> Self {
        Self {
            tag,
            pid: None,
            tracker: Tracker::new(now),
            kind: None,
            hook_kind: None,
            hook_at: None,
            bells,
            notices,
            cwd: None,
            label: String::new(),
            visible: false,
            ended: false,
            last_alert: None,
        }
    }

    fn project(&self) -> String {
        self.cwd
            .as_deref()
            .map(|c| c.trim_end_matches(['/', '\\']))
            .and_then(|c| c.rsplit(['/', '\\']).next())
            .filter(|f| !f.is_empty())
            .map_or_else(|| self.label.clone(), str::to_string)
    }
}

/// What a terminal shows, read under one short lock.
struct Seen {
    title: String,
    cwd: Option<String>,
    bells: u64,
    notices: u64,
    notice: Option<String>,
    last_output: Option<Instant>,
    pid: Option<u32>,
    link: LinkState,
}

fn read(handle: &TerminalHandle) -> Option<Seen> {
    // Never wait on a busy reader: this terminal is simply read next frame.
    let s = handle.shared.try_lock().ok()?;
    let ev = s.parser.callbacks();
    Some(Seen {
        title: ev.title.clone().unwrap_or_default(),
        cwd: ev.cwd.clone(),
        bells: ev.bell_count,
        notices: ev.notice_count,
        notice: ev.notice.clone(),
        last_output: s.last_output,
        pid: s.pid,
        link: s.state.clone(),
    })
}

/// `Process exited with code 3` -> `Some(3)`.
fn exit_code(link: &LinkState) -> Option<u32> {
    match link {
        LinkState::Closed(reason) => reason.strip_prefix("Process exited with code ").and_then(|c| c.trim().parse().ok()),
        _ => None,
    }
}

pub struct AgentHub {
    sessions: HashMap<u64, Session>,
    inbox: ipc::Inbox,
    monitor: Arc<Monitor>,
}

impl AgentHub {
    /// Starts the hook pipe and the resource monitor.
    pub fn new(rt: &tokio::runtime::Handle, ctx: &egui::Context) -> Self {
        Self { sessions: HashMap::new(), inbox: ipc::start(rt, ctx), monitor: Arc::new(Monitor::start(ctx)) }
    }

    pub fn monitor(&self) -> &Monitor {
        &self.monitor
    }

    /// The user typed into tab `uid`.
    pub fn note_input(&mut self, uid: u64, now: Instant) {
        if let Some(s) = self.sessions.get_mut(&uid) {
            s.tracker.apply(Event::Input, now);
        }
    }

    /// Runs once per frame. Returns the sessions that just started needing attention.
    pub fn frame(&mut self, ctx: &egui::Context, terms: Vec<TermInfo>, now: Instant) -> Vec<Alert> {
        let mut alerts = Vec::new();
        self.drain_inbox(now, &mut alerts);
        let snap = procmon::latest();
        let mut watch = HashMap::new();
        let mut live = HashSet::new();
        for t in terms {
            live.insert(t.uid);
            let Some(seen) = read(&t.handle) else {
                // Busy this frame: keep watching what we knew.
                if let Some(pid) = self.sessions.get(&t.uid).and_then(|s| s.pid) {
                    watch.insert(t.uid, pid);
                }
                continue;
            };
            let pid = seen.pid.filter(|_| seen.link.is_live());
            if let Some(pid) = pid {
                watch.insert(t.uid, pid);
            }
            let fresh = || Session::new(t.handle.tag.clone(), seen.bells, seen.notices, now);
            let session = self.sessions.entry(t.uid).or_insert_with(fresh);
            if session.tag != t.handle.tag {
                // Respawned in place: a new shell, a new session.
                *session = fresh();
            }
            session.pid = pid;
            // The agent's own title ("✳ Fix the login bug") names the task best.
            let title = crate::ui::agent_badge::clean_title(&seen.title).trim();
            session.label = if title.is_empty() || title.eq_ignore_ascii_case("claude code") { t.label } else { title.to_string() };
            session.visible = t.visible;
            if seen.cwd.is_some() && session.hook_kind.is_none() {
                session.cwd.clone_from(&seen.cwd);
            }
            let sample = snap.as_ref().and_then(|s| s.per_tab.get(&t.uid).map(|u| (u.agent, s.at.unwrap_or(now))));
            update_kind(session, sample);
            let Some(kind) = session.kind else {
                session.bells = seen.bells;
                session.notices = seen.notices;
                continue;
            };
            let mut transitions = Vec::new();
            if seen.bells > session.bells {
                transitions.extend(session.tracker.apply(Event::Bell, now));
            }
            if seen.notices > session.notices
                && let Some(text) = seen.notice.clone()
            {
                transitions.extend(session.tracker.apply(Event::Notify(text), now));
            }
            session.bells = seen.bells;
            session.notices = seen.notices;
            if matches!(seen.link, LinkState::Closed(_) | LinkState::Failed(_)) && !session.ended {
                session.ended = true;
                transitions.extend(session.tracker.apply(Event::Exited(exit_code(&seen.link)), now));
            }
            let obs = Observation { title_busy: title_activity(Some(kind), &seen.title), last_output: seen.last_output };
            transitions.extend(session.tracker.observe(obs, now));
            for (_, to) in transitions {
                push_alert(&mut alerts, t.uid, session, to, now);
            }
        }
        self.sessions.retain(|uid, _| live.contains(uid));
        self.monitor.set_watch(watch);
        self.schedule(ctx, now);
        self.publish_badges();
        alerts
    }

    fn drain_inbox(&mut self, now: Instant, alerts: &mut Vec<Alert>) {
        for msg in self.inbox.drain() {
            let Some((&uid, session)) = self.sessions.iter_mut().find(|(_, s)| s.tag == msg.session) else { continue };
            if let Some(to) = apply_hook(session, &msg, now) {
                push_alert(alerts, uid, session, to, now);
            }
        }
    }

    fn schedule(&self, ctx: &egui::Context, now: Instant) {
        let recheck = self.sessions.values().filter(|s| s.kind.is_some()).filter_map(|s| s.tracker.recheck_in(now)).min();
        let pulsing = crate::settings::snapshot().agents.badges
            && crate::theme::anim_time(1.0) > 0.0
            && self.sessions.values().any(|s| s.kind.is_some() && s.tracker.state == AgentState::Working);
        let next = match (recheck, pulsing) {
            (Some(r), true) => Some(r.min(PULSE_FRAME)),
            (Some(r), false) => Some(r),
            (None, true) => Some(PULSE_FRAME),
            (None, false) => None,
        };
        if let Some(after) = next {
            ctx.request_repaint_after(after);
        }
    }

    fn publish_badges(&self) {
        let map = self.sessions.iter().filter_map(|(uid, s)| s.kind.map(|kind| (*uid, Badge { kind, state: s.tracker.state }))).collect();
        if let Ok(mut b) = BADGES.lock() {
            *b = Some(map);
        }
    }

    /// Every agent session, most urgent first.
    pub fn sessions(&self) -> Vec<SessionInfo> {
        let snap = procmon::latest();
        let mut out: Vec<SessionInfo> = self
            .sessions
            .iter()
            .filter_map(|(uid, s)| {
                Some(SessionInfo {
                    uid: *uid,
                    kind: s.kind?,
                    state: s.tracker.state,
                    since: s.tracker.since,
                    message: s.tracker.message.clone(),
                    project: s.project(),
                    cwd: s.cwd.clone(),
                    hooked: s.tracker.hooked,
                    usage: snap.as_ref().and_then(|p| p.per_tab.get(uid).cloned()),
                    visible: s.visible,
                })
            })
            .collect();
        out.sort_by_key(|s| (s.state.urgency(), s.since, s.uid));
        out
    }

    /// The next session needing attention after `current` (wrapping), most urgent first.
    pub fn next_attention(&self, current: Option<u64>) -> Option<u64> {
        let list: Vec<u64> = self.sessions().into_iter().filter(|s| s.state.needs_attention()).map(|s| s.uid).collect();
        let at = current.and_then(|c| list.iter().position(|u| *u == c));
        match at {
            Some(i) => list.get((i + 1) % list.len()).copied(),
            None => list.first().copied(),
        }
    }

    pub fn attention_count(&self) -> usize {
        self.sessions.values().filter(|s| s.kind.is_some() && s.tracker.state.needs_attention()).count()
    }
}

/// Picks the session's agent: what runs in its process tree, else what its hooks said
/// (until a newer process sample shows the agent gone).
fn update_kind(session: &mut Session, sample: Option<(Option<AgentKind>, Instant)>) {
    let gone_after_hook = matches!((sample, session.hook_at), (Some((None, at)), Some(h)) if at > h + HOOK_GRACE);
    if gone_after_hook {
        session.hook_kind = None;
        session.hook_at = None;
    }
    let kind = sample.and_then(|(k, _)| k).or(session.hook_kind);
    if kind.is_none() && session.kind.is_some() {
        // The agent quit: forget its state, the shell is just a shell again.
        let now = Instant::now();
        session.tracker = Tracker::new(now);
        session.cwd = None;
    }
    session.kind = kind;
}

fn apply_hook(session: &mut Session, msg: &IpcMessage, now: Instant) -> Option<AgentState> {
    let kind = HookKind::parse(&msg.event, msg.notification_type.as_deref(), msg.message.as_deref())?;
    if let Some(agent) = AgentKind::from_id(&msg.agent) {
        session.hook_kind = Some(agent);
        session.hook_at = Some(now);
        session.kind.get_or_insert(agent);
    }
    if msg.cwd.is_some() {
        session.cwd.clone_from(&msg.cwd);
    }
    let message = match kind {
        HookKind::SubagentStop => Some("Subagent finished".to_string()),
        _ => msg.message.clone(),
    };
    session.tracker.apply(Event::Hook { kind, message }, now).map(|(_, to)| to)
}

fn push_alert(alerts: &mut Vec<Alert>, uid: u64, session: &mut Session, to: AgentState, now: Instant) {
    let Some(kind) = session.kind else { return };
    if !to.needs_attention() || session.last_alert.is_some_and(|t| now.saturating_duration_since(t) < ALERT_GAP) {
        return;
    }
    session.last_alert = Some(now);
    alerts.push(Alert { uid, kind, state: to, project: session.project(), message: session.tracker.message.clone(), visible: session.visible });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(event: &str, extra: impl FnOnce(&mut IpcMessage)) -> IpcMessage {
        let mut m = IpcMessage {
            v: 1,
            session: "tag".into(),
            agent: "claude".into(),
            event: event.into(),
            notification_type: None,
            message: None,
            cwd: Some("D:/work/api-server".into()),
            agent_session: None,
            tool: None,
        };
        extra(&mut m);
        m
    }

    #[test]
    fn hook_messages_drive_a_session() {
        let now = Instant::now();
        let mut s = Session::new("tag".into(), 0, 0, now);
        assert_eq!(apply_hook(&mut s, &msg("SessionStart", |_| {}), now), None);
        assert_eq!(s.kind, Some(AgentKind::Claude));
        assert_eq!(s.project(), "api-server");
        assert_eq!(apply_hook(&mut s, &msg("UserPromptSubmit", |m| m.message = Some("add tests".into())), now), Some(AgentState::Working));
        let perm = msg("Notification", |m| {
            m.notification_type = Some("permission_prompt".into());
            m.message = Some("Claude needs your permission to use Bash".into());
        });
        assert_eq!(apply_hook(&mut s, &perm, now), Some(AgentState::NeedsPermission));
        assert_eq!(s.tracker.message.as_deref(), Some("Claude needs your permission to use Bash"));
        assert_eq!(apply_hook(&mut s, &msg("PreToolUse", |_| {}), now), None, "unknown events are ignored");
        let mut alerts = Vec::new();
        push_alert(&mut alerts, 1, &mut s, AgentState::NeedsPermission, now);
        push_alert(&mut alerts, 1, &mut s, AgentState::Done, now + Duration::from_millis(500));
        push_alert(&mut alerts, 1, &mut s, AgentState::Working, now + Duration::from_secs(5));
        assert_eq!(alerts.len(), 1, "rate limited, and work is not an alert");
    }

    #[test]
    fn agent_kind_follows_the_process_tree() {
        let t0 = Instant::now();
        let mut s = Session::new("tag".into(), 0, 0, t0);
        update_kind(&mut s, Some((Some(AgentKind::Codex), t0)));
        assert_eq!(s.kind, Some(AgentKind::Codex));
        s.tracker.apply(Event::Bell, t0);
        update_kind(&mut s, Some((None, t0)));
        assert_eq!(s.kind, None);
        assert_eq!(s.tracker.state, AgentState::Idle, "state is reset when the agent quits");
        // A hook seen before the next process sample keeps the session alive...
        apply_hook(&mut s, &msg("SessionStart", |_| {}), t0);
        update_kind(&mut s, Some((None, t0 + Duration::from_secs(1))));
        assert_eq!(s.kind, Some(AgentKind::Claude));
        // ...until a sample well after the hook shows no agent.
        update_kind(&mut s, Some((None, t0 + Duration::from_secs(10))));
        assert_eq!(s.kind, None);
    }

    #[test]
    fn exit_codes_parse() {
        assert_eq!(exit_code(&LinkState::Closed("Process exited with code 2".into())), Some(2));
        assert_eq!(exit_code(&LinkState::Closed("Closed".into())), None);
        assert_eq!(exit_code(&LinkState::Connected), None);
    }
}
