//! Painted vector icons for the file list: folders, document pages with a folded corner,
//! a symlink overlay and a small extension badge coloured by file category.
//!
//! Everything is drawn with `Painter` shapes so it stays crisp at any DPI and never depends
//! on glyphs the default egui fonts lack.

use eframe::egui::{Color32, CornerRadius, FontId, Painter, Pos2, Rect, Shape, Stroke, Vec2, pos2, vec2};

use crate::theme;

/// Edge length of the square every icon is drawn into.
pub const ICON_SIZE: f32 = 18.0;
/// Longest badge label; longer extensions are truncated.
const BADGE_MAX_CHARS: usize = 4;
/// Extensions longer than this are treated as part of the name, not as an extension.
const EXT_MAX_CHARS: usize = 10;
const BADGE_TEXT: Color32 = Color32::from_rgb(0x0d, 0x0f, 0x1a);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    Code,
    Image,
    Archive,
    Media,
    Text,
    Config,
    Executable,
    Document,
    Other,
}

/// Lower-case extension of a file name, without the dot. Dotfiles (`.bashrc`), trailing dots
/// and absurdly long suffixes have none.
pub fn extension(name: &str) -> Option<String> {
    let (stem, ext) = name.rsplit_once('.')?;
    if stem.is_empty() || ext.is_empty() || ext.chars().count() > EXT_MAX_CHARS || ext.contains(' ') {
        return None;
    }
    Some(ext.to_ascii_lowercase())
}

/// Category of a lower-case extension.
pub fn category(ext: &str) -> Category {
    match ext {
        "rs" | "py" | "js" | "mjs" | "cjs" | "ts" | "tsx" | "jsx" | "go" | "java" | "kt" | "rb" | "php" | "c" | "h" | "cc" | "cpp" | "hpp" | "cs" | "swift"
        | "lua" | "pl" | "sql" | "html" | "htm" | "css" | "scss" | "vue" | "svelte" | "dart" | "scala" | "zig" => Category::Code,
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "bmp" | "ico" | "tif" | "tiff" | "avif" | "heic" | "psd" => Category::Image,
        "zip" | "gz" | "tgz" | "xz" | "bz2" | "7z" | "rar" | "tar" | "zst" | "lz4" | "deb" | "rpm" | "jar" | "war" | "iso" | "dmg" => Category::Archive,
        "mp3" | "wav" | "flac" | "ogg" | "m4a" | "aac" | "opus" | "mp4" | "mkv" | "mov" | "avi" | "webm" | "wmv" | "m4v" => Category::Media,
        "txt" | "md" | "markdown" | "log" | "csv" | "tsv" | "rst" | "out" | "err" => Category::Text,
        "json" | "yaml" | "yml" | "toml" | "ini" | "conf" | "cfg" | "env" | "xml" | "properties" | "lock" | "service" | "plist" => Category::Config,
        "sh" | "bash" | "zsh" | "fish" | "exe" | "bat" | "cmd" | "ps1" | "bin" | "run" | "appimage" | "msi" | "so" | "dll" | "dylib" => Category::Executable,
        "pdf" | "doc" | "docx" | "odt" | "rtf" | "xls" | "xlsx" | "ods" | "ppt" | "pptx" | "odp" | "epub" => Category::Document,
        _ => Category::Other,
    }
}

pub fn category_color(cat: Category) -> Color32 {
    match cat {
        Category::Code => theme::CAT_CODE,
        Category::Image => theme::CAT_IMAGE,
        Category::Archive => theme::CAT_ARCHIVE,
        Category::Media => theme::CAT_MEDIA,
        Category::Text => theme::CAT_TEXT,
        Category::Config => theme::CAT_CONFIG,
        Category::Executable => theme::CAT_EXEC,
        Category::Document => theme::CAT_DOC,
        Category::Other => theme::text_faint(),
    }
}

/// Upper-case badge label for an extension: at most four ASCII alphanumerics.
pub fn badge_label(ext: &str) -> Option<String> {
    let label: String = ext.chars().filter(char::is_ascii_alphanumeric).take(BADGE_MAX_CHARS).collect::<String>().to_ascii_uppercase();
    (!label.is_empty()).then_some(label)
}

/// Square icon area of `ICON_SIZE` centred on `center`.
pub fn icon_rect(center: Pos2) -> Rect {
    Rect::from_center_size(center, Vec2::splat(ICON_SIZE))
}

/// Folder: back panel with a tab, and a lighter front panel for depth.
pub fn paint_folder(p: &Painter, rect: Rect, color: Color32) {
    let x0 = rect.left() + 1.0;
    let x1 = rect.right() - 1.0;
    let top = rect.top() + 2.5;
    let bottom = rect.bottom() - 2.5;
    let back = color.gamma_multiply(0.62);
    let tab = Rect::from_min_max(pos2(x0, top), pos2(x0 + 7.0, top + 4.0));
    p.rect_filled(tab, CornerRadius::same(2), back);
    let body = Rect::from_min_max(pos2(x0, top + 2.0), pos2(x1, bottom));
    p.rect_filled(body, CornerRadius::same(2), back);
    let front = Rect::from_min_max(pos2(x0, top + 4.5), pos2(x1, bottom));
    p.rect_filled(front, CornerRadius::same(2), color);
    // Thin highlight along the front lip.
    p.hline((front.left() + 1.5)..=(front.right() - 1.5), front.top() + 0.75, Stroke::new(1.0, Color32::WHITE.gamma_multiply(0.18)));
}

/// Document page with a folded top-right corner, plus an extension badge or text lines.
pub fn paint_file(p: &Painter, rect: Rect, ext: Option<&str>) {
    let cat = ext.map_or(Category::Other, category);
    let tint = category_color(cat);
    let w = 13.0;
    let x0 = rect.center().x - w / 2.0;
    let x1 = x0 + w;
    let y0 = rect.top() + 1.0;
    let y1 = rect.bottom() - 1.0;
    let fold = 4.0;
    let page = vec![pos2(x0, y0), pos2(x1 - fold, y0), pos2(x1, y0 + fold), pos2(x1, y1), pos2(x0, y1)];
    p.add(Shape::convex_polygon(page, theme::icon_page(), Stroke::new(1.0, theme::icon_page_edge())));
    let corner = vec![pos2(x1 - fold, y0), pos2(x1 - fold, y0 + fold), pos2(x1, y0 + fold)];
    p.add(Shape::convex_polygon(corner, theme::icon_page_edge(), Stroke::NONE));

    match ext.and_then(badge_label) {
        Some(label) => paint_badge(p, pos2(rect.center().x, y1 - 4.0), &label, tint),
        None => {
            for (i, len) in [7.0, 5.0, 7.0].into_iter().enumerate() {
                let y = y0 + 7.0 + i as f32 * 2.6;
                p.hline((x0 + 3.0)..=(x0 + 3.0 + len), y, Stroke::new(1.0, theme::text_faint()));
            }
        }
    }
}

fn paint_badge(p: &Painter, center: Pos2, label: &str, color: Color32) {
    let font = FontId::proportional(if label.len() > 3 { 6.5 } else { 7.5 });
    let galley = p.layout_no_wrap(label.to_string(), font, BADGE_TEXT);
    let size = vec2((galley.size().x + 4.0).max(11.0), 8.0);
    let rect = Rect::from_center_size(center, size);
    p.rect_filled(rect, CornerRadius::same(2), color);
    let pos = pos2(rect.center().x - galley.size().x / 2.0, rect.center().y - galley.size().y / 2.0);
    p.galley(pos, galley, BADGE_TEXT);
}

/// Small curved-arrow disc in the bottom-left corner marking a symbolic link.
pub fn paint_link_overlay(p: &Painter, rect: Rect) {
    let c = pos2(rect.left() + 4.0, rect.bottom() - 4.0);
    p.circle_filled(c, 4.5, theme::surface());
    p.circle_stroke(c, 4.5, Stroke::new(1.0, theme::text_dim()));
    let s = Stroke::new(1.2, theme::text());
    let tip = c + vec2(2.2, -2.2);
    p.line_segment([c + vec2(-2.0, 2.0), tip], s);
    p.line_segment([tip, tip + vec2(-2.2, 0.0)], s);
    p.line_segment([tip, tip + vec2(0.0, 2.2)], s);
}

/// "Go to parent" arrow: up chevron with a stem, bent to the right at the bottom.
pub fn paint_parent(p: &Painter, rect: Rect, color: Color32) {
    let s = Stroke::new(1.5, color);
    let x = rect.center().x - 2.0;
    let top = rect.top() + 4.0;
    let bottom = rect.bottom() - 4.0;
    p.line_segment([pos2(x, top), pos2(x, bottom)], s);
    p.line_segment([pos2(x, bottom), pos2(x + 7.0, bottom)], s);
    p.line_segment([pos2(x - 3.5, top + 3.5), pos2(x, top)], s);
    p.line_segment([pos2(x, top), pos2(x + 3.5, top + 3.5)], s);
}

/// Magnifier used by the type-to-select pill.
pub fn paint_search(p: &Painter, center: Pos2, color: Color32) {
    let s = Stroke::new(1.3, color);
    let c = center + vec2(-1.0, -1.0);
    p.circle_stroke(c, 3.5, s);
    p.line_segment([c + vec2(2.6, 2.6), c + vec2(5.0, 5.0)], s);
}

/// Icon for a list row. `folder_color` distinguishes local and remote panes.
pub fn paint_entry(p: &Painter, center: Pos2, name: &str, is_dir: bool, is_link: bool, folder_color: Color32) {
    let rect = icon_rect(center);
    if is_dir {
        paint_folder(p, rect, folder_color);
    } else {
        paint_file(p, rect, extension(name).as_deref());
    }
    if is_link {
        paint_link_overlay(p, rect);
    }
}

/// Label for the Type column: "Folder", "Link", the upper-case extension, or "File".
pub fn type_label(name: &str, is_dir: bool, is_link: bool) -> String {
    match (is_dir, is_link) {
        (true, _) => "Folder".into(),
        (false, _) => match extension(name) {
            Some(ext) => ext.to_ascii_uppercase(),
            None if is_link => "Link".into(),
            None => "File".into(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extension_rules() {
        assert_eq!(extension("photo.PNG").as_deref(), Some("png"));
        assert_eq!(extension("archive.tar.gz").as_deref(), Some("gz"));
        assert_eq!(extension(".bashrc"), None);
        assert_eq!(extension("Makefile"), None);
        assert_eq!(extension("trailing."), None);
        assert_eq!(extension("weird.thisisaverylongsuffix"), None);
        assert_eq!(extension("my file.with space"), None);
    }

    #[test]
    fn categories() {
        assert_eq!(category("rs"), Category::Code);
        assert_eq!(category("jpeg"), Category::Image);
        assert_eq!(category("zst"), Category::Archive);
        assert_eq!(category("mkv"), Category::Media);
        assert_eq!(category("md"), Category::Text);
        assert_eq!(category("yaml"), Category::Config);
        assert_eq!(category("sh"), Category::Executable);
        assert_eq!(category("pdf"), Category::Document);
        assert_eq!(category("xyz"), Category::Other);
    }

    #[test]
    fn badge_labels() {
        assert_eq!(badge_label("js").as_deref(), Some("JS"));
        assert_eq!(badge_label("json").as_deref(), Some("JSON"));
        assert_eq!(badge_label("appimage").as_deref(), Some("APPI"));
        assert_eq!(badge_label("--"), None);
    }

    #[test]
    fn type_labels() {
        assert_eq!(type_label("src", true, false), "Folder");
        assert_eq!(type_label("a.png", false, false), "PNG");
        assert_eq!(type_label("README", false, false), "File");
        assert_eq!(type_label("current", false, true), "Link");
    }
}
