//! Small OS-level input helpers egui/winit don't expose: the live cursor position during an
//! Explorer (OLE) drag, clipboard formats (image / files), and the physical `Ctrl+V` state
//! (egui-winit swallows `Ctrl+V` when the clipboard holds no text, e.g. an image or files).

use std::path::{Path, PathBuf};

use eframe::egui::{self, Pos2};

/// Pointer position (in points, relative to this viewport) for routing an Explorer drop.
/// Windows sends no `CursorMoved` during an OLE drag, so the live OS cursor is used there.
pub fn drop_pointer_pos(ctx: &egui::Context) -> Option<Pos2> {
    #[cfg(windows)]
    {
        if let Some(p) = os_cursor_in_viewport(ctx) {
            return Some(p);
        }
    }
    ctx.input(|i| i.pointer.hover_pos().or(i.pointer.latest_pos()))
}

#[cfg(windows)]
fn os_cursor_in_viewport(ctx: &egui::Context) -> Option<Pos2> {
    #[repr(C)]
    struct Point {
        x: i32,
        y: i32,
    }
    #[link(name = "user32")]
    unsafe extern "system" {
        fn GetCursorPos(p: *mut Point) -> i32;
    }
    let mut p = Point { x: 0, y: 0 };
    if unsafe { GetCursorPos(&mut p) } == 0 {
        return None;
    }
    let (inner, ppp) = ctx.input(|i| (i.viewport().inner_rect, i.pixels_per_point()));
    let inner = inner?;
    let screen = egui::pos2(p.x as f32 / ppp, p.y as f32 / ppp);
    Some((screen - inner.min).to_pos2())
}

/// True while Ctrl+V is physically held (no Shift / Alt).
#[cfg(windows)]
pub fn ctrl_v_down() -> bool {
    const VK_CONTROL: i32 = 0x11;
    const VK_SHIFT: i32 = 0x10;
    const VK_MENU: i32 = 0x12;
    const VK_V: i32 = 0x56;
    #[link(name = "user32")]
    unsafe extern "system" {
        fn GetAsyncKeyState(vkey: i32) -> i16;
    }
    unsafe {
        let down = |vk: i32| (GetAsyncKeyState(vk) as u16 & 0x8000) != 0;
        down(VK_CONTROL) && down(VK_V) && !down(VK_SHIFT) && !down(VK_MENU)
    }
}

#[cfg(not(windows))]
pub fn ctrl_v_down() -> bool {
    false
}

#[cfg(windows)]
fn has_format(format: u32) -> bool {
    #[link(name = "user32")]
    unsafe extern "system" {
        fn IsClipboardFormatAvailable(format: u32) -> i32;
    }
    unsafe { IsClipboardFormatAvailable(format) != 0 }
}

/// The clipboard holds a bitmap (screenshot, copied image) — what Claude Code pastes as an image.
#[cfg(windows)]
pub fn clipboard_has_image() -> bool {
    const CF_BITMAP: u32 = 2;
    const CF_DIB: u32 = 8;
    const CF_DIBV5: u32 = 17;
    has_format(CF_DIB) || has_format(CF_DIBV5) || has_format(CF_BITMAP)
}

#[cfg(not(windows))]
pub fn clipboard_has_image() -> bool {
    arboard::Clipboard::new().and_then(|mut clipboard| clipboard.get_image()).is_ok()
}

/// The clipboard holds plain text (egui-winit already turns it into `Event::Paste`).
#[cfg(windows)]
pub fn clipboard_has_text() -> bool {
    const CF_TEXT: u32 = 1;
    const CF_UNICODETEXT: u32 = 13;
    has_format(CF_UNICODETEXT) || has_format(CF_TEXT)
}

#[cfg(not(windows))]
pub fn clipboard_has_text() -> bool {
    arboard::Clipboard::new().and_then(|mut clipboard| clipboard.get_text()).is_ok()
}

/// Paths of files copied in Explorer (`CF_HDROP`).
pub fn clipboard_files() -> Vec<PathBuf> {
    super::sftp_view::os_clipboard_file_list()
}

/// Paths as a shell / agent prompt expects them: space separated, quoted when they contain
/// spaces, with a trailing space so the user can keep typing.
pub fn format_paths(paths: &[PathBuf]) -> String {
    let mut out = String::new();
    for p in paths {
        out.push_str(&quote_path(p));
        out.push(' ');
    }
    out
}

fn quote_path(p: &Path) -> String {
    let s = p.display().to_string();
    if s.contains([' ', '\t', '&', '(', ')', '\'', ';']) { format!("\"{s}\"") } else { s }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_paths_with_quotes_only_when_needed() {
        let paths = vec![PathBuf::from(r"C:\img\shot.png"), PathBuf::from(r"C:\My Files\a b.png")];
        assert_eq!(format_paths(&paths), "C:\\img\\shot.png \"C:\\My Files\\a b.png\" ");
        assert_eq!(format_paths(&[]), "");
    }
}
