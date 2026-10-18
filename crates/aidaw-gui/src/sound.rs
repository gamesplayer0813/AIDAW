//! "Sound" window: which speakers we play through, whether the device is
//! actually running, a test beep and a level meter. Also writes `aidaw.log`
//! next to the exe so a "no sound" report comes with the reason.

use std::io::Write;

use aidaw_engine::{output_device_names, Engine};
use eframe::egui::{self, Color32, RichText};

use crate::app::App;

pub fn log(line: &str) {
    let Some(path) = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("aidaw.log")))
    else {
        return;
    };
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let _ = writeln!(f, "[{t}] {line}");
    }
}

pub fn log_engine(result: &Result<Engine, String>) {
    match result {
        Ok(e) => log(&format!(
            "audio: opened {:?} ({})",
            e.device_name(),
            e.format()
        )),
        Err(err) => log(&format!("audio: FAILED: {err}")),
    }
    log(&format!("audio: devices = {:?}", output_device_names()));
}

/// Small level meter for the top bar.
pub fn meter(ui: &mut egui::Ui, peak: f32) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(60.0, 10.0), egui::Sense::click());
    let p = ui.painter();
    p.rect_filled(rect, 2.0, Color32::from_gray(40));
    let level = peak.clamp(0.0, 1.0);
    let mut fill = rect;
    fill.set_width(rect.width() * level);
    let color = if level > 0.95 {
        Color32::RED
    } else if level > 0.7 {
        Color32::YELLOW
    } else {
        Color32::LIGHT_GREEN
    };
    p.rect_filled(fill, 2.0, color);
    resp
}

pub fn show_window(ctx: &egui::Context, app: &mut App, open: &mut bool) {
    let mut switch_to: Option<String> = None;
    egui::Window::new("🔊 Sound").open(open).resizable(false).default_width(420.0).show(ctx, |ui| {
        match &app.engine {
            Some(e) => {
                let d = e.diagnostics();
                ui.label(RichText::new(format!("Playing through: {}", e.device_name())).strong());
                ui.weak(e.format());
                if d.callbacks == 0 {
                    ui.colored_label(Color32::LIGHT_RED, "⚠ The sound device is not running (it never asked for audio). Try another device below.");
                } else {
                    ui.colored_label(Color32::LIGHT_GREEN, format!("✔ Sound device is running ({} buffers so far)", d.callbacks));
                }
                if let Some(err) = &d.error {
                    ui.colored_label(Color32::LIGHT_RED, format!("Device error: {err}"));
                }
                ui.horizontal(|ui| {
                    ui.label("Output level");
                    meter(ui, d.peak);
                });
                ui.add_space(4.0);
                if ui.button(RichText::new("🔔 Play a test beep").strong()).clicked() {
                    e.test_tone();
                }
                ui.weak("Meter moves but you hear nothing? Check the Windows volume (speaker icon in the taskbar), that the right speakers/headphones are the default, and that they aren't muted.");
            }
            None => {
                ui.colored_label(Color32::LIGHT_RED, RichText::new("🔇 AIDAW couldn't open any sound device.").strong());
                if let Some(err) = &app.engine_error {
                    ui.add(egui::Label::new(RichText::new(err).monospace().small()).wrap());
                }
            }
        }
        ui.separator();
        ui.label("Choose speakers / headphones:");
        if app.device_names.is_empty() {
            app.device_names = output_device_names();
        }
        let current = app.engine.as_ref().map(|e| e.device_name().to_string());
        for (i, name) in app.device_names.iter().enumerate() {
            let label = if i == 0 { format!("{name}  (Windows default)") } else { name.clone() };
            if ui.selectable_label(current.as_deref() == Some(name.as_str()), label).clicked() {
                switch_to = Some(name.clone());
            }
        }
        if app.device_names.is_empty() {
            ui.weak("No output devices found. Plug in speakers or headphones.");
        }
        if ui.button("⟳ Refresh list").clicked() {
            app.device_names = output_device_names();
        }
    });
    if let Some(name) = switch_to {
        app.open_audio(Some(&name));
    }
}
