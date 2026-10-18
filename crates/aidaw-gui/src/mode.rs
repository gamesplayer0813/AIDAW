//! The Easy / Pro switch: one big, always-visible control in the top bar.
//!
//! Easy mode hides everything a first-timer doesn't need; Pro shows every
//! control. Switching never changes the song, only what is on screen.

use eframe::egui::{self, Color32, CornerRadius, Sense, Stroke, StrokeKind};

use crate::app::App;

const EASY: Color32 = Color32::from_rgb(0x2e, 0xb8, 0x6b);
const PRO: Color32 = Color32::from_rgb(0x8a, 0x5c, 0xf6);

/// Draw the switch.
pub fn switch(ui: &mut egui::Ui, app: &mut App) {
    let seg = egui::vec2(104.0, 32.0);
    let frame = egui::Frame::new()
        .fill(ui.visuals().extreme_bg_color)
        .stroke(Stroke::new(1.5_f32, Color32::from_white_alpha(70)))
        .corner_radius(CornerRadius::same(18))
        .inner_margin(egui::Margin::same(3));
    let resp = frame.show(ui, |ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        ui.horizontal(|ui| {
            for (pro, label, color, tip) in [
                (
                    false,
                    "🐣 Easy",
                    EASY,
                    "Easy mode: only the essentials, big buttons, everything stays in key.\nYou can switch any time; your song is not changed.",
                ),
                (
                    true,
                    "🔧 Pro",
                    PRO,
                    "Pro mode: every control (grid, pan, solo, note velocity, sound shaping, zoom…).\nYou can switch any time; your song is not changed.",
                ),
            ] {
                let on = app.settings.pro == pro;
                let (rect, r) = ui.allocate_exact_size(seg, Sense::click());
                let r = r.on_hover_text(tip);
                let fill = if on {
                    color
                } else if r.hovered() {
                    color.gamma_multiply(0.25)
                } else {
                    Color32::TRANSPARENT
                };
                ui.painter().rect_filled(rect, CornerRadius::same(15), fill);
                if on {
                    ui.painter().rect_stroke(
                        rect,
                        CornerRadius::same(15),
                        Stroke::new(1.0_f32, Color32::WHITE.gamma_multiply(0.6)),
                        StrokeKind::Inside,
                    );
                }
                ui.painter().text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    label,
                    egui::FontId::proportional(17.0),
                    if on {
                        Color32::WHITE
                    } else {
                        ui.visuals().weak_text_color()
                    },
                );
                if r.clicked() && !on {
                    set(app, pro);
                }
            }
        });
    });
    app.ui_rects.insert("mode", resp.response.rect);
}

/// Switch mode, remember it, and tidy settings that Easy mode hides.
pub fn set(app: &mut App, pro: bool) {
    app.settings.pro = pro;
    app.settings.save();
    app.drag = None;
    // Easy mode always keeps notes in key; make sure there is a key to stay in.
    // Pro settings (grid, Stay in key) are left untouched and come back as they were.
    if !pro && app.scale == crate::scale::Scale::Off {
        app.scale = crate::scale::Scale::Major;
    }
    app.status = if pro {
        "Switched to 🔧 Pro mode: every control is shown. Your song is unchanged.".into()
    } else {
        "Switched to 🐣 Easy mode: just the essentials. Your song is unchanged.".into()
    };
}
