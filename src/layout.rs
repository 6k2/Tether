//! Window arrangement settings: where tabs and the sidebar go, which sidebar sections
//! show in what order, and pane behaviour (dimming, focus on hover).
//!
//! Stored under `settings.layout`; every field has a serde default so settings files
//! written before this existed keep loading.

use eframe::egui;
use serde::{Deserialize, Serialize};

/// Where the tabs of the workspace are listed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum TabPosition {
    /// A tab strip on top of every pane (egui_dock's own tab bars).
    #[default]
    Top,
    /// A vertical "Sessions" panel on the left of the panes.
    Left,
    /// A vertical "Sessions" panel on the right of the panes.
    Right,
}

impl TabPosition {
    pub const ALL: [TabPosition; 3] = [TabPosition::Top, TabPosition::Left, TabPosition::Right];

    pub fn is_vertical(self) -> bool {
        self != TabPosition::Top
    }

    pub fn label(self) -> &'static str {
        match self {
            TabPosition::Top => "Top",
            TabPosition::Left => "Left",
            TabPosition::Right => "Right",
        }
    }

    /// The next position (for "cycle tab position").
    pub fn next(self) -> Self {
        match self {
            TabPosition::Top => TabPosition::Left,
            TabPosition::Left => TabPosition::Right,
            TabPosition::Right => TabPosition::Top,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Side {
    #[default]
    Left,
    Right,
}

impl Side {
    pub fn label(self) -> &'static str {
        match self {
            Side::Left => "Left",
            Side::Right => "Right",
        }
    }

    pub fn flip(self) -> Self {
        match self {
            Side::Left => Side::Right,
            Side::Right => Side::Left,
        }
    }
}

/// A block of the sidebar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SidebarSection {
    Hosts,
    /// Every open tab (shown in the sidebar while tabs sit on top of the panes).
    Sessions,
    Workspaces,
    /// Reserved for AI agents running in terminals.
    Agents,
}

impl SidebarSection {
    pub const ALL: [SidebarSection; 4] = [SidebarSection::Hosts, SidebarSection::Sessions, SidebarSection::Workspaces, SidebarSection::Agents];

    pub fn title(self) -> &'static str {
        match self {
            SidebarSection::Hosts => "Hosts",
            SidebarSection::Sessions => "Sessions",
            SidebarSection::Workspaces => "Workspaces",
            SidebarSection::Agents => "Agents",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SectionEntry {
    pub kind: SidebarSection,
    #[serde(default)]
    pub collapsed: bool,
}

impl SectionEntry {
    pub const fn new(kind: SidebarSection) -> Self {
        Self { kind, collapsed: false }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LayoutSettings {
    pub tab_position: TabPosition,
    /// Vertical tabs: group rows under a header per pane (split).
    pub group_by_pane: bool,
    /// Vertical tabs: one line per tab instead of title + subtitle.
    pub compact_tabs: bool,
    /// Width of the vertical tabs panel.
    pub tabs_width: f32,
    pub sidebar_side: Side,
    pub sidebar_visible: bool,
    pub sidebar_width: f32,
    /// A thin icon strip that switches the sidebar between its sections (one at a time).
    pub activity_bar: bool,
    /// The section shown while the activity bar is on.
    pub active_view: SidebarSection,
    /// Sidebar sections, top to bottom. Read through [`LayoutSettings::sections`].
    pub sections: Vec<SectionEntry>,
    /// Fade panes that don't have focus.
    pub dim_inactive_panes: bool,
    /// 0..=1, how much inactive panes are faded.
    pub dim_amount: f32,
    /// Hovering a pane focuses it.
    pub focus_follows_mouse: bool,
}

impl LayoutSettings {
    pub const SIDEBAR_MIN: f32 = 200.0;
    pub const SIDEBAR_MAX: f32 = 460.0;
    pub const TABS_MIN: f32 = 170.0;
    pub const TABS_MAX: f32 = 420.0;
    pub const DIM_MAX: f32 = 0.8;

    pub fn sidebar_width(&self) -> f32 {
        sane(self.sidebar_width, Self::SIDEBAR_MIN, Self::SIDEBAR_MAX, 264.0)
    }

    pub fn tabs_width(&self) -> f32 {
        sane(self.tabs_width, Self::TABS_MIN, Self::TABS_MAX, 232.0)
    }

    pub fn dim_amount(&self) -> f32 {
        sane(self.dim_amount, 0.0, Self::DIM_MAX, 0.35)
    }

    /// The sections in display order: every kind exactly once (unknown duplicates
    /// dropped, missing ones appended), so a hand-edited file can't lose a section.
    pub fn sections(&self) -> Vec<SectionEntry> {
        normalize_sections(&self.sections)
    }

    pub fn is_collapsed(&self, kind: SidebarSection) -> bool {
        self.sections().iter().any(|e| e.kind == kind && e.collapsed)
    }

    pub fn set_collapsed(&mut self, kind: SidebarSection, collapsed: bool) {
        let mut list = self.sections();
        for e in &mut list {
            if e.kind == kind {
                e.collapsed = collapsed;
            }
        }
        self.sections = list;
    }

    /// Moves the section at `from` so it lands before the one now at `to`
    /// (`to == len` moves it to the end).
    pub fn move_section(&mut self, from: usize, to: usize) {
        let mut list = self.sections();
        move_item(&mut list, from, to);
        self.sections = list;
    }

    /// Whether the sidebar lists a "Sessions" section: only while tabs are on top,
    /// since vertical tabs already have a panel of their own.
    pub fn sessions_in_sidebar(&self) -> bool {
        !self.tab_position.is_vertical()
    }
}

impl Default for LayoutSettings {
    fn default() -> Self {
        Self {
            tab_position: TabPosition::Top,
            group_by_pane: true,
            compact_tabs: false,
            tabs_width: 232.0,
            sidebar_side: Side::Left,
            sidebar_visible: true,
            sidebar_width: 264.0,
            activity_bar: false,
            active_view: SidebarSection::Hosts,
            sections: SidebarSection::ALL.iter().map(|k| SectionEntry::new(*k)).collect(),
            dim_inactive_panes: false,
            dim_amount: 0.35,
            focus_follows_mouse: false,
        }
    }
}

pub fn normalize_sections(list: &[SectionEntry]) -> Vec<SectionEntry> {
    let mut out: Vec<SectionEntry> = Vec::with_capacity(SidebarSection::ALL.len());
    for e in list {
        if !out.iter().any(|o| o.kind == e.kind) {
            out.push(*e);
        }
    }
    for k in SidebarSection::ALL {
        if !out.iter().any(|o| o.kind == k) {
            out.push(SectionEntry::new(k));
        }
    }
    out
}

/// Moves `v[from]` so it ends up just before the element currently at `to`
/// (`to == v.len()` = the end). Out-of-range `from` is ignored; `to` is clamped.
pub fn move_item<T>(v: &mut Vec<T>, from: usize, to: usize) {
    if from >= v.len() {
        return;
    }
    let to = to.min(v.len());
    if to == from || to == from + 1 {
        return;
    }
    let item = v.remove(from);
    let at = if to > from { to - 1 } else { to };
    v.insert(at, item);
}

/// Insertion index for a drag at `y` over rows whose vertical centres are `centres`
/// (top to bottom): the first row whose centre lies below the pointer.
pub fn insertion_index(centres: &[f32], y: f32) -> usize {
    centres.iter().position(|c| y < *c).unwrap_or(centres.len())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

/// The pane to move focus to from `from` in `dir`: among the rects lying that way,
/// prefer ones that overlap `from` across the axis, then the nearest edge, then the
/// smallest offset of centres. Returns an index into `others`.
pub fn neighbor(from: egui::Rect, others: &[egui::Rect], dir: Direction) -> Option<usize> {
    const SLACK: f32 = 2.0;
    let c = from.center();
    others
        .iter()
        .enumerate()
        .filter_map(|(i, r)| {
            let (gap, overlap, offset) = match dir {
                Direction::Left if r.center().x < c.x && r.right() <= from.left() + SLACK => {
                    (from.left() - r.right(), r.y_range().intersects(from.y_range()), (r.center().y - c.y).abs())
                }
                Direction::Right if r.center().x > c.x && r.left() >= from.right() - SLACK => {
                    (r.left() - from.right(), r.y_range().intersects(from.y_range()), (r.center().y - c.y).abs())
                }
                Direction::Up if r.center().y < c.y && r.bottom() <= from.top() + SLACK => {
                    (from.top() - r.bottom(), r.x_range().intersects(from.x_range()), (r.center().x - c.x).abs())
                }
                Direction::Down if r.center().y > c.y && r.top() >= from.bottom() - SLACK => {
                    (r.top() - from.bottom(), r.x_range().intersects(from.x_range()), (r.center().x - c.x).abs())
                }
                _ => return None,
            };
            Some((i, !overlap, gap.max(0.0), offset))
        })
        .min_by(|a, b| (a.1, a.2, a.3).partial_cmp(&(b.1, b.2, b.3)).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(i, ..)| i)
}

fn sane(v: f32, min: f32, max: f32, default: f32) -> f32 {
    if v.is_finite() { v.clamp(min, max) } else { default }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(l: &LayoutSettings) -> Vec<SidebarSection> {
        l.sections().iter().map(|e| e.kind).collect()
    }

    #[test]
    fn move_item_inserts_before_target() {
        let mut v = vec!['a', 'b', 'c', 'd'];
        move_item(&mut v, 0, 2);
        assert_eq!(v, vec!['b', 'a', 'c', 'd']);
        move_item(&mut v, 3, 0);
        assert_eq!(v, vec!['d', 'b', 'a', 'c']);
        move_item(&mut v, 1, 4);
        assert_eq!(v, vec!['d', 'a', 'c', 'b']);
        // No-ops: onto itself, just after itself, out of range.
        move_item(&mut v, 1, 1);
        move_item(&mut v, 1, 2);
        move_item(&mut v, 9, 0);
        assert_eq!(v, vec!['d', 'a', 'c', 'b']);
        move_item(&mut v, 0, 99);
        assert_eq!(v, vec!['a', 'c', 'b', 'd']);
    }

    #[test]
    fn sections_reorder_and_collapse() {
        let mut l = LayoutSettings::default();
        assert_eq!(kinds(&l), SidebarSection::ALL.to_vec());
        l.move_section(2, 0);
        assert_eq!(kinds(&l), vec![SidebarSection::Workspaces, SidebarSection::Hosts, SidebarSection::Sessions, SidebarSection::Agents]);
        l.set_collapsed(SidebarSection::Hosts, true);
        assert!(l.is_collapsed(SidebarSection::Hosts));
        assert!(!l.is_collapsed(SidebarSection::Agents));
        let back: LayoutSettings = serde_json::from_str(&serde_json::to_string(&l).unwrap()).unwrap();
        assert_eq!(back, l);
    }

    #[test]
    fn hand_edited_sections_are_repaired() {
        let l: LayoutSettings = serde_json::from_str(r#"{"sections":[{"kind":"Workspaces"},{"kind":"Workspaces","collapsed":true}]}"#).unwrap();
        assert_eq!(kinds(&l), vec![SidebarSection::Workspaces, SidebarSection::Hosts, SidebarSection::Sessions, SidebarSection::Agents]);
        assert!(!l.is_collapsed(SidebarSection::Workspaces), "the first entry wins");
    }

    #[test]
    fn insertion_index_follows_row_centres() {
        let c = [10.0, 30.0, 50.0];
        assert_eq!(insertion_index(&c, 0.0), 0);
        assert_eq!(insertion_index(&c, 20.0), 1);
        assert_eq!(insertion_index(&c, 49.0), 2);
        assert_eq!(insertion_index(&c, 80.0), 3);
        assert_eq!(insertion_index(&[], 5.0), 0);
    }

    #[test]
    fn widths_and_dim_are_clamped() {
        let l = LayoutSettings { sidebar_width: 9999.0, tabs_width: f32::NAN, dim_amount: -1.0, ..Default::default() };
        assert_eq!(l.sidebar_width(), LayoutSettings::SIDEBAR_MAX);
        assert_eq!(l.tabs_width(), 232.0);
        assert_eq!(l.dim_amount(), 0.0);
    }

    #[test]
    fn old_settings_without_layout_load_with_defaults() {
        let old = r#"{"appearance":{"theme":"Nord"},"terminal":{"font_size":15.0},"keybindings":{"CloseTab":null}}"#;
        let s: crate::settings::Settings = serde_json::from_str(old).unwrap();
        assert_eq!(s.layout, LayoutSettings::default());
        assert_eq!(s.appearance.theme, "Nord");
        let partial: crate::settings::Settings = serde_json::from_str(r#"{"layout":{"tab_position":"Right","dim_inactive_panes":true}}"#).unwrap();
        assert_eq!(partial.layout.tab_position, TabPosition::Right);
        assert!(partial.layout.dim_inactive_panes && partial.layout.sidebar_visible);
        assert_eq!(partial.layout.sections().len(), SidebarSection::ALL.len());
    }

    #[test]
    fn neighbors_follow_the_split_tree_geometry() {
        use egui::{Rect, pos2};
        // Left half | right half split top / bottom.
        let left = Rect::from_min_max(pos2(0.0, 0.0), pos2(500.0, 600.0));
        let top_right = Rect::from_min_max(pos2(506.0, 0.0), pos2(1000.0, 300.0));
        let bottom_right = Rect::from_min_max(pos2(506.0, 306.0), pos2(1000.0, 600.0));
        let all = [left, top_right, bottom_right];
        assert_eq!(neighbor(left, &all, Direction::Right), Some(1), "ties go to the first overlapping pane");
        assert_eq!(neighbor(bottom_right, &all, Direction::Left), Some(0));
        assert_eq!(neighbor(top_right, &all, Direction::Down), Some(2));
        assert_eq!(neighbor(bottom_right, &all, Direction::Up), Some(1));
        assert_eq!(neighbor(left, &all, Direction::Left), None);
        assert_eq!(neighbor(top_right, &all, Direction::Up), None);
    }

    #[test]
    fn positions_cycle() {
        assert_eq!(TabPosition::Top.next().next().next(), TabPosition::Top);
        assert!(TabPosition::Left.is_vertical() && !TabPosition::Top.is_vertical());
        assert_eq!(Side::Left.flip(), Side::Right);
    }
}
