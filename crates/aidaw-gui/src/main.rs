#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod arrange;
mod fonts;
mod inspector;
mod onboarding;
mod piano_roll;
mod presets;
mod scale;
mod settings;

use std::path::PathBuf;

use eframe::egui;

fn main() -> eframe::Result {
    let path = std::env::args().nth(1).map(PathBuf::from);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("AIDAW")
            .with_inner_size([1400.0, 880.0])
            .with_min_inner_size([900.0, 600.0]),
        ..Default::default()
    };
    eframe::run_native(
        "AIDAW",
        options,
        Box::new(|cc| {
            fonts::install(&cc.egui_ctx);
            cc.egui_ctx.set_visuals(egui::Visuals::dark());
            Ok(Box::new(app::App::new(path)))
        }),
    )
}
