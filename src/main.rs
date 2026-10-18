#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use tether::app;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default().with_title("tether").with_inner_size([1360.0, 840.0]).with_min_inner_size([760.0, 480.0]),
        ..Default::default()
    };
    eframe::run_native("tether", options, Box::new(|cc| Ok(Box::new(app::TetherApp::new(cc)?))))
}
