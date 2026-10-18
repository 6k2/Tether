//! Built-in theme catalog. Each theme is a full `Palette`: UI surfaces, text tiers,
//! accent, semantic colours, terminal colours and the 16 ANSI colours.
//!
//! Themes are written as a compact `Spec` (the scheme's published colours) and the
//! in-between tones (raised surfaces, borders, dim/faint text, translucent accents)
//! are derived so every theme gets the same visual rhythm and readable contrast.

use std::sync::{LazyLock, Mutex};

use eframe::egui::{self, Color32};

use crate::settings;
use crate::theme::{self, Palette};

/// Name of the theme used when the saved one is unknown.
pub const DEFAULT_THEME: &str = "Tether Night";
const CROSSFADE_SECS: f64 = 0.32;
/// Minimum WCAG contrast the derived dim text keeps against every surface.
const DIM_TARGET: f32 = 3.3;
const FAINT_TARGET: f32 = 2.1;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Theme {
    pub name: &'static str,
    /// One-line flavour text shown in the theme picker.
    pub tagline: &'static str,
    pub palette: Palette,
}

pub fn all() -> &'static [Theme] {
    static THEMES: LazyLock<Vec<Theme>> = LazyLock::new(build_catalog);
    &THEMES
}

/// Case-insensitive lookup.
pub fn by_name(name: &str) -> Option<&'static Theme> {
    // Names used by earlier builds.
    let name = match name.trim() {
        "Termius Dark" => DEFAULT_THEME,
        "Termius Light" => "Tether Day",
        other => other,
    };
    all().iter().find(|t| t.name.eq_ignore_ascii_case(name))
}

/// Applies a theme instantly. Returns false (and applies the default) if `name` is unknown.
pub fn apply(ctx: &egui::Context, name: &str) -> bool {
    *transition() = None;
    let found = by_name(name);
    theme::set_palette(found.map_or(Palette::TETHER_NIGHT, |t| t.palette));
    theme::install(ctx);
    found.is_some()
}

/// Applies a theme with a short colour crossfade (instant when animations are off).
/// Call `tick` every frame while it runs; `SettingsView::show` does.
pub fn apply_animated(ctx: &egui::Context, name: &str) {
    let Some(target) = by_name(name) else {
        apply(ctx, name);
        return;
    };
    if !settings::get().appearance.animations {
        apply(ctx, name);
        return;
    }
    let start = ctx.input(|i| i.time);
    *transition() = Some(Transition { from: theme::palette(), to: target.palette, start });
    ctx.request_repaint();
}

/// Applies the theme saved in settings. Call once at startup, after `settings::init`.
pub fn apply_saved(ctx: &egui::Context) {
    apply(ctx, &settings::get().appearance.theme);
}

/// Advances a running crossfade. Returns true while one is in progress.
pub fn tick(ctx: &egui::Context) -> bool {
    let mut guard = transition();
    let Some(tr) = *guard else { return false };
    let now = ctx.input(|i| i.time);
    let t = (((now - tr.start) / CROSSFADE_SECS) as f32).clamp(0.0, 1.0);
    let eased = ease_out_cubic(t);
    theme::set_palette(lerp(&tr.from, &tr.to, eased));
    if t >= 1.0 {
        *guard = None;
    } else {
        ctx.request_repaint();
    }
    drop(guard);
    theme::install(ctx);
    t < 1.0
}

/// Jumps a running crossfade to its end (e.g. when the settings screen closes).
pub fn finish(ctx: &egui::Context) {
    let pending = transition().take();
    if let Some(tr) = pending {
        theme::set_palette(tr.to);
        theme::install(ctx);
    }
}

#[derive(Clone, Copy)]
struct Transition {
    from: Palette,
    to: Palette,
    start: f64,
}

fn transition() -> std::sync::MutexGuard<'static, Option<Transition>> {
    static T: Mutex<Option<Transition>> = Mutex::new(None);
    T.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn ease_out_cubic(t: f32) -> f32 {
    1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(3)
}

/// Blends every colour of two palettes; `dark` flips at the midpoint.
pub fn lerp(a: &Palette, b: &Palette, t: f32) -> Palette {
    let m = |x: Color32, y: Color32| lerp_color(x, y, t);
    let mut ansi = a.ansi;
    for (i, c) in ansi.iter_mut().enumerate() {
        *c = m(a.ansi[i], b.ansi[i]);
    }
    Palette {
        dark: if t < 0.5 { a.dark } else { b.dark },
        bg: m(a.bg, b.bg),
        surface: m(a.surface, b.surface),
        surface_2: m(a.surface_2, b.surface_2),
        surface_3: m(a.surface_3, b.surface_3),
        border: m(a.border, b.border),
        text: m(a.text, b.text),
        text_dim: m(a.text_dim, b.text_dim),
        text_faint: m(a.text_faint, b.text_faint),
        accent: m(a.accent, b.accent),
        accent_soft: m(a.accent_soft, b.accent_soft),
        blue: m(a.blue, b.blue),
        danger: m(a.danger, b.danger),
        warn: m(a.warn, b.warn),
        success: m(a.success, b.success),
        shadow: m(a.shadow, b.shadow),
        on_accent: m(a.on_accent, b.on_accent),
        icon_page: m(a.icon_page, b.icon_page),
        icon_page_edge: m(a.icon_page_edge, b.icon_page_edge),
        row_separator: m(a.row_separator, b.row_separator),
        term_bg: m(a.term_bg, b.term_bg),
        term_fg: m(a.term_fg, b.term_fg),
        term_cursor: m(a.term_cursor, b.term_cursor),
        term_selection: m(a.term_selection, b.term_selection),
        ansi,
    }
}

/// Component-wise blend (works on premultiplied colours too).
pub fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let ch = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_premultiplied(ch(a.r(), b.r()), ch(a.g(), b.g()), ch(a.b(), b.b()), ch(a.a(), b.a()))
}

/// WCAG relative luminance of an opaque colour.
pub fn luminance(c: Color32) -> f32 {
    let lin = |v: u8| {
        let s = v as f32 / 255.0;
        if s <= 0.04045 { s / 12.92 } else { ((s + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * lin(c.r()) + 0.7152 * lin(c.g()) + 0.0722 * lin(c.b())
}

/// WCAG contrast ratio (1..=21).
pub fn contrast(a: Color32, b: Color32) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

// ---- catalog ------------------------------------------------------------------------

struct Spec {
    name: &'static str,
    tagline: &'static str,
    dark: bool,
    /// Window chrome behind panels.
    bg: u32,
    /// Cards, tab bodies.
    surface: u32,
    text: u32,
    accent: u32,
    blue: u32,
    danger: u32,
    warn: u32,
    success: u32,
    term_bg: u32,
    term_fg: u32,
    cursor: u32,
    /// Solid selection colour; `None` derives a translucent accent.
    selection: Option<u32>,
    ansi: [u32; 16],
}

const fn hex(v: u32) -> Color32 {
    Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    lerp_color(a, b, t)
}

fn with_alpha(c: Color32, alpha: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), alpha)
}

/// Strongest blend of `fg` toward `base` that still keeps `target` contrast on all `backs`.
fn readable_blend(fg: Color32, base: Color32, start: f32, target: f32, backs: &[Color32]) -> Color32 {
    let mut t = start;
    loop {
        let c = mix(fg, base, t);
        if t <= 0.0 || backs.iter().all(|b| contrast(c, *b) >= target) {
            return c;
        }
        t = (t - 0.02).max(0.0);
    }
}

impl Spec {
    fn build(&self) -> Theme {
        let (bg, surface, text) = (hex(self.bg), hex(self.surface), hex(self.text));
        let accent = hex(self.accent);
        let lift = |t: f32| mix(surface, text, t);
        let (surface_2, surface_3) = if self.dark { (lift(0.045), lift(0.095)) } else { (lift(0.035), lift(0.075)) };
        let border = lift(if self.dark { 0.14 } else { 0.13 });
        let backs = [bg, surface, surface_2];
        let text_dim = readable_blend(text, surface, 0.42, DIM_TARGET, &backs);
        let text_faint = readable_blend(text, surface, 0.62, FAINT_TARGET, &backs);
        let ink = if self.dark { mix(hex(self.term_bg), Color32::BLACK, 0.5) } else { mix(text, Color32::BLACK, 0.4) };
        let on_accent = if contrast(ink, accent) >= contrast(Color32::WHITE, accent) { ink } else { Color32::WHITE };
        let palette = Palette {
            dark: self.dark,
            bg,
            surface,
            surface_2,
            surface_3,
            border,
            text,
            text_dim,
            text_faint,
            accent,
            accent_soft: with_alpha(accent, if self.dark { 0x5c } else { 0x3a }),
            blue: hex(self.blue),
            danger: hex(self.danger),
            warn: hex(self.warn),
            success: hex(self.success),
            shadow: Color32::from_black_alpha(if self.dark { 140 } else { 36 }),
            on_accent,
            icon_page: lift(if self.dark { 0.2 } else { 0.16 }),
            icon_page_edge: lift(if self.dark { 0.32 } else { 0.28 }),
            row_separator: if self.dark { Color32::from_black_alpha(0x50) } else { with_alpha(text, 0x12) },
            term_bg: hex(self.term_bg),
            term_fg: hex(self.term_fg),
            term_cursor: hex(self.cursor),
            term_selection: self.selection.map_or(with_alpha(accent, if self.dark { 0x70 } else { 0x48 }), hex),
            ansi: self.ansi.map(hex),
        };
        Theme { name: self.name, tagline: self.tagline, palette }
    }
}

fn build_catalog() -> Vec<Theme> {
    let mut out = vec![Theme { name: DEFAULT_THEME, tagline: "Deep navy, signature green", palette: Palette::TETHER_NIGHT }];
    out.extend(SPECS.iter().map(Spec::build));
    out
}

const SPECS: &[Spec] = &[
    Spec {
        name: "Tether Day",
        tagline: "Crisp paper, the same green",
        dark: false,
        bg: 0xeceef3,
        surface: 0xffffff,
        text: 0x1e2133,
        accent: 0x12a05a,
        blue: 0x1f6fd1,
        danger: 0xd63b40,
        warn: 0xb5761a,
        success: 0x12a05a,
        term_bg: 0xfbfbfd,
        term_fg: 0x2a2e3f,
        cursor: 0x12a05a,
        selection: Some(0xcdeedd),
        ansi: [
            0x2a2e3f, 0xd63b40, 0x0d8a50, 0xb5761a, 0x1b66c9, 0xb0307a, 0x0b8496, 0xa3a7b8, 0x5a5f7a, 0xe5534b, 0x17a862, 0xc98a20, 0x3384e6, 0xcc4499,
            0x1a9fb0, 0xc9ccd6,
        ],
    },
    Spec {
        name: "Kanagawa Wave",
        tagline: "Ink, fuji white and wave blue",
        dark: true,
        bg: 0x16161d,
        surface: 0x1f1f28,
        text: 0xdcd7ba,
        accent: 0x7e9cd8,
        blue: 0x7fb4ca,
        danger: 0xe82424,
        warn: 0xff9e3b,
        success: 0x98bb6c,
        term_bg: 0x1f1f28,
        term_fg: 0xdcd7ba,
        cursor: 0xc8c093,
        selection: Some(0x2d4f67),
        ansi: [
            0x090618, 0xc34043, 0x76946a, 0xc0a36e, 0x7e9cd8, 0x957fb8, 0x6a9589, 0xc8c093, 0x727169, 0xe82424, 0x98bb6c, 0xe6c384, 0x7fb4ca, 0x938aa9,
            0x7aa89f, 0xdcd7ba,
        ],
    },
    Spec {
        name: "Kanagawa Lotus",
        tagline: "Warm parchment, lotus ink",
        dark: false,
        bg: 0xe5ddb0,
        surface: 0xf2ecbc,
        text: 0x43436c,
        accent: 0x4d699b,
        blue: 0x6693bf,
        danger: 0xc84053,
        warn: 0xcc6d00,
        success: 0x6f894e,
        term_bg: 0xf2ecbc,
        term_fg: 0x545464,
        cursor: 0x43436c,
        selection: Some(0xc9cbd1),
        ansi: [
            0x1f1f28, 0xc84053, 0x6f894e, 0x77713f, 0x4d699b, 0xb35b79, 0x597b75, 0x545464, 0x8a8980, 0xd7474b, 0x6e915f, 0x836f4a, 0x6693bf, 0x624c83,
            0x5e857a, 0x43436c,
        ],
    },
    Spec {
        name: "Flexoki Dark",
        tagline: "Inky paper, analog tones",
        dark: true,
        bg: 0x100f0f,
        surface: 0x1c1b1a,
        text: 0xcecdc3,
        accent: 0x3aa99f,
        blue: 0x4385be,
        danger: 0xd14d41,
        warn: 0xd0a215,
        success: 0x879a39,
        term_bg: 0x1c1b1a,
        term_fg: 0xcecdc3,
        cursor: 0xcecdc3,
        selection: Some(0x403e3c),
        ansi: [
            0x100f0f, 0xaf3029, 0x66800b, 0xad8301, 0x205ea6, 0xa02f6f, 0x24837b, 0x878580, 0x575653, 0xd14d41, 0x879a39, 0xd0a215, 0x4385be, 0xce5d97,
            0x3aa99f, 0xcecdc3,
        ],
    },
    Spec {
        name: "Hacker Green",
        tagline: "Phosphor on black",
        dark: true,
        bg: 0x030805,
        surface: 0x07110a,
        text: 0xc9f7d6,
        accent: 0x2bff6b,
        blue: 0x3ae0c4,
        danger: 0xff5f56,
        warn: 0xe3ff7a,
        success: 0x2bff6b,
        term_bg: 0x050c07,
        term_fg: 0x33ff77,
        cursor: 0x2bff6b,
        selection: Some(0x0f3d1f),
        ansi: [
            0x0d1a12, 0xff5f56, 0x2bff6b, 0xc8f542, 0x3ae0c4, 0x7dffb0, 0x22d3a0, 0xa8f0bc, 0x2f5a3d, 0xff8a80, 0x6dff9a, 0xe3ff7a, 0x6ff5dc, 0xb0ffd0,
            0x5cf0c0, 0xe6ffec,
        ],
    },
    Spec {
        name: "Nord",
        tagline: "Arctic, north-bluish calm",
        dark: true,
        bg: 0x242933,
        surface: 0x2e3440,
        text: 0xeceff4,
        accent: 0x88c0d0,
        blue: 0x81a1c1,
        danger: 0xbf616a,
        warn: 0xebcb8b,
        success: 0xa3be8c,
        term_bg: 0x2e3440,
        term_fg: 0xd8dee9,
        cursor: 0xd8dee9,
        selection: Some(0x434c5e),
        ansi: [
            0x3b4252, 0xbf616a, 0xa3be8c, 0xebcb8b, 0x81a1c1, 0xb48ead, 0x88c0d0, 0xe5e9f0, 0x4c566a, 0xbf616a, 0xa3be8c, 0xebcb8b, 0x81a1c1, 0xb48ead,
            0x8fbcbb, 0xeceff4,
        ],
    },
    Spec {
        name: "Dracula",
        tagline: "Dark purple, vivid neon",
        dark: true,
        bg: 0x21222c,
        surface: 0x282a36,
        text: 0xf8f8f2,
        accent: 0xbd93f9,
        blue: 0x8be9fd,
        danger: 0xff5555,
        warn: 0xffb86c,
        success: 0x50fa7b,
        term_bg: 0x282a36,
        term_fg: 0xf8f8f2,
        cursor: 0xf8f8f2,
        selection: Some(0x44475a),
        ansi: [
            0x21222c, 0xff5555, 0x50fa7b, 0xf1fa8c, 0xbd93f9, 0xff79c6, 0x8be9fd, 0xf8f8f2, 0x6272a4, 0xff6e6e, 0x69ff94, 0xffffa5, 0xd6acff, 0xff92df,
            0xa4ffff, 0xffffff,
        ],
    },
    Spec {
        name: "Catppuccin Mocha",
        tagline: "Soothing pastels after dark",
        dark: true,
        bg: 0x181825,
        surface: 0x1e1e2e,
        text: 0xcdd6f4,
        accent: 0xcba6f7,
        blue: 0x89b4fa,
        danger: 0xf38ba8,
        warn: 0xfab387,
        success: 0xa6e3a1,
        term_bg: 0x1e1e2e,
        term_fg: 0xcdd6f4,
        cursor: 0xf5e0dc,
        selection: Some(0x45475a),
        ansi: [
            0x45475a, 0xf38ba8, 0xa6e3a1, 0xf9e2af, 0x89b4fa, 0xf5c2e7, 0x94e2d5, 0xbac2de, 0x585b70, 0xf38ba8, 0xa6e3a1, 0xf9e2af, 0x89b4fa, 0xf5c2e7,
            0x94e2d5, 0xa6adc8,
        ],
    },
    Spec {
        name: "Catppuccin Latte",
        tagline: "Pastels in daylight",
        dark: false,
        bg: 0xe6e9ef,
        surface: 0xeff1f5,
        text: 0x4c4f69,
        accent: 0x8839ef,
        blue: 0x1e66f5,
        danger: 0xd20f39,
        warn: 0xfe640b,
        success: 0x40a02b,
        term_bg: 0xeff1f5,
        term_fg: 0x4c4f69,
        cursor: 0xdc8a78,
        selection: Some(0xccd0da),
        ansi: [
            0x5c5f77, 0xd20f39, 0x40a02b, 0xdf8e1d, 0x1e66f5, 0xea76cb, 0x179299, 0xacb0be, 0x6c6f85, 0xd20f39, 0x40a02b, 0xdf8e1d, 0x1e66f5, 0xea76cb,
            0x179299, 0xbcc0cc,
        ],
    },
    Spec {
        name: "Tokyo Night",
        tagline: "Neon city lights",
        dark: true,
        bg: 0x16161e,
        surface: 0x1a1b26,
        text: 0xc0caf5,
        accent: 0x7aa2f7,
        blue: 0x7dcfff,
        danger: 0xf7768e,
        warn: 0xe0af68,
        success: 0x9ece6a,
        term_bg: 0x1a1b26,
        term_fg: 0xc0caf5,
        cursor: 0xc0caf5,
        selection: Some(0x283457),
        ansi: [
            0x15161e, 0xf7768e, 0x9ece6a, 0xe0af68, 0x7aa2f7, 0xbb9af7, 0x7dcfff, 0xa9b1d6, 0x414868, 0xf7768e, 0x9ece6a, 0xe0af68, 0x7aa2f7, 0xbb9af7,
            0x7dcfff, 0xc0caf5,
        ],
    },
    Spec {
        name: "Solarized Light",
        tagline: "Precision colours, warm base",
        dark: false,
        bg: 0xeee8d5,
        surface: 0xfdf6e3,
        text: 0x073642,
        accent: 0x268bd2,
        blue: 0x268bd2,
        danger: 0xdc322f,
        warn: 0xb58900,
        success: 0x859900,
        term_bg: 0xfdf6e3,
        term_fg: 0x586e75,
        cursor: 0x586e75,
        selection: None,
        ansi: [
            0x073642, 0xdc322f, 0x859900, 0xb58900, 0x268bd2, 0xd33682, 0x2aa198, 0xeee8d5, 0x002b36, 0xcb4b16, 0x586e75, 0x657b83, 0x839496, 0x6c71c4,
            0x93a1a1, 0xfdf6e3,
        ],
    },
    Spec {
        name: "Rosé Pine",
        tagline: "All natural pine, faux fur and soho vibes",
        dark: true,
        bg: 0x191724,
        surface: 0x1f1d2e,
        text: 0xe0def4,
        accent: 0xebbcba,
        blue: 0x9ccfd8,
        danger: 0xeb6f92,
        warn: 0xf6c177,
        success: 0x9ccfd8,
        term_bg: 0x191724,
        term_fg: 0xe0def4,
        cursor: 0xebbcba,
        selection: Some(0x403d52),
        ansi: [
            0x26233a, 0xeb6f92, 0x31748f, 0xf6c177, 0x9ccfd8, 0xc4a7e7, 0xebbcba, 0xe0def4, 0x6e6a86, 0xeb6f92, 0x31748f, 0xf6c177, 0x9ccfd8, 0xc4a7e7,
            0xebbcba, 0xe0def4,
        ],
    },
    Spec {
        name: "One Dark",
        tagline: "The classic Atom editor look",
        dark: true,
        bg: 0x21252b,
        surface: 0x282c34,
        text: 0xd7dae0,
        accent: 0x61afef,
        blue: 0x61afef,
        danger: 0xe06c75,
        warn: 0xe5c07b,
        success: 0x98c379,
        term_bg: 0x282c34,
        term_fg: 0xabb2bf,
        cursor: 0x528bff,
        selection: Some(0x3e4451),
        ansi: [
            0x3f4451, 0xe05561, 0x8cc265, 0xd18f52, 0x4aa5f0, 0xc162de, 0x42b3c2, 0xd7dae0, 0x4f5666, 0xff616e, 0xa5e075, 0xf0a45d, 0x4dc4ff, 0xde73ff,
            0x4cd1e0, 0xe6e6e6,
        ],
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_names_are_unique_and_default_is_first() {
        let names: Vec<_> = all().iter().map(|t| t.name.to_ascii_lowercase()).collect();
        let mut sorted = names.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), names.len());
        assert!(names.len() >= 12);
        assert_eq!(all()[0].name, DEFAULT_THEME);
        assert_eq!(all()[0].palette, Palette::TETHER_NIGHT);
        assert_eq!(settings::Appearance::default().theme, DEFAULT_THEME);
    }

    #[test]
    fn every_theme_has_readable_text() {
        for t in all() {
            let p = &t.palette;
            for (label, back) in [("bg", p.bg), ("surface", p.surface), ("surface_2", p.surface_2), ("surface_3", p.surface_3)] {
                let c = contrast(p.text, back);
                assert!(c >= 4.5, "{}: text on {label} is {c:.2}", t.name);
            }
            for (label, back) in [("bg", p.bg), ("surface", p.surface), ("surface_2", p.surface_2)] {
                let c = contrast(p.text_dim, back);
                assert!(c >= 3.0, "{}: text_dim on {label} is {c:.2}", t.name);
            }
            let c = contrast(p.term_fg, p.term_bg);
            assert!(c >= 4.5, "{}: terminal fg is {c:.2}", t.name);
            assert!(contrast(p.on_accent, p.accent) >= 3.0, "{}: on_accent", t.name);
        }
    }

    #[test]
    fn light_flag_matches_background() {
        for t in all() {
            let light_bg = luminance(t.palette.surface) > 0.5;
            assert_eq!(t.palette.dark, !light_bg, "{}", t.name);
        }
        assert!(all().iter().filter(|t| !t.palette.dark).count() >= 3);
    }

    #[test]
    fn lookup_is_case_insensitive() {
        assert_eq!(by_name("nord").map(|t| t.name), Some("Nord"));
        assert_eq!(by_name(" Tokyo Night ").map(|t| t.name), Some("Tokyo Night"));
        assert!(by_name("nope").is_none());
    }

    #[test]
    fn lerp_hits_endpoints() {
        let a = all()[0].palette;
        let b = by_name("Catppuccin Latte").unwrap().palette;
        assert_eq!(lerp(&a, &b, 0.0), a);
        assert_eq!(lerp(&a, &b, 1.0), b);
        assert!(lerp(&a, &b, 0.4).dark);
        assert!(!lerp(&a, &b, 0.6).dark);
    }

    #[test]
    fn contrast_reference_values() {
        assert!((contrast(Color32::BLACK, Color32::WHITE) - 21.0).abs() < 0.01);
        assert!((contrast(Color32::WHITE, Color32::WHITE) - 1.0).abs() < 1e-6);
    }
}
