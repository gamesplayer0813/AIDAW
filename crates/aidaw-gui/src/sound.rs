//! "Sound" window: which speakers we play through, whether the device is
//! actually running, a test beep and a level meter. Also writes `aidaw.log`
//! (next to the settings, see [`crate::settings::dir`]) so a "no sound"
//! report comes with the reason.

use std::io::Write;

use aidaw_engine::{
    drivers, has_asio, input_devices, output_device_names, output_devices, Engine, MidiProblem,
};
use eframe::egui::{self, Color32, RichText};

use crate::app::App;
use crate::i18n::{tr, trf};
use crate::icons::{self, Icon};

/// Past this size the log becomes `aidaw.old.log` and a new one starts.
const LOG_LIMIT: u64 = 1 << 20;

/// Add a line to `aidaw.log`. It lives with the settings because the exe's
/// folder may be read-only (Program Files), where logging used to fail
/// silently.
pub fn log(line: &str) {
    let Some(dir) = crate::settings::dir() else {
        return;
    };
    let path = dir.join("aidaw.log");
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > LOG_LIMIT) {
        let _ = std::fs::rename(&path, dir.join("aidaw.old.log"));
    }
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
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
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(60.0, 8.0), egui::Sense::click());
    let p = ui.painter();
    p.rect_filled(rect, 4.0, crate::theme::CANVAS);
    let level = peak.clamp(0.0, 1.0);
    let mut fill = rect;
    fill.set_width(rect.width() * level);
    let color = if level > 0.95 {
        crate::theme::DANGER
    } else if level > 0.7 {
        crate::theme::WARN
    } else {
        crate::theme::OK
    };
    if level > 0.0 {
        crate::theme::glow(p, fill, 4, crate::theme::alpha(color, 90), 8);
    }
    p.rect_filled(fill, 4.0, color);
    resp
}

/// A row of small level meters, one per input channel.
pub fn input_meters(ui: &mut egui::Ui, peaks: &[f32]) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        for (i, p) in peaks.iter().take(8).enumerate() {
            ui.label(RichText::new(format!("{}", i + 1)).size(10.5).color(crate::theme::TEXT_FAINT));
            meter(ui, *p).on_hover_text(tr("Input level: sing or play, it should move. Red means too loud: turn the gain down on your audio interface."));
        }
        if peaks.is_empty() {
            ui.weak(tr("no input open"));
        }
    });
}

pub fn show_window(ctx: &egui::Context, app: &mut App, open: &mut bool) {
    // What to (re)open once the window is drawn: driver and output device.
    let mut switch_to: Option<(Option<String>, Option<String>)> = None;
    let mut pick_input: Option<Option<String>> = None;
    let mut refresh_midi = false;
    egui::Window::new(tr("🔊 Sound")).id(egui::Id::new("sound_window")).open(open).resizable(false).default_width(440.0).show(ctx, |ui| {
        let drivers = drivers();
        let current_driver = app.engine.as_ref().map(|e| e.driver().to_string());
        if drivers.len() > 1 || app.pro() {
            ui.horizontal(|ui| {
                ui.label(tr("Driver"));
                let shown = current_driver.clone().unwrap_or_else(|| drivers.first().map_or(String::new(), |d| d.to_string()));
                egui::ComboBox::from_id_salt("driver").selected_text(&shown).show_ui(ui, |ui| {
                    for d in &drivers {
                        if ui.selectable_label(shown.eq_ignore_ascii_case(d), *d).clicked() && !shown.eq_ignore_ascii_case(d) {
                            switch_to = Some((Some(d.to_string()), None));
                        }
                    }
                });
                if current_driver.as_deref().is_some_and(|d| d.eq_ignore_ascii_case("asio")) {
                    ui.weak(tr("ASIO: the lowest delay with an audio interface"));
                }
            });
            if cfg!(windows) && !has_asio() {
                ui.weak(tr("This build has no ASIO. WASAPI works with every sound card; for ASIO see \"ASIO\" in README.txt."));
            }
        }
        match &app.engine {
            Some(e) => {
                let d = e.diagnostics();
                ui.label(RichText::new(trf("Playing through: {}", &[&e.device_name()])).strong());
                ui.weak(e.format());
                if d.callbacks == 0 {
                    ui.colored_label(Color32::LIGHT_RED, tr("⚠ The sound device is not running (it never asked for audio). Try another device below."));
                } else {
                    ui.colored_label(Color32::LIGHT_GREEN, trf("✔ Sound device is running ({} buffers so far)", &[&d.callbacks]));
                }
                if let Some(err) = &d.error {
                    ui.colored_label(Color32::LIGHT_RED, trf("Device error: {}", &[err]));
                }
                ui.horizontal(|ui| {
                    ui.label(tr("Output level"));
                    meter(ui, d.peak);
                    if !d.latency.is_zero() {
                        ui.weak(trf("delay {} ms", &[&d.latency.as_millis()])).on_hover_text(tr("How long it takes from AIDAW to your ears. Recordings are lined up with the song automatically."));
                    }
                });
                ui.add_space(4.0);
                if ui.button(RichText::new(tr("🔔 Play a test beep")).strong()).clicked() {
                    e.test_tone();
                }
                ui.weak(tr("Meter moves but you hear nothing? Check the Windows volume (speaker icon in the taskbar), that the right speakers/headphones are the default, and that they aren't muted."));
            }
            None => {
                ui.colored_label(Color32::LIGHT_RED, RichText::new(tr("🔇 AIDAW couldn't open any sound device.")).strong());
                if let Some(err) = &app.engine_error {
                    ui.add(egui::Label::new(RichText::new(err).monospace().small()).wrap());
                }
            }
        }
        ui.separator();
        ui.label(tr("Choose speakers / headphones:"));
        let driver = current_driver.clone().or_else(|| app.settings.driver.clone());
        if app.device_names.is_empty() {
            app.device_names = output_devices(driver.as_deref());
        }
        let current = app.engine.as_ref().map(|e| e.device_name().to_string());
        for (i, name) in app.device_names.iter().enumerate() {
            let label = if i == 0 { trf("{}  (Windows default)", &[name]) } else { name.clone() };
            if ui.selectable_label(current.as_deref() == Some(name.as_str()), label).clicked() {
                // The default entry follows Windows when the default changes.
                switch_to = Some((driver.clone(), (i > 0).then(|| name.clone())));
            }
        }
        if app.device_names.is_empty() {
            ui.weak(tr("No output devices found. Plug in speakers or headphones."));
        }

        ui.separator();
        ui.label(RichText::new(tr("Recording input (microphone, guitar, line in):")).strong());
        let asio = app.engine.as_ref().is_some_and(|e| e.is_asio());
        if asio {
            ui.weak(tr("With ASIO, recording uses the inputs of the same device."));
        } else {
            if app.input_names.is_empty() {
                app.input_names = input_devices(driver.as_deref());
            }
            let open_input = app.engine.as_ref().and_then(|e| e.input()).map(|i| i.name().to_string());
            for (i, name) in app.input_names.iter().enumerate() {
                let chosen = match &open_input {
                    Some(n) => n == name,
                    None => app.settings.input.as_deref().map_or(i == 0, |s| s == name),
                };
                let label = if i == 0 { trf("{}  (Windows default)", &[name]) } else { name.clone() };
                if ui.selectable_label(chosen, label).clicked() && !chosen {
                    pick_input = Some((i > 0).then(|| name.clone()));
                }
            }
            if app.input_names.is_empty() {
                ui.weak(tr("No microphone or other input found. Plug one in and click Refresh."));
            }
        }
        match app.engine.as_ref().and_then(|e| e.input()) {
            Some(input) => {
                ui.weak(format!("{}: {}", input.name(), input.format()));
                input_meters(ui, &app.input_peaks);
                if input.callbacks() == 0 {
                    ui.weak(tr("Waiting for the input to start…"));
                }
                if let Some(err) = input.error() {
                    ui.colored_label(Color32::LIGHT_RED, trf("Device error: {}", &[&err]));
                }
            }
            None => {
                if let Some(err) = &app.input_error {
                    ui.colored_label(Color32::LIGHT_RED, trf("Couldn't open the input: {}", &[err]));
                }
            }
        }
        if cfg!(windows) && !asio {
            ui.weak(tr("No level at all? In Windows Settings › Privacy & security › Microphone, turn on \"Let desktop apps access your microphone\"."));
        }

        ui.separator();
        ui.label(RichText::new(tr("MIDI keyboards:")).strong());
        let (ports, problems) = app.engine.as_ref().map(|e| (e.midi_ports(), e.midi_problems())).unwrap_or_default();
        if ports.is_empty() {
            ui.weak(tr("No MIDI keyboard found. Plug one in (USB) and click Refresh."));
        } else {
            for p in &ports {
                let fresh = app.midi_seen.is_some_and(|t| t.elapsed().as_millis() < 300);
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
                    ui.painter().circle_filled(r.center(), 3.5, if fresh { crate::theme::OK } else { crate::theme::TEXT_FAINT });
                    ui.label(p);
                });
            }
            ui.weak(tr("Keys play the armed instrument track, or the selected one. Arm it and press Record to record the notes."));
        }
        for p in &problems {
            let text = match p {
                MidiProblem::Unavailable(e) => trf("MIDI isn't available on this computer: {}", &[e]),
                MidiProblem::Port { name, error } => trf("Couldn't open {}: {}", &[name, error]),
                MidiProblem::Missing(name) => trf("MIDI keyboard {} not found", &[name]),
            };
            ui.colored_label(Color32::LIGHT_RED, text);
        }
        ui.add_space(4.0);
        if ui.add(icons::button(Icon::Sync, tr("Refresh list"))).on_hover_text(tr("Look again for speakers, inputs and MIDI keyboards")).clicked() {
            app.device_names = output_devices(driver.as_deref());
            app.input_names = input_devices(driver.as_deref());
            refresh_midi = true;
        }
    });
    if let Some((driver, output)) = switch_to {
        app.device_names.clear();
        app.input_names.clear();
        app.open_audio(driver.as_deref(), output.as_deref());
    }
    if let Some(input) = pick_input {
        app.settings.input = input;
        app.settings.save();
        app.reopen_input();
    }
    if refresh_midi {
        app.reopen_input();
        if let Some(e) = &mut app.engine {
            e.open_midi(None);
        }
    }
}
