//! Warp-style terminal backdrop: the active theme's background gradient and/or a
//! background image, painted over the terminal background and under the text.
//! Images decode on a worker thread and are cached as textures per path.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use eframe::egui::{self, Color32, ColorImage, CornerRadius, Id, Pos2, Rect, TextureHandle, TextureOptions, epaint::RectShape};

use crate::theme;
use crate::themes::{self, Gradient};

/// Longest side images are scaled down to (keeps GPU memory sane for 8K wallpapers).
const MAX_SIDE: u32 = 2560;
/// Larger images are refused rather than decoded.
const MAX_DECODE_SIDE: u32 = 16_384;
const MAX_DECODE_BYTES: u64 = 256 << 20;

enum Slot {
    Loading(Arc<Mutex<Option<Result<ColorImage, String>>>>),
    Ready(TextureHandle),
    Failed,
}

#[derive(Default)]
struct Cache {
    images: HashMap<String, Slot>,
    gradients: HashMap<[u8; 9], TextureHandle>,
}

type SharedCache = Arc<Mutex<Cache>>;

fn cache(ctx: &egui::Context) -> SharedCache {
    ctx.data_mut(|d| d.get_temp_mut_or_default::<SharedCache>(Id::new("tether_backdrop_cache")).clone())
}

/// Paints the backdrop of the active theme into a terminal's `rect` (rounded like the
/// terminal background). Does nothing for themes without one.
pub fn paint(painter: &egui::Painter, rect: Rect) {
    let Some(b) = themes::backdrop() else { return };
    let ctx = painter.ctx();
    let cache = cache(ctx);
    let mut cache = cache.lock().unwrap_or_else(|e| e.into_inner());
    let radius = CornerRadius::same(theme::RADIUS);
    // The gradient replaces the flat terminal colour, so it keeps its alpha (window opacity).
    let alpha = theme::term_bg().a() as f32 / 255.0;
    if let Some(g) = b.gradient {
        let tex = gradient_texture(ctx, &mut cache, g);
        painter.add(RectShape::filled(rect, radius, Color32::WHITE.gamma_multiply(alpha)).with_texture(tex.id(), gradient_uv(g)));
    }
    if let Some(img) = &b.image
        && let Some(tex) = image_texture(ctx, &mut cache, &img.path)
    {
        let uv = cover_uv(tex.size_vec2(), rect.size());
        painter.add(RectShape::filled(rect, radius, Color32::WHITE.gamma_multiply(img.opacity() * alpha)).with_texture(tex.id(), uv));
    }
}

/// Fills the gutter between the panel edge (`ui.max_rect()` grown by `margin` to
/// the right/bottom) and the dock surface inset by its padding, for translucent
/// windows where the panel itself is left unfilled.
pub fn paint_frame(ui: &egui::Ui, margin: egui::Vec2) {
    const DOCK_PADDING: f32 = 6.0;
    let inner = ui.max_rect().shrink(DOCK_PADDING);
    let outer = Rect::from_min_max(ui.max_rect().min, ui.max_rect().max + margin);
    let c = theme::bg();
    let p = ui.painter();
    p.rect_filled(Rect::from_min_max(outer.min, Pos2::new(outer.right(), inner.top())), 0.0, c);
    p.rect_filled(Rect::from_min_max(Pos2::new(outer.left(), inner.bottom()), outer.max), 0.0, c);
    p.rect_filled(Rect::from_min_max(Pos2::new(outer.left(), inner.top()), Pos2::new(inner.left(), inner.bottom())), 0.0, c);
    p.rect_filled(Rect::from_min_max(Pos2::new(inner.right(), inner.top()), Pos2::new(outer.right(), inner.bottom())), 0.0, c);
}

/// A 2-texel texture; linear sampling between the texel centres gives the gradient.
fn gradient_texture(ctx: &egui::Context, cache: &mut Cache, g: Gradient) -> TextureHandle {
    let (a, b) = (g.start, g.end);
    let key = [a.r(), a.g(), a.b(), b.r(), b.g(), b.b(), g.vertical as u8, 0, 0];
    cache
        .gradients
        .entry(key)
        .or_insert_with(|| {
            let size = if g.vertical { [1, 2] } else { [2, 1] };
            let img = ColorImage::new(size, vec![a.to_opaque(), b.to_opaque()]);
            ctx.load_texture("tether-gradient", img, TextureOptions::LINEAR)
        })
        .clone()
}

fn gradient_uv(g: Gradient) -> Rect {
    if g.vertical { Rect::from_min_max(Pos2::new(0.5, 0.25), Pos2::new(0.5, 0.75)) } else { Rect::from_min_max(Pos2::new(0.25, 0.5), Pos2::new(0.75, 0.5)) }
}

/// UV sub-rectangle that scales an image of `img` size to cover `area` (centre crop).
pub fn cover_uv(img: egui::Vec2, area: egui::Vec2) -> Rect {
    if img.x <= 0.0 || img.y <= 0.0 || area.x <= 0.0 || area.y <= 0.0 {
        return Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
    }
    let (img_ratio, area_ratio) = (img.x / img.y, area.x / area.y);
    if img_ratio > area_ratio {
        let w = area_ratio / img_ratio;
        Rect::from_min_max(Pos2::new((1.0 - w) / 2.0, 0.0), Pos2::new((1.0 + w) / 2.0, 1.0))
    } else {
        let h = img_ratio / area_ratio;
        Rect::from_min_max(Pos2::new(0.0, (1.0 - h) / 2.0), Pos2::new(1.0, (1.0 + h) / 2.0))
    }
}

fn image_texture(ctx: &egui::Context, cache: &mut Cache, path: &str) -> Option<TextureHandle> {
    let slot = cache.images.entry(path.to_string()).or_insert_with(|| start_decode(ctx, path));
    if let Slot::Loading(pending) = slot {
        let done = pending.lock().unwrap_or_else(|e| e.into_inner()).take();
        match done {
            None => return None,
            Some(Ok(img)) => *slot = Slot::Ready(ctx.load_texture(format!("tether-bg:{path}"), img, TextureOptions::LINEAR)),
            Some(Err(e)) => {
                eprintln!("tether: background image {path}: {e}");
                *slot = Slot::Failed;
            }
        }
    }
    match slot {
        Slot::Ready(tex) => Some(tex.clone()),
        _ => None,
    }
}

fn start_decode(ctx: &egui::Context, path: &str) -> Slot {
    let pending = Arc::new(Mutex::new(None));
    let (out, path, ctx) = (pending.clone(), path.to_string(), ctx.clone());
    let spawned = std::thread::Builder::new().name("tether-bg-decode".into()).spawn(move || {
        let result = decode(&path);
        *out.lock().unwrap_or_else(|e| e.into_inner()) = Some(result);
        ctx.request_repaint();
    });
    if spawned.is_err() { Slot::Failed } else { Slot::Loading(pending) }
}

/// Decodes a PNG/JPEG file, downscaled to `MAX_SIDE`. Explicit decoder limits: the
/// file may come with a downloaded theme.
pub fn decode(path: &str) -> Result<ColorImage, String> {
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_DECODE_SIDE);
    limits.max_image_height = Some(MAX_DECODE_SIDE);
    limits.max_alloc = Some(MAX_DECODE_BYTES);
    let mut reader = image::ImageReader::open(path).map_err(|e| e.to_string())?.with_guessed_format().map_err(|e| e.to_string())?;
    reader.limits(limits);
    let img = reader.decode().map_err(|e| e.to_string())?;
    let img = if img.width() > MAX_SIDE || img.height() > MAX_SIDE { img.thumbnail(MAX_SIDE, MAX_SIDE) } else { img };
    let rgba = img.to_rgba8();
    Ok(ColorImage::from_rgba_unmultiplied([rgba.width() as usize, rgba.height() as usize], rgba.as_raw()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cover_crops_the_long_side() {
        let uv = cover_uv(egui::vec2(200.0, 100.0), egui::vec2(100.0, 100.0));
        assert!((uv.width() - 0.5).abs() < 1e-6 && (uv.height() - 1.0).abs() < 1e-6);
        assert!((uv.center().x - 0.5).abs() < 1e-6);
        let uv = cover_uv(egui::vec2(100.0, 400.0), egui::vec2(100.0, 100.0));
        assert!((uv.height() - 0.25).abs() < 1e-6);
        assert_eq!(cover_uv(egui::Vec2::ZERO, egui::vec2(1.0, 1.0)), Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)));
    }

    #[test]
    fn decode_reports_bad_files() {
        assert!(decode("no/such/image.png").is_err());
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("x.png");
        std::fs::write(&f, b"garbage").unwrap();
        assert!(decode(&f.display().to_string()).is_err());
        let ok = dir.path().join("ok.png");
        image::RgbaImage::from_pixel(4, 2, image::Rgba([10, 20, 30, 255])).save(&ok).unwrap();
        assert_eq!(decode(&ok.display().to_string()).unwrap().size, [4, 2]);
        // A header claiming a huge image is refused before allocating for it.
        let huge = dir.path().join("huge.png");
        let mut png = std::fs::read(&ok).unwrap();
        png[16..20].copy_from_slice(&100_000u32.to_be_bytes());
        png[20..24].copy_from_slice(&100_000u32.to_be_bytes());
        std::fs::write(&huge, png).unwrap();
        assert!(decode(&huge.display().to_string()).is_err());
    }
}
