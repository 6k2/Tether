//! User themes in Warp's YAML format (also accepted as JSON with the same fields):
//!
//! ```yaml
//! name: Neon Night
//! accent: '#ff2bd6'                      # or {left, right} / {top, bottom}
//! background: { top: '#0b0221', bottom: '#1a0638' }
//! foreground: '#e6e6ff'
//! cursor: '#ff2bd6'                      # optional, defaults to the accent
//! details: darker                        # darker = dark theme, lighter = light theme
//! background_image: { path: bg.jpg, opacity: 40 }   # path relative to the file
//! terminal_colors:
//!   normal: { black: '#1a1a2e', red: '#ff3864', ... }
//!   bright: { black: '#4a4a6a', ... }
//! ```
//!
//! The UI tones are derived from background/foreground/accent the same way the
//! built-ins are (see `theme_catalog::derive_palette`).

use std::path::{Path, PathBuf};

use eframe::egui::Color32;
use serde::Deserialize;
use serde::de::IgnoredAny;

use crate::settings::BackgroundImage;
use crate::theme::Palette;
use crate::theme_catalog::{Colors, derive_palette, ensure_contrast};
use crate::themes::{Gradient, Source, Theme, lerp_color, luminance};

/// Larger files are certainly not themes.
const MAX_FILE_BYTES: u64 = 256 * 1024;
const EXTENSIONS: [&str; 3] = ["yaml", "yml", "json"];

#[derive(Debug, Deserialize)]
struct WarpTheme {
    name: Option<String>,
    background: Fill,
    accent: Fill,
    foreground: String,
    cursor: Option<String>,
    details: Option<Details>,
    background_image: Option<WarpImage>,
    terminal_colors: Option<TerminalColors>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum Fill {
    Solid(String),
    Vertical { top: String, bottom: String },
    Horizontal { left: String, right: String },
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum Details {
    Named(String),
    /// Warp's per-opacity overrides; tether only needs to know it is not darker/lighter.
    Custom(IgnoredAny),
}

#[derive(Debug, Deserialize)]
struct WarpImage {
    path: String,
    /// 0..=100.
    opacity: Option<f32>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct TerminalColors {
    normal: AnsiColors,
    bright: AnsiColors,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct AnsiColors {
    black: Option<String>,
    red: Option<String>,
    green: Option<String>,
    yellow: Option<String>,
    blue: Option<String>,
    magenta: Option<String>,
    cyan: Option<String>,
    white: Option<String>,
}

impl AnsiColors {
    fn get(&self) -> [&Option<String>; 8] {
        [&self.black, &self.red, &self.green, &self.yellow, &self.blue, &self.magenta, &self.cyan, &self.white]
    }
}

/// Parses `#rgb`, `#rrggbb` or `#rrggbbaa` (the `#` is optional; alpha is ignored).
pub fn parse_color(raw: &str) -> Option<Color32> {
    let s = raw.trim().trim_start_matches('#');
    let s = s.strip_prefix("0x").unwrap_or(s);
    if !s.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(&s[i..i + 2], 16).ok();
    match s.len() {
        3 => {
            let d = |i: usize| u8::from_str_radix(&s[i..=i], 16).ok().map(|v| v * 17);
            Some(Color32::from_rgb(d(0)?, d(1)?, d(2)?))
        }
        6 | 8 => Some(Color32::from_rgb(byte(0)?, byte(2)?, byte(4)?)),
        _ => None,
    }
}

fn color(raw: &str, field: &str) -> Result<Color32, String> {
    parse_color(raw).ok_or_else(|| format!("{field}: '{raw}' is not a #rrggbb colour"))
}

fn fill(f: &Fill, field: &str) -> Result<(Color32, Option<Gradient>), String> {
    let (a, b, vertical) = match f {
        Fill::Solid(c) => return Ok((color(c, field)?, None)),
        Fill::Vertical { top, bottom } => (top, bottom, true),
        Fill::Horizontal { left, right } => (left, right, false),
    };
    let start = color(a, field)?;
    Ok((start, Some(Gradient { start, end: color(b, field)?, vertical })))
}

/// Parses theme text. `json` selects the JSON reader; `base_dir` resolves a relative
/// background image path.
pub fn parse(text: &str, json: bool, base_dir: Option<&Path>) -> Result<Theme, String> {
    parse_with(text, json, base_dir, None)
}

/// `parse`, with a name to use when the file has none.
fn parse_with(text: &str, json: bool, base_dir: Option<&Path>, default_name: Option<&str>) -> Result<Theme, String> {
    let w: WarpTheme = if json { serde_json::from_str(text).map_err(|e| e.to_string())? } else { serde_norway::from_str(text).map_err(|e| e.to_string())? };
    let (background, gradient) = fill(&w.background, "background")?;
    let (accent, accent_gradient) = fill(&w.accent, "accent")?;
    let foreground = color(&w.foreground, "foreground")?;
    let cursor = w.cursor.as_deref().map(|c| color(c, "cursor")).transpose()?.unwrap_or(accent);
    let dark = match &w.details {
        Some(Details::Named(d)) if d.eq_ignore_ascii_case("darker") => true,
        Some(Details::Named(d)) if d.eq_ignore_ascii_case("lighter") => false,
        Some(Details::Named(d)) => return Err(format!("details: '{d}' should be darker or lighter")),
        Some(Details::Custom(_)) | None => luminance(background) < 0.4,
    };
    let ansi = ansi_colors(w.terminal_colors.as_ref(), dark)?;
    let chrome = if dark { lerp_color(background, Color32::BLACK, 0.22) } else { lerp_color(background, Color32::BLACK, 0.05) };
    // UI text must stay readable on the chrome and the raised surfaces.
    let text = ensure_contrast(foreground, chrome, 4.6, dark);
    let text = ensure_contrast(text, lerp_color(background, text, 0.1), 4.6, dark);
    let colors = Colors {
        dark,
        bg: chrome,
        surface: background,
        text,
        accent,
        blue: ansi[4],
        danger: ansi[1],
        warn: ansi[3],
        success: ansi[2],
        term_bg: background,
        term_fg: foreground,
        cursor,
        selection: None,
        ansi,
    };
    let name = w.name.or(default_name.map(str::to_string)).map(|n| n.trim().to_string()).filter(|n| !n.is_empty()).ok_or("missing name")?;
    let mut theme = Theme::new(name, if dark { "Custom dark theme" } else { "Custom light theme" }, derive_palette(&colors), Source::Builtin);
    theme.gradient = gradient;
    theme.accent_gradient = accent_gradient;
    theme.background_image = w.background_image.map(|img| {
        let path = PathBuf::from(img.path.trim());
        let path = match base_dir {
            Some(dir) if path.is_relative() => dir.join(path),
            _ => path,
        };
        BackgroundImage { path: path.display().to_string(), opacity: (img.opacity.unwrap_or(30.0) / 100.0).clamp(0.0, 1.0) }
    });
    Ok(theme)
}

fn ansi_colors(tc: Option<&TerminalColors>, dark: bool) -> Result<[Color32; 16], String> {
    let fallback = if dark { Palette::TETHER_NIGHT.ansi } else { crate::theme_catalog::TETHER_DAY_ANSI.map(crate::theme_catalog::hex) };
    let mut out = fallback;
    let Some(tc) = tc else { return Ok(out) };
    for (bank, (label, colors)) in [("normal", &tc.normal), ("bright", &tc.bright)].into_iter().enumerate() {
        for (i, c) in colors.get().into_iter().enumerate() {
            if let Some(raw) = c {
                out[bank * 8 + i] = color(raw, &format!("terminal_colors.{label}[{i}]"))?;
            }
        }
    }
    Ok(out)
}

fn is_theme_file(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| EXTENSIONS.iter().any(|x| x.eq_ignore_ascii_case(e)))
}

/// Reads and parses one theme file; the name defaults to the file stem.
pub fn load_file(path: &Path, base_dir: Option<&Path>) -> Result<Theme, String> {
    let size = std::fs::metadata(path).map_err(|e| e.to_string())?.len();
    if size > MAX_FILE_BYTES {
        return Err(format!("file is too large ({size} bytes)"));
    }
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let json = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("json"));
    let stem = path.file_stem().and_then(|s| s.to_str()).map(|s| s.replace(['_', '-'], " "));
    let mut theme = parse_with(&text, json, base_dir, stem.as_deref())?;
    theme.source = Source::User(path.to_path_buf());
    Ok(theme)
}

/// Loads every theme file in `dir` (sorted by name). Returns the themes and one
/// `"file: reason"` line per file that failed. A missing folder is not an error.
pub fn load_dir(dir: &Path) -> (Vec<Theme>, Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return (Vec::new(), Vec::new()) };
    let mut files: Vec<PathBuf> = entries.flatten().map(|e| e.path()).filter(|p| p.is_file() && is_theme_file(p)).collect();
    files.sort();
    let (mut themes, mut errors) = (Vec::new(), Vec::new());
    for f in files {
        match load_file(&f, Some(dir)) {
            Ok(t) => themes.push(t),
            Err(e) => errors.push(format!("{}: {e}", f.file_name().map(|n| n.to_string_lossy()).unwrap_or_default())),
        }
    }
    themes.sort_by_key(|t| t.name.to_lowercase());
    (themes, errors)
}

/// Cheap fingerprint of the theme files in `dir` (names, sizes, mtimes) for hot reload.
pub fn dir_stamp(dir: &Path) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        let mut rows: Vec<(PathBuf, u64, Option<std::time::SystemTime>)> =
            entries.flatten().filter(|e| is_theme_file(&e.path())).filter_map(|e| e.metadata().ok().map(|m| (e.path(), m.len(), m.modified().ok()))).collect();
        rows.sort();
        rows.hash(&mut h);
    }
    h.finish()
}

/// The `background_image.path` of a theme file when it is relative (to copy it along on import).
pub fn relative_image(file: &Path) -> anyhow::Result<Option<PathBuf>> {
    let base = Path::new("");
    let theme = load_file(file, Some(base)).map_err(|e| anyhow::anyhow!(e))?;
    Ok(theme
        .background_image
        .map(|b| PathBuf::from(b.path))
        .filter(|p| p.is_relative() && !p.components().any(|c| matches!(c, std::path::Component::ParentDir))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::themes::contrast;

    const DRACULA: &str = r##"
name: Dracula
accent: '#bd93f9'
cursor: '#f8f8f2'
background: '#282a36'
foreground: '#f8f8f2'
details: darker
terminal_colors:
  normal:
    black: '#21222c'
    red: '#ff5555'
    green: '#50fa7b'
    yellow: '#f1fa8c'
    blue: '#bd93f9'
    magenta: '#ff79c6'
    cyan: '#8be9fd'
    white: '#f8f8f2'
  bright:
    black: '#6272a4'
    red: '#ff6e6e'
    green: '#69ff94'
    yellow: '#ffffa5'
    blue: '#d6acff'
    magenta: '#ff92df'
    cyan: '#a4ffff'
    white: '#ffffff'
"##;

    const NEON: &str = r##"
name: Neon Night
accent: { left: '#ff2bd6', right: '#00e5ff' }
background: { top: '#0b0221', bottom: '#1a0638' }
foreground: '#e6e6ff'
details: darker
background_image: { path: neon_bg.jpg, opacity: 40 }
terminal_colors:
  normal: { black: '#1a1a2e', red: '#ff3864', green: '#2de2a6', yellow: '#ffd319', blue: '#3a86ff', magenta: '#ff2bd6', cyan: '#00e5ff', white: '#cfcfe8' }
  bright: { black: '#4a4a6a', red: '#ff6b8b', green: '#6bffc8', yellow: '#ffe066', blue: '#6aa6ff', magenta: '#ff70e6', cyan: '#66f0ff', white: '#ffffff' }
"##;

    #[test]
    fn parses_a_solid_warp_theme() {
        let t = parse(DRACULA, false, None).unwrap();
        assert_eq!(t.name, "Dracula");
        let p = t.palette;
        assert!(p.dark);
        assert_eq!(p.term_bg, Color32::from_rgb(0x28, 0x2a, 0x36));
        assert_eq!(p.term_fg, Color32::from_rgb(0xf8, 0xf8, 0xf2));
        assert_eq!(p.accent, Color32::from_rgb(0xbd, 0x93, 0xf9));
        assert_eq!(p.term_cursor, Color32::from_rgb(0xf8, 0xf8, 0xf2));
        assert_eq!(p.ansi[1], Color32::from_rgb(0xff, 0x55, 0x55));
        assert_eq!(p.ansi[15], Color32::WHITE);
        assert!(t.gradient.is_none() && t.background_image.is_none());
        assert!(contrast(p.text, p.surface_3) >= 4.5);
    }

    #[test]
    fn gradients_use_the_first_stop_and_keep_the_data() {
        let dir = Path::new("C:/themes");
        let t = parse(NEON, false, Some(dir)).unwrap();
        let g = t.gradient.unwrap();
        assert!(g.vertical);
        assert_eq!(t.palette.term_bg, Color32::from_rgb(0x0b, 0x02, 0x21));
        assert_eq!(g.end, Color32::from_rgb(0x1a, 0x06, 0x38));
        let a = t.accent_gradient.unwrap();
        assert!(!a.vertical);
        assert_eq!(t.palette.accent, a.start);
        let img = t.background_image.unwrap();
        assert_eq!(PathBuf::from(img.path), dir.join("neon_bg.jpg"));
        assert!((img.opacity - 0.4).abs() < 1e-6);
    }

    #[test]
    fn details_lighter_makes_a_light_theme() {
        let src = "name: Paper\naccent: '#d65d0e'\nbackground: '#fbf1c7'\nforeground: '#3c3836'\ndetails: lighter\n";
        let t = parse(src, false, None).unwrap();
        assert!(!t.palette.dark);
        // No terminal_colors: falls back to a light ANSI set.
        assert_eq!(t.palette.ansi[0], crate::theme_catalog::hex(crate::theme_catalog::TETHER_DAY_ANSI[0]));
        // Custom details: guessed from the background.
        let custom = "name: X\naccent: '#ff0000'\nbackground: '#fafafa'\nforeground: '#111111'\ndetails: { main_text_opacity: 90 }\n";
        assert!(!parse(custom, false, None).unwrap().palette.dark);
        let bad = "name: X\naccent: '#ff0000'\nbackground: '#fafafa'\nforeground: '#111111'\ndetails: sideways\n";
        assert!(parse(bad, false, None).is_err());
    }

    #[test]
    fn json_uses_the_same_fields() {
        let src = r##"{"name":"Json One","accent":"#7aa2f7","background":"#1a1b26","foreground":"#c0caf5","terminal_colors":{"normal":{"red":"#f7768e"}}}"##;
        let t = parse(src, true, None).unwrap();
        assert_eq!(t.name, "Json One");
        assert!(t.palette.dark);
        assert_eq!(t.palette.ansi[1], Color32::from_rgb(0xf7, 0x76, 0x8e));
        assert_eq!(t.palette.ansi[2], Palette::TETHER_NIGHT.ansi[2]);
    }

    #[test]
    fn low_contrast_foreground_is_lifted_for_ui_text() {
        let src = "name: Murky\naccent: '#888888'\nbackground: '#202020'\nforeground: '#4a4a4a'\n";
        let p = parse(src, false, None).unwrap().palette;
        assert!(contrast(p.text, p.bg) >= 4.5 && contrast(p.text, p.surface) >= 4.5);
        assert_eq!(p.term_fg, Color32::from_rgb(0x4a, 0x4a, 0x4a), "terminal colours are the author's");
    }

    #[test]
    fn colour_formats() {
        assert_eq!(parse_color("#fff"), Some(Color32::WHITE));
        assert_eq!(parse_color("00ff00"), Some(Color32::from_rgb(0, 255, 0)));
        assert_eq!(parse_color("#11223344"), Some(Color32::from_rgb(0x11, 0x22, 0x33)));
        assert_eq!(parse_color("#12345"), None);
        assert_eq!(parse_color("#gg0000"), None);
        assert_eq!(parse_color("€€€"), None);
    }

    #[test]
    fn invalid_files_are_skipped_not_fatal() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        std::fs::write(d.join("dracula.yaml"), DRACULA).unwrap();
        std::fs::write(d.join("broken.yml"), "name: [unclosed").unwrap();
        std::fs::write(d.join("badcolour.yaml"), "name: B\naccent: 'blue'\nbackground: '#000000'\nforeground: '#ffffff'\n").unwrap();
        std::fs::write(d.join("missing.json"), r#"{"name":"No background"}"#).unwrap();
        std::fs::write(d.join("nameless_theme.yaml"), "accent: '#ff0000'\nbackground: '#000000'\nforeground: '#ffffff'\n").unwrap();
        std::fs::write(d.join("notes.txt"), "not a theme").unwrap();
        std::fs::write(d.join("huge.yaml"), "#".repeat(MAX_FILE_BYTES as usize + 1)).unwrap();
        let (themes, errors) = load_dir(d);
        let names: Vec<_> = themes.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["Dracula", "nameless theme"]);
        assert!(themes.iter().all(|t| t.is_custom()));
        assert_eq!(errors.len(), 4, "{errors:?}");
        assert!(errors.iter().any(|e| e.starts_with("badcolour.yaml: accent")));
        assert!(load_dir(&d.join("does-not-exist")).0.is_empty());
    }

    #[test]
    fn dir_stamp_changes_when_files_change() {
        let dir = tempfile::tempdir().unwrap();
        let before = dir_stamp(dir.path());
        std::fs::write(dir.path().join("a.yaml"), DRACULA).unwrap();
        let after = dir_stamp(dir.path());
        assert_ne!(before, after);
        std::fs::write(dir.path().join("ignored.txt"), "x").unwrap();
        assert_eq!(after, dir_stamp(dir.path()));
    }

    #[test]
    fn relative_images_are_detected_for_import() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("neon.yaml");
        std::fs::write(&f, NEON).unwrap();
        assert_eq!(relative_image(&f).unwrap(), Some(PathBuf::from("neon_bg.jpg")));
        std::fs::write(&f, DRACULA).unwrap();
        assert_eq!(relative_image(&f).unwrap(), None);
    }
}
