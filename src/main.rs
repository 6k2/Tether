#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::Arc;

use tether::app;

fn main() -> eframe::Result {
    // `tether hook <agent> [event]`: run by agent hooks. Reports and exits; no window.
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("hook") {
        tether::agents::hook::run(&args[2..]);
        return Ok(());
    }
    let mut viewport = eframe::egui::ViewportBuilder::default()
        .with_title("tether")
        .with_inner_size([1360.0, 840.0])
        .with_min_inner_size([760.0, 480.0])
        .with_transparent(true);
    if let Ok(icon) = eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png")) {
        viewport = viewport.with_icon(Arc::new(icon));
    }
    let options = eframe::NativeOptions { viewport, ..Default::default() };
    eframe::run_native("tether", options, Box::new(|cc| Ok(Box::new(app::TetherApp::new(cc)?))))
}
