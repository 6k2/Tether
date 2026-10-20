//! Rebindable keyboard shortcuts. Defaults live here; the user's overrides are stored
//! in `settings.keybindings` (an action mapped to `None` is unbound).

use std::sync::atomic::{AtomicBool, Ordering};

use eframe::egui::{self, Key, Modifiers};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Action {
    ToggleSidebar,
    OpenSettings,
    KeyManager,
    NewHost,
    NewFolder,
    QuickConnect,
    SearchHosts,
    NewLocalTerminal,
    SplitRight,
    SplitDown,
    OpenSftp,
    ToggleBroadcast,
    CloseTab,
    NextTab,
    PrevTab,
    FontBigger,
    FontSmaller,
    FontReset,
    SftpBack,
    SftpForward,
    SftpUp,
    SftpRefresh,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    General,
    Sessions,
    Terminal,
    Sftp,
}

impl Group {
    pub const ALL: [Group; 4] = [Group::General, Group::Sessions, Group::Terminal, Group::Sftp];

    pub fn title(self) -> &'static str {
        match self {
            Group::General => "General",
            Group::Sessions => "Sessions & tabs",
            Group::Terminal => "Terminal",
            Group::Sftp => "SFTP",
        }
    }
}

impl Action {
    pub const ALL: [Action; 22] = [
        Action::ToggleSidebar,
        Action::OpenSettings,
        Action::KeyManager,
        Action::NewHost,
        Action::NewFolder,
        Action::QuickConnect,
        Action::SearchHosts,
        Action::NewLocalTerminal,
        Action::SplitRight,
        Action::SplitDown,
        Action::OpenSftp,
        Action::ToggleBroadcast,
        Action::CloseTab,
        Action::NextTab,
        Action::PrevTab,
        Action::FontBigger,
        Action::FontSmaller,
        Action::FontReset,
        Action::SftpBack,
        Action::SftpForward,
        Action::SftpUp,
        Action::SftpRefresh,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Action::ToggleSidebar => "Toggle sidebar",
            Action::OpenSettings => "Open settings",
            Action::KeyManager => "Open key manager",
            Action::NewHost => "New host",
            Action::NewFolder => "New folder",
            Action::QuickConnect => "Quick connect",
            Action::SearchHosts => "Search hosts",
            Action::NewLocalTerminal => "New local terminal (default shell)",
            Action::SplitRight => "New terminal, split right",
            Action::SplitDown => "New terminal, split down",
            Action::OpenSftp => "SFTP pane for the focused host",
            Action::ToggleBroadcast => "Toggle broadcast input",
            Action::CloseTab => "Close tab",
            Action::NextTab => "Next tab",
            Action::PrevTab => "Previous tab",
            Action::FontBigger => "Larger font",
            Action::FontSmaller => "Smaller font",
            Action::FontReset => "Reset font size",
            Action::SftpBack => "Back",
            Action::SftpForward => "Forward",
            Action::SftpUp => "Parent folder",
            Action::SftpRefresh => "Refresh listing",
        }
    }

    pub fn group(self) -> Group {
        match self {
            Action::ToggleSidebar
            | Action::OpenSettings
            | Action::KeyManager
            | Action::NewHost
            | Action::NewFolder
            | Action::QuickConnect
            | Action::SearchHosts => Group::General,
            Action::NewLocalTerminal
            | Action::SplitRight
            | Action::SplitDown
            | Action::OpenSftp
            | Action::ToggleBroadcast
            | Action::CloseTab
            | Action::NextTab
            | Action::PrevTab => Group::Sessions,
            Action::FontBigger | Action::FontSmaller | Action::FontReset => Group::Terminal,
            Action::SftpBack | Action::SftpForward | Action::SftpUp | Action::SftpRefresh => Group::Sftp,
        }
    }

    pub fn default_binding(self) -> Option<Binding> {
        let cs = |k| Some(Binding::new(k, true, true, false));
        let c = |k| Some(Binding::new(k, true, false, false));
        let a = |k| Some(Binding::new(k, false, false, true));
        match self {
            Action::ToggleSidebar => cs(Key::B),
            Action::OpenSettings => c(Key::Comma),
            Action::KeyManager => cs(Key::K),
            Action::NewHost => cs(Key::N),
            Action::NewFolder => None,
            Action::QuickConnect => cs(Key::L),
            Action::SearchHosts => cs(Key::P),
            Action::NewLocalTerminal => cs(Key::T),
            Action::SplitRight => cs(Key::D),
            Action::SplitDown => cs(Key::E),
            Action::OpenSftp => cs(Key::F),
            Action::ToggleBroadcast => cs(Key::I),
            Action::CloseTab => cs(Key::W),
            Action::NextTab => c(Key::Tab),
            Action::PrevTab => cs(Key::Tab),
            Action::FontBigger => c(Key::Equals),
            Action::FontSmaller => cs(Key::Minus),
            Action::FontReset => c(Key::Num0),
            Action::SftpBack => a(Key::ArrowLeft),
            Action::SftpForward => a(Key::ArrowRight),
            Action::SftpUp => a(Key::ArrowUp),
            Action::SftpRefresh => Some(Binding::new(Key::F5, false, false, false)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Binding {
    /// egui key name (`Key::name`), e.g. `B`, `Comma`, `F5`.
    pub key: String,
    #[serde(default)]
    pub ctrl: bool,
    #[serde(default)]
    pub shift: bool,
    #[serde(default)]
    pub alt: bool,
}

impl Binding {
    pub fn new(key: Key, ctrl: bool, shift: bool, alt: bool) -> Self {
        Self { key: key.name().to_string(), ctrl, shift, alt }
    }

    pub fn egui_key(&self) -> Option<Key> {
        Key::from_name(&self.key)
    }

    pub fn modifiers(&self) -> Modifiers {
        Modifiers { alt: self.alt, ctrl: self.ctrl, shift: self.shift, mac_cmd: false, command: self.ctrl }
    }

    /// Whether a key event is this binding (`=` and `+` count as the same key, so
    /// "Ctrl =" also fires on keyboards that report Ctrl + Plus).
    pub fn matches(&self, key: Key, m: Modifiers) -> bool {
        let Some(own) = self.egui_key() else { return false };
        let same_key = own == key || (matches!(own, Key::Equals | Key::Plus) && matches!(key, Key::Equals | Key::Plus));
        same_key && m.ctrl == self.ctrl && m.alt == self.alt && (m.shift == self.shift || (own == Key::Equals && key == Key::Plus))
    }

    /// Key caps for display: `["Ctrl", "Shift", "D"]`.
    pub fn caps(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.ctrl {
            out.push("Ctrl".to_string());
        }
        if self.alt {
            out.push("Alt".to_string());
        }
        if self.shift {
            out.push("Shift".to_string());
        }
        out.push(self.egui_key().map_or_else(|| self.key.clone(), key_label));
        out
    }

    pub fn label(&self) -> String {
        self.caps().join("+")
    }
}

/// Short printable name of a key.
pub fn key_label(key: Key) -> String {
    match key {
        Key::Comma => ",".into(),
        Key::Period => ".".into(),
        Key::Minus => "−".into(),
        Key::Equals => "=".into(),
        Key::Plus => "+".into(),
        Key::Slash => "/".into(),
        Key::Backslash => "\\".into(),
        Key::Semicolon => ";".into(),
        Key::Quote => "'".into(),
        Key::Backtick => "`".into(),
        Key::OpenBracket => "[".into(),
        Key::CloseBracket => "]".into(),
        Key::ArrowLeft => "←".into(),
        Key::ArrowRight => "→".into(),
        Key::ArrowUp => "↑".into(),
        Key::ArrowDown => "↓".into(),
        Key::PageUp => "PgUp".into(),
        Key::PageDown => "PgDn".into(),
        k => {
            let name = k.name();
            name.strip_prefix("Num").filter(|d| d.len() == 1).unwrap_or(name).to_string()
        }
    }
}

/// The effective binding of `action` (user override, else default).
pub fn binding(action: Action) -> Option<Binding> {
    crate::settings::get().keybindings.get(&action).cloned().unwrap_or_else(|| action.default_binding())
}

/// `Ctrl+Shift+D`, or an empty string when unbound.
pub fn label(action: Action) -> String {
    binding(action).map(|b| b.label()).unwrap_or_default()
}

/// "Tooltip text (Ctrl+Shift+B)".
pub fn hint(text: &str, action: Action) -> String {
    match binding(action) {
        Some(b) => format!("{text} ({})", b.label()),
        None => text.to_string(),
    }
}

static RECORDING: AtomicBool = AtomicBool::new(false);

/// While the Settings screen records a new binding, shortcuts must not fire.
pub fn set_recording(on: bool) {
    RECORDING.store(on, Ordering::Relaxed);
}

pub fn is_recording() -> bool {
    RECORDING.load(Ordering::Relaxed)
}

/// Consumes the key press of `action` if it happened this frame. Modifiers must match
/// exactly: egui's logical matching would let Ctrl+Tab swallow Ctrl+Shift+Tab.
pub fn consume(ctx: &egui::Context, action: Action) -> bool {
    if is_recording() {
        return false;
    }
    let Some(b) = binding(action) else { return false };
    ctx.input_mut(|i| {
        let hit = i.events.iter().position(|e| matches!(e, egui::Event::Key { key, pressed: true, modifiers, .. } if b.matches(*key, *modifiers)));
        if let Some(at) = hit {
            i.events.remove(at);
        }
        hit.is_some()
    })
}

/// Whether `action` was pressed this frame, without consuming it (for widgets that
/// only act while hovered, like the SFTP panes).
pub fn pressed_in(i: &egui::InputState, action: Action) -> bool {
    if is_recording() {
        return false;
    }
    let Some(b) = binding(action) else { return false };
    i.events.iter().any(|e| matches!(e, egui::Event::Key { key, pressed: true, modifiers, .. } if b.matches(*key, *modifiers)))
}

/// Other actions already using `binding`.
pub fn conflicts(action: Action, binding: &Binding) -> Vec<Action> {
    Action::ALL.into_iter().filter(|a| *a != action && self::binding(*a).as_ref() == Some(binding)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_action_has_metadata_and_unique_defaults() {
        let mut seen = std::collections::HashSet::new();
        for a in Action::ALL {
            assert!(!a.title().is_empty());
            if let Some(b) = a.default_binding() {
                assert!(b.egui_key().is_some(), "{a:?}");
                assert!(seen.insert(b.label()), "duplicate default {}", b.label());
            }
        }
        for g in Group::ALL {
            assert!(Action::ALL.iter().any(|a| a.group() == g));
        }
    }

    #[test]
    fn labels_and_matching() {
        let b = Binding::new(Key::D, true, true, false);
        assert_eq!(b.label(), "Ctrl+Shift+D");
        let cs = Modifiers { ctrl: true, shift: true, command: true, ..Modifiers::NONE };
        assert!(b.matches(Key::D, cs));
        assert!(!b.matches(Key::D, Modifiers::CTRL));
        let next = Binding::new(Key::Tab, true, false, false);
        assert!(!next.matches(Key::Tab, cs), "Ctrl+Shift+Tab is not Ctrl+Tab");
        let plus = Binding::new(Key::Equals, true, false, false);
        assert!(plus.matches(Key::Plus, Modifiers { ctrl: true, shift: true, ..Modifiers::NONE }));
        assert_eq!(Binding::new(Key::Num0, true, false, false).label(), "Ctrl+0");
        assert_eq!(Binding::new(Key::ArrowLeft, false, false, true).label(), "Alt+←");
    }

    #[test]
    fn bindings_round_trip_through_json() {
        let b = Binding::new(Key::F5, false, true, false);
        let back: Binding = serde_json::from_str(&serde_json::to_string(&b).unwrap()).unwrap();
        assert_eq!(back, b);
        let mut map = std::collections::BTreeMap::new();
        map.insert(Action::CloseTab, None::<Binding>);
        map.insert(Action::NextTab, Some(b.clone()));
        let text = serde_json::to_string(&map).unwrap();
        assert!(text.contains("CloseTab"));
        let back: std::collections::BTreeMap<Action, Option<Binding>> = serde_json::from_str(&text).unwrap();
        assert_eq!(back, map);
    }
}
