//! VSCode-style code area: syntect highlighting, a pinned line-number gutter, current-line
//! highlight, find-match highlights, soft tabs and auto-indent on Enter.

use std::ops::Range;
use std::sync::{Arc, LazyLock};

use eframe::egui::text::{CCursor, CCursorRange, LayoutJob};
use eframe::egui::{self, CornerRadius, Event, FontId, Id, Key, Rect, Shape, Stroke, TextBuffer, Ui, pos2};
use egui_extras::syntax_highlighting::{self as hl, CodeTheme, SyntectSettings};

use crate::theme;

static SYNTAX: LazyLock<SyntectSettings> = LazyLock::new(SyntectSettings::default);
/// Larger buffers are shown without highlighting so typing stays instant.
const HIGHLIGHT_LIMIT: usize = 256 * 1024;
const GUTTER_PAD: f32 = 26.0;
const NO_GUTTER: f32 = 10.0;
/// egui margins are `i8`.
const MAX_GUTTER: f32 = 120.0;
const MIN_DIGITS: usize = 3;
pub const MAX_MATCHES: usize = 10_000;

/// Syntect syntax name for a file (by extension, then whole name, then first line).
pub fn language_for(file_name: &str, first_line: &str) -> Option<String> {
    let ps = &SYNTAX.ps;
    let ext = file_name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase());
    ext.as_deref()
        .and_then(|e| ps.find_syntax_by_extension(e))
        .or_else(|| ps.find_syntax_by_extension(file_name))
        .or_else(|| ps.find_syntax_by_first_line(first_line))
        .filter(|s| s.name != "Plain Text")
        .map(|s| s.name.clone())
}

/// Leading whitespace of the line containing char index `cursor` (up to the cursor).
pub fn indent_at(text: &str, cursor: usize) -> String {
    let before: String = text.chars().take(cursor).collect();
    let line = before.rsplit('\n').next().unwrap_or("");
    line.chars().take_while(|c| *c == ' ' || *c == '\t').collect()
}

/// 1-based (line, column) of char index `cursor`.
pub fn line_col(text: &str, cursor: usize) -> (usize, usize) {
    let (mut line, mut col) = (1, 1);
    for c in text.chars().take(cursor) {
        if c == '\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
    }
    (line, col)
}

/// Char-index ranges of non-overlapping matches (ASCII case folding when not case sensitive).
pub fn find_matches(text: &str, query: &str, case_sensitive: bool) -> Vec<Range<usize>> {
    if query.is_empty() {
        return Vec::new();
    }
    let (hay, needle) = if case_sensitive {
        (std::borrow::Cow::Borrowed(text), std::borrow::Cow::Borrowed(query))
    } else {
        // ASCII folding keeps byte offsets identical to the original text.
        (text.to_ascii_lowercase().into(), query.to_ascii_lowercase().into())
    };
    let needle_chars = needle.chars().count();
    let (mut last_byte, mut last_char) = (0, 0);
    let mut out = Vec::new();
    for (b, _) in hay.match_indices(needle.as_ref()).take(MAX_MATCHES) {
        last_char += hay[last_byte..b].chars().count();
        last_byte = b;
        out.push(last_char..last_char + needle_chars);
    }
    out
}

pub struct Look {
    pub font: FontId,
    pub dark: bool,
    pub wrap: bool,
    pub language: Option<String>,
}

fn layout_job(ui: &Ui, text: &str, look: &Look, wrap_width: f32) -> LayoutJob {
    let mut job = match &look.language {
        Some(lang) if text.len() <= HIGHLIGHT_LIMIT => {
            let size = look.font.size;
            let code_theme = if look.dark { CodeTheme::dark(size) } else { CodeTheme::light(size) };
            let mut job = hl::highlight_with(ui.ctx(), ui.style(), &code_theme, text, lang, &SYNTAX);
            // egui_extras underlines italic tokens; comments look cleaner without it.
            for s in &mut job.sections {
                s.format.underline = Stroke::NONE;
            }
            job
        }
        _ => LayoutJob::simple(text.to_owned(), look.font.clone(), theme::text(), wrap_width),
    };
    job.wrap.max_width = if look.wrap { wrap_width } else { f32::INFINITY };
    job
}

/// Rewrites Tab into spaces and Enter into newline + current indentation.
fn rewrite_keys(ui: &Ui, id: Id, text: &str, tab_size: usize) {
    let cursor = egui::TextEdit::load_state(ui.ctx(), id).and_then(|s| s.cursor.char_range()).map(|r| r.primary.index.0);
    let spaces = " ".repeat(tab_size.clamp(1, 8));
    let indent = cursor.map(|c| indent_at(text, c)).unwrap_or_default();
    ui.ctx().input_mut(|i| {
        for e in &mut i.events {
            match e {
                Event::Key { key: Key::Tab, pressed: true, modifiers, .. } if modifiers.is_none() => *e = Event::Text(spaces.clone()),
                Event::Key { key: Key::Enter, pressed: true, modifiers, .. } if modifiers.is_none() && !indent.is_empty() => {
                    *e = Event::Text(format!("\n{indent}"));
                }
                _ => {}
            }
        }
    });
}

pub struct CodeArea<'a> {
    pub id: Id,
    pub text: &'a mut String,
    pub look: &'a Look,
    pub tab_size: usize,
    pub line_numbers: bool,
    pub matches: &'a [Range<usize>],
    pub current_match: Option<usize>,
    /// Char range to select and scroll into view this frame.
    pub select: Option<Range<usize>>,
    /// Give the text keyboard focus this frame.
    pub focus: bool,
}

pub struct CodeOutput {
    pub changed: bool,
    pub has_focus: bool,
    /// Primary cursor (char index) and the sorted selection.
    pub cursor: Option<usize>,
    pub selection: Option<Range<usize>>,
}

pub fn show(ui: &mut Ui, area: CodeArea<'_>) -> CodeOutput {
    let CodeArea { id, text, look, tab_size, line_numbers, matches, current_match, select, focus } = area;
    if ui.memory(|m| m.has_focus(id)) {
        rewrite_keys(ui, id, text, tab_size);
    }
    if let Some(sel) = &select {
        let mut state = egui::TextEdit::load_state(ui.ctx(), id).unwrap_or_default();
        state.cursor.set_char_range(Some(CCursorRange::two(CCursor::new(sel.start), CCursor::new(sel.end))));
        state.store(ui.ctx(), id);
    }
    let lines_id = id.with("line_count");
    let lines = ui.data(|d| d.get_temp::<usize>(lines_id)).unwrap_or(1);
    let digits = lines.to_string().len().max(MIN_DIGITS);
    let char_w = ui.fonts_mut(|f| f.glyph_width(&look.font, '0'));
    let gutter_w = if line_numbers { (digits as f32 * char_w + GUTTER_PAD).min(MAX_GUTTER) } else { NO_GUTTER };

    let prev_cursor = egui::TextEdit::load_state(ui.ctx(), id).and_then(|s| s.cursor.char_range()).map(|r| r.primary.index.0);
    let backdrop = ui.painter().add(Shape::Noop);
    let mut layouter = |ui: &Ui, buf: &dyn TextBuffer, wrap_width: f32| {
        let job = layout_job(ui, buf.as_str(), look, wrap_width);
        ui.fonts_mut(|f| f.layout_job(job))
    };
    let width = if look.wrap { ui.available_width() } else { f32::INFINITY };
    let margin = egui::Margin { left: gutter_w as i8, right: 12, top: 6, bottom: 6 };
    let out = egui::TextEdit::multiline(text)
        .id(id)
        .font(look.font.clone())
        .code_editor()
        .frame(egui::Frame::new().inner_margin(margin))
        .desired_width(width)
        .min_size(ui.available_size())
        .layouter(&mut layouter)
        .show(ui);
    if focus {
        out.response.request_focus();
    }

    let galley = &out.galley;
    let origin = out.galley_pos;
    let clip = ui.clip_rect();
    let cursor = out.cursor_range.map(|r| r.primary);
    let cursor_row = cursor.map(|c| galley.layout_from_cursor(c).row);

    // Logical line of every row, the cursor's line, and the line count.
    let mut row_lines = Vec::with_capacity(galley.rows.len());
    let mut line = 1usize;
    for row in &galley.rows {
        row_lines.push(line);
        if row.ends_with_newline {
            line += 1;
        }
    }
    ui.data_mut(|d| d.insert_temp(lines_id, line));
    let cursor_line = cursor_row.and_then(|r| row_lines.get(r).copied());

    let mut back = Vec::new();
    for (row, &ln) in galley.rows.iter().zip(&row_lines) {
        let r = row.rect().translate(origin.to_vec2());
        if Some(ln) == cursor_line && r.max.y >= clip.min.y && r.min.y <= clip.max.y {
            let band = Rect::from_x_y_ranges(clip.x_range(), r.y_range());
            back.push(Shape::rect_filled(band, CornerRadius::ZERO, theme::surface_2().gamma_multiply(0.7)));
        }
    }
    paint_matches(&mut back, galley, origin, clip, matches, current_match);
    ui.painter().set(backdrop, Shape::Vec(back));

    if line_numbers || gutter_w > 0.0 {
        paint_gutter(ui, galley, origin, clip, gutter_w, &row_lines, cursor_line, line_numbers, &look.font);
    }
    let has_focus = out.response.has_focus();
    let cursor_idx = cursor.map(|c| c.index.0);
    if let Some(sel) = &select {
        let r = galley.pos_from_cursor(CCursor::new(sel.start)).translate(origin.to_vec2());
        ui.scroll_to_rect(r.expand2(egui::vec2(gutter_w + 40.0, 40.0)), Some(egui::Align::Center));
    } else if has_focus
        && (out.response.changed() || cursor_idx != prev_cursor)
        && let Some(c) = cursor
    {
        let r = galley.pos_from_cursor(c).translate(origin.to_vec2());
        let clear = Rect::from_min_max(pos2(r.min.x - gutter_w, r.min.y), pos2(r.max.x + 12.0, r.max.y));
        ui.scroll_to_rect(clear, None);
    }
    CodeOutput {
        changed: out.response.changed(),
        has_focus,
        cursor: cursor.map(|c| c.index.0),
        selection: out.cursor_range.map(|r| {
            let s = r.as_sorted_char_range();
            s.start.0..s.end.0
        }),
    }
}

fn paint_matches(out: &mut Vec<Shape>, galley: &egui::Galley, origin: egui::Pos2, clip: Rect, matches: &[Range<usize>], current: Option<usize>) {
    if matches.is_empty() {
        return;
    }
    // Only the matches inside the visible char range are positioned.
    let top = galley.cursor_from_pos(egui::vec2(0.0, clip.min.y - origin.y)).index.0;
    let bottom = galley.cursor_from_pos(egui::vec2(f32::INFINITY, clip.max.y - origin.y)).index.0;
    let first = matches.partition_point(|m| m.end < top);
    let soft = theme::warn().gamma_multiply(0.22);
    for (i, m) in matches.iter().enumerate().skip(first).take_while(|(_, m)| m.start <= bottom) {
        let a = galley.pos_from_cursor(CCursor::new(m.start));
        let b = galley.pos_from_cursor(CCursor::new(m.end));
        let right = if (b.min.y - a.min.y).abs() < 1.0 { b.min.x } else { a.min.x + 6.0 };
        let r = Rect::from_min_max(pos2(a.min.x, a.min.y), pos2(right.max(a.min.x + 2.0), a.max.y)).translate(origin.to_vec2());
        if Some(i) == current {
            out.push(Shape::rect_filled(r, CornerRadius::same(2), theme::warn().gamma_multiply(0.45)));
            out.push(Shape::rect_stroke(r, CornerRadius::same(2), Stroke::new(1.0, theme::warn()), egui::StrokeKind::Outside));
        } else {
            out.push(Shape::rect_filled(r, CornerRadius::same(2), soft));
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_gutter(
    ui: &Ui,
    galley: &Arc<egui::Galley>,
    origin: egui::Pos2,
    clip: Rect,
    gutter_w: f32,
    row_lines: &[usize],
    cursor_line: Option<usize>,
    numbers: bool,
    font: &FontId,
) {
    // Pinned to the visible left edge so it stays put while scrolling sideways.
    let left = clip.min.x;
    let rect = Rect::from_min_max(pos2(left, clip.min.y), pos2(left + gutter_w - 8.0, clip.max.y));
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::ZERO, theme::surface());
    if !numbers {
        return;
    }
    let mut prev = 0;
    for (row, &ln) in galley.rows.iter().zip(row_lines) {
        let is_first_row = ln != prev;
        prev = ln;
        let r = row.rect().translate(origin.to_vec2());
        if !is_first_row || r.max.y < clip.min.y || r.min.y > clip.max.y {
            continue;
        }
        let color = if Some(ln) == cursor_line { theme::text_dim() } else { theme::text_faint().gamma_multiply(0.85) };
        p.text(pos2(rect.max.x - 10.0, r.min.y), egui::Align2::RIGHT_TOP, ln.to_string(), font.clone(), color);
    }
    p.vline(rect.max.x, rect.y_range(), Stroke::new(1.0, theme::border().gamma_multiply(0.5)));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_matches_as_char_ranges() {
        assert_eq!(find_matches("abcabc", "bc", true), [1..3, 4..6]);
        assert_eq!(find_matches("ñaXñax", "ax", false), [1..3, 4..6]);
        assert_eq!(find_matches("aaaa", "aa", true), [0..2, 2..4], "non-overlapping");
        assert!(find_matches("abc", "", true).is_empty());
        assert!(find_matches("ABC", "abc", true).is_empty());
    }

    #[test]
    fn line_and_column_are_one_based() {
        assert_eq!(line_col("ab\ncd", 0), (1, 1));
        assert_eq!(line_col("ab\ncd", 2), (1, 3));
        assert_eq!(line_col("ab\ncd", 3), (2, 1));
        assert_eq!(line_col("ñ\nx", 99), (2, 2));
    }

    #[test]
    fn indent_follows_the_current_line() {
        assert_eq!(indent_at("fn x() {\n    let a;", 18), "    ");
        assert_eq!(indent_at("\tfoo", 4), "\t");
        assert_eq!(indent_at("top", 3), "");
        assert_eq!(indent_at("    abc", 2), "  ", "only up to the cursor");
    }

    #[test]
    fn detects_languages() {
        assert_eq!(language_for("main.rs", "").as_deref(), Some("Rust"));
        assert_eq!(language_for("SETUP.PY", "").as_deref(), Some("Python"));
        assert!(language_for("deploy", "#!/bin/bash").is_some_and(|n| n.to_lowercase().contains("bash")));
        assert_eq!(language_for("notes", "hello"), None);
    }
}
