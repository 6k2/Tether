//! Theme catalog: the built-ins (see `theme_catalog`) plus user themes loaded from
//! `<config>/themes/*.yaml|yml|json` (Warp's theme format, see `theme_import`).
//! Each theme is a full `Palette` plus optional Warp-style decoration (gradient,
//! background image).
//!
//! Also owns the "live" side of theming: applying with a crossfade, window opacity,
//! following the OS light/dark mode and the backdrop painted behind terminals.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, LazyLock, Mutex, RwLock};

use eframe::egui::{self, Color32};

use crate::settings::{self, BackgroundImage};
use crate::theme::{self, Palette};

/// Name of the theme used when the saved one is unknown.
pub const DEFAULT_THEME: &str = "Tether Night";
const CROSSFADE_SECS: f64 = 0.32;
/// How often the themes folder is checked for changes while Settings is open.
const POLL_SECS: f64 = 2.0;

/// Two colour stops; `vertical` runs top -> bottom, otherwise left -> right.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Gradient {
    pub start: Color32,
    pub end: Color32,
    pub vertical: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    Builtin,
    /// Loaded from this file in the themes folder.
    User(PathBuf),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Theme {
    pub name: String,
    /// One-line flavour text shown in the theme picker.
    pub tagline: String,
    pub palette: Palette,
    pub source: Source,
    /// Terminal background gradient; `palette.term_bg` is its first stop.
    pub gradient: Option<Gradient>,
    /// Accent gradient of a Warp theme (`palette.accent` is its first stop).
    pub accent_gradient: Option<Gradient>,
    /// Image shipped with the theme (absolute path, opacity 0..=1).
    pub background_image: Option<BackgroundImage>,
}

impl Theme {
    pub fn new(name: impl Into<String>, tagline: impl Into<String>, palette: Palette, source: Source) -> Self {
        Self { name: name.into(), tagline: tagline.into(), palette, source, gradient: None, accent_gradient: None, background_image: None }
    }

    pub fn is_custom(&self) -> bool {
        matches!(self.source, Source::User(_))
    }
}

// ---- catalog ------------------------------------------------------------------------

fn builtins() -> &'static [Theme] {
    static B: LazyLock<Vec<Theme>> = LazyLock::new(crate::theme_catalog::build);
    &B
}

static CATALOG: RwLock<Option<Arc<Vec<Theme>>>> = RwLock::new(None);
static LOAD_ERRORS: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Built-ins (default first) followed by the user themes.
pub fn all() -> Arc<Vec<Theme>> {
    if let Some(list) = CATALOG.read().unwrap_or_else(|e| e.into_inner()).as_ref() {
        return list.clone();
    }
    let list = Arc::new(builtins().to_vec());
    *CATALOG.write().unwrap_or_else(|e| e.into_inner()) = Some(list.clone());
    list
}

/// Case-insensitive lookup.
pub fn by_name(name: &str) -> Option<Theme> {
    // Names used by earlier builds.
    let name = match name.trim() {
        "Termius Dark" => DEFAULT_THEME,
        "Termius Light" => "Tether Day",
        other => other,
    };
    all().iter().find(|t| t.name.eq_ignore_ascii_case(name)).cloned()
}

/// Replaces the user themes. A name taken by a built-in (or an earlier file) gets a suffix.
pub fn set_user_themes(user: Vec<Theme>) {
    let mut list = builtins().to_vec();
    for mut t in user {
        let base = t.name.clone();
        let mut n = 1;
        while list.iter().any(|o| o.name.eq_ignore_ascii_case(&t.name)) {
            n += 1;
            t.name = if n == 2 { format!("{base} (custom)") } else { format!("{base} (custom {n})") };
        }
        list.push(t);
    }
    *CATALOG.write().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(list));
}

/// `<config>/themes`, once settings are initialised.
pub fn user_dir() -> Option<PathBuf> {
    settings::path().and_then(|p| p.parent().map(|d| d.join("themes")))
}

/// Re-reads the themes folder. Returns how many themes loaded; failures are logged
/// and listed by `load_errors`.
pub fn reload_user_themes() -> usize {
    let Some(dir) = user_dir() else { return 0 };
    let (themes, errors) = crate::theme_import::load_dir(&dir);
    for e in &errors {
        eprintln!("tether: skipped theme {e}");
    }
    let n = themes.len();
    set_user_themes(themes);
    *LOAD_ERRORS.lock().unwrap_or_else(|e| e.into_inner()) = errors;
    *poll_state() = PollState { stamp: crate::theme_import::dir_stamp(&dir), checked_at: f64::NEG_INFINITY };
    n
}

/// Files in the themes folder that could not be read (`"file: reason"`).
pub fn load_errors() -> Vec<String> {
    LOAD_ERRORS.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

#[derive(Default)]
struct PollState {
    stamp: u64,
    checked_at: f64,
}

fn poll_state() -> std::sync::MutexGuard<'static, PollState> {
    static P: Mutex<PollState> = Mutex::new(PollState { stamp: 0, checked_at: f64::NEG_INFINITY });
    P.lock().unwrap_or_else(|e| e.into_inner())
}

/// Hot reload: every couple of seconds, reloads the user themes if the folder changed
/// (and re-applies the active theme if it is one of them). Call while Settings is open.
pub fn poll_user_themes(ctx: &egui::Context) {
    let now = ctx.input(|i| i.time);
    let Some(dir) = user_dir() else { return };
    {
        let mut st = poll_state();
        if now - st.checked_at < POLL_SECS {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(POLL_SECS));
            return;
        }
        st.checked_at = now;
        if crate::theme_import::dir_stamp(&dir) == st.stamp {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(POLL_SECS));
            return;
        }
    }
    reload_user_themes();
    poll_state().checked_at = now;
    let current = settings::get().appearance.theme;
    if by_name(&current).is_some_and(|t| t.is_custom()) {
        apply_animated(ctx, &current);
    }
}

/// Validates `file` as a theme, copies it (and a relative background image) into the
/// themes folder and reloads. Returns the imported theme's name.
pub fn import_file(file: &Path) -> anyhow::Result<String> {
    let dir = user_dir().ok_or_else(|| anyhow::anyhow!("settings are not initialised"))?;
    import_into(file, &dir)
}

/// [`import_file`] into `dir`. An existing theme file of the same name is kept (the
/// import gets a numbered name); an image would be overwritten only by the same bytes.
fn import_into(file: &Path, dir: &Path) -> anyhow::Result<String> {
    let parsed = crate::theme_import::load_file(file, file.parent()).map_err(|e| anyhow::anyhow!(e))?;
    std::fs::create_dir_all(dir)?;
    let file_name = file.file_name().ok_or_else(|| anyhow::anyhow!("not a file"))?;
    if let Some(rel) = crate::theme_import::relative_image(file)? {
        let src_dir = file.parent().unwrap_or(Path::new(".")).canonicalize()?;
        let (src, dst) = (src_dir.join(&rel), dir.join(&rel));
        // `load_file` checked the path; check again after resolving links.
        let src_real = src.canonicalize().ok().filter(|s| s.starts_with(&src_dir) && s.is_file());
        if let Some(src) = src_real
            && dst.canonicalize().ok() != Some(src.clone())
        {
            let bytes = std::fs::read(&src)?;
            if std::fs::read(&dst).is_ok_and(|old| old != bytes) {
                anyhow::bail!("the themes folder already has a different {}; rename the image first", rel.display());
            }
            if let Some(parent) = dst.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&dst, bytes)?;
        }
    }
    // The theme file last, so a refused image leaves nothing half-imported.
    let mut dest = dir.join(file_name);
    if dest != file {
        let bytes = std::fs::read(file)?;
        dest = free_name(&dest, &bytes);
        std::fs::write(&dest, &bytes)?;
    }
    reload_user_themes();
    let name = all().iter().rev().find(|t| t.source == Source::User(dest.clone())).map_or(parsed.name, |t| t.name.clone());
    Ok(name)
}

/// `path`, or `name (2).ext`, `name (3).ext`… when a different file already has that name.
fn free_name(path: &Path, bytes: &[u8]) -> PathBuf {
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let ext = path.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    let mut candidate = path.to_path_buf();
    for n in 2.. {
        match std::fs::read(&candidate) {
            Ok(existing) if existing != bytes => candidate = path.with_file_name(format!("{stem} ({n}){ext}")),
            _ => break,
        }
    }
    candidate
}

// ---- applying -----------------------------------------------------------------------

/// The active palette before window opacity is applied (what crossfades start from).
static BASE: Mutex<Palette> = Mutex::new(Palette::TETHER_NIGHT);
/// Window opacity as f32 bits (1.0 = opaque).
static OPACITY: AtomicU32 = AtomicU32::new(0x3f80_0000);

/// What `ui::backdrop` paints behind terminal text.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Backdrop {
    pub gradient: Option<Gradient>,
    pub image: Option<BackgroundImage>,
}

static BACKDROP: RwLock<Option<Backdrop>> = RwLock::new(None);
static ACTIVE: RwLock<Option<Theme>> = RwLock::new(None);

/// The gradient/image of the active theme, if it has any.
pub fn backdrop() -> Option<Backdrop> {
    BACKDROP.read().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Window opacity in effect (1.0 = opaque).
pub fn window_opacity() -> f32 {
    f32::from_bits(OPACITY.load(Ordering::Relaxed))
}

/// True when the window background is translucent.
pub fn see_through() -> bool {
    window_opacity() < 0.999
}

/// `p` with the window opacity applied to the window and terminal backgrounds.
fn with_opacity(mut p: Palette, opacity: f32) -> Palette {
    if opacity < 0.999 {
        p.bg = p.bg.gamma_multiply(opacity);
        p.term_bg = p.term_bg.gamma_multiply(opacity);
    }
    p
}

fn set_base(ctx: &egui::Context, p: Palette) {
    *BASE.lock().unwrap_or_else(|e| e.into_inner()) = p;
    theme::set_palette(with_opacity(p, window_opacity()));
    theme::install(ctx);
}

fn base() -> Palette {
    *BASE.lock().unwrap_or_else(|e| e.into_inner())
}

fn set_active(t: Option<Theme>) {
    *ACTIVE.write().unwrap_or_else(|e| e.into_inner()) = t;
    update_backdrop(&settings::get().appearance);
}

fn update_backdrop(a: &settings::Appearance) {
    let active = ACTIVE.read().unwrap_or_else(|e| e.into_inner()).clone();
    let spec = active.and_then(|t| {
        let image = a.background_image(&t.name).cloned().or(t.background_image.clone()).filter(|b| b.opacity() > 0.0);
        (t.gradient.is_some() || image.is_some()).then_some(Backdrop { gradient: t.gradient, image })
    });
    *BACKDROP.write().unwrap_or_else(|e| e.into_inner()) = spec;
}

/// Applies a theme instantly. Returns false (and applies the default) if `name` is unknown.
pub fn apply(ctx: &egui::Context, name: &str) -> bool {
    *transition() = None;
    let found = by_name(name);
    set_base(ctx, found.as_ref().map_or(Palette::TETHER_NIGHT, |t| t.palette));
    let ok = found.is_some();
    set_active(found.or_else(|| by_name(DEFAULT_THEME)));
    ok
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
    *transition() = Some(Transition { from: base(), to: target.palette, start });
    set_active(Some(target));
    ctx.request_repaint();
}

/// Applies the saved settings: user themes, theme (or the OS-synced one), opacity and
/// terminal font. Call once at startup, after `settings::init`.
pub fn apply_saved(ctx: &egui::Context) {
    reload_user_themes();
    let a = settings::get().appearance;
    OPACITY.store(a.window_opacity().to_bits(), Ordering::Relaxed);
    let name = synced_theme(&a, ctx.system_theme()).unwrap_or(a.theme);
    apply(ctx, &name);
    crate::fonts::apply(ctx);
}

/// Re-applies the non-theme appearance settings (opacity, background images).
pub fn refresh(ctx: &egui::Context) {
    let a = settings::get().appearance;
    OPACITY.store(a.window_opacity().to_bits(), Ordering::Relaxed);
    update_backdrop(&a);
    if transition().is_none() {
        set_base(ctx, base());
    }
}

/// With "Sync with OS" on, the theme to use for the OS mode `system`.
pub fn synced_theme(a: &settings::Appearance, system: Option<egui::Theme>) -> Option<String> {
    if !a.sync_with_os {
        return None;
    }
    let name = match system? {
        egui::Theme::Dark => &a.dark_theme,
        egui::Theme::Light => &a.light_theme,
    };
    by_name(name).map(|t| t.name)
}

/// Follows OS light/dark switches when "Sync with OS" is on. Call every frame (cheap).
pub fn follow_system(ctx: &egui::Context) {
    static LAST: Mutex<Option<egui::Theme>> = Mutex::new(None);
    let sys = ctx.system_theme();
    {
        let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
        if *last == sys {
            return;
        }
        let first = last.is_none();
        *last = sys;
        if first && sys.is_none() {
            return;
        }
    }
    let a = settings::get().appearance;
    if let Some(name) = synced_theme(&a, sys)
        && !name.eq_ignore_ascii_case(&a.theme)
    {
        let _ = settings::update(|s| s.appearance.theme = name.clone());
        apply_animated(ctx, &name);
    }
}

/// Advances a running crossfade. Returns true while one is in progress.
pub fn tick(ctx: &egui::Context) -> bool {
    let mut guard = transition();
    let Some(tr) = *guard else { return false };
    let now = ctx.input(|i| i.time);
    let t = (((now - tr.start) / CROSSFADE_SECS) as f32).clamp(0.0, 1.0);
    let eased = ease_out_cubic(t);
    if t >= 1.0 {
        *guard = None;
    } else {
        ctx.request_repaint();
    }
    drop(guard);
    set_base(ctx, lerp(&tr.from, &tr.to, eased));
    t < 1.0
}

/// Jumps a running crossfade to its end (e.g. when the settings screen closes).
pub fn finish(ctx: &egui::Context) {
    let pending = transition().take();
    if let Some(tr) = pending {
        set_base(ctx, tr.to);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imports_never_overwrite_other_files() {
        let (src, themes) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let theme = "name: Glow\naccent: '#ff0000'\nbackground: '#000000'\nforeground: '#ffffff'\nbackground_image: { path: bg.png }\n";
        let file = src.path().join("glow.yaml");
        std::fs::write(&file, theme).unwrap();
        std::fs::write(src.path().join("bg.png"), b"new image").unwrap();
        // A different theme already uses the name: it is kept, the import is renamed.
        std::fs::write(themes.path().join("glow.yaml"), "name: Mine\n").unwrap();
        // A different image of that name blocks the import instead of being replaced.
        std::fs::write(themes.path().join("bg.png"), b"their image").unwrap();
        assert!(import_into(&file, themes.path()).is_err());
        assert_eq!(std::fs::read(themes.path().join("bg.png")).unwrap(), b"their image");
        assert!(!themes.path().join("glow (2).yaml").exists(), "nothing half-imported");
        std::fs::remove_file(themes.path().join("bg.png")).unwrap();
        import_into(&file, themes.path()).unwrap();
        assert_eq!(std::fs::read_to_string(themes.path().join("glow.yaml")).unwrap(), "name: Mine\n");
        assert_eq!(std::fs::read_to_string(themes.path().join("glow (2).yaml")).unwrap(), theme);
        assert_eq!(std::fs::read(themes.path().join("bg.png")).unwrap(), b"new image");
        // Importing the same file again reuses its copy.
        import_into(&file, themes.path()).unwrap();
        assert!(!themes.path().join("glow (3).yaml").exists());
    }

    #[test]
    fn catalog_names_are_unique_and_default_is_first() {
        let names: Vec<_> = builtins().iter().map(|t| t.name.to_lowercase()).collect();
        let mut sorted = names.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), names.len());
        assert!(names.len() >= 30, "only {} built-in themes", names.len());
        assert_eq!(builtins()[0].name, DEFAULT_THEME);
        assert_eq!(builtins()[0].palette, Palette::TETHER_NIGHT);
        assert_eq!(settings::Appearance::default().theme, DEFAULT_THEME);
        assert!(builtins().iter().all(|t| !t.is_custom() && !t.tagline.is_empty()));
    }

    #[test]
    fn every_theme_has_readable_text() {
        for t in builtins() {
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
            if let Some(g) = t.gradient {
                for stop in [g.start, g.end] {
                    let c = contrast(p.term_fg, stop);
                    assert!(c >= 4.5, "{}: terminal fg on gradient stop is {c:.2}", t.name);
                }
            }
            assert!(contrast(p.on_accent, p.accent) >= 3.0, "{}: on_accent", t.name);
        }
    }

    #[test]
    fn light_flag_matches_background() {
        for t in builtins() {
            let light_bg = luminance(t.palette.surface) > 0.5;
            assert_eq!(t.palette.dark, !light_bg, "{}", t.name);
        }
        assert!(builtins().iter().filter(|t| !t.palette.dark).count() >= 8);
    }

    #[test]
    fn lookup_is_case_insensitive() {
        assert_eq!(by_name("nord").map(|t| t.name), Some("Nord".to_string()));
        assert_eq!(by_name(" Tokyo Night ").map(|t| t.name), Some("Tokyo Night".to_string()));
        assert_eq!(by_name("gruvbox dark").map(|t| t.name), Some("Gruvbox Dark".to_string()));
        assert!(by_name("nope").is_none());
    }

    #[test]
    fn lerp_hits_endpoints() {
        let a = builtins()[0].palette;
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

    #[test]
    fn opacity_only_touches_window_and_terminal_backgrounds() {
        let p = Palette::TETHER_NIGHT;
        assert_eq!(with_opacity(p, 1.0), p);
        let q = with_opacity(p, 0.5);
        assert!(q.bg.a() < 255 && q.term_bg.a() < 255);
        assert_eq!((q.surface, q.text, q.ansi), (p.surface, p.text, p.ansi));
    }

    #[test]
    fn sync_with_os_picks_the_matching_slot() {
        let mut a = settings::Appearance { sync_with_os: true, light_theme: "github light".into(), dark_theme: "Nord".into(), ..Default::default() };
        assert_eq!(synced_theme(&a, Some(egui::Theme::Light)).as_deref(), Some("GitHub Light"));
        assert_eq!(synced_theme(&a, Some(egui::Theme::Dark)).as_deref(), Some("Nord"));
        assert_eq!(synced_theme(&a, None), None);
        a.sync_with_os = false;
        assert_eq!(synced_theme(&a, Some(egui::Theme::Dark)), None);
    }
}
