//! The Easy / Pro switch: one big, always-visible control in the top bar.
//!
//! Easy mode hides everything a first-timer doesn't need; Pro shows every
//! control. Switching never changes the song, only what is on screen.

use eframe::egui::{self, Color32, CornerRadius, Sense, Stroke, StrokeKind};

use crate::app::App;

use crate::theme::{self, EASY, PRO};

/// Draw the switch.
pub fn switch(ui: &mut egui::Ui, app: &mut App) {
    let seg = egui::vec2(96.0, 30.0);
    let frame = egui::Frame::new()
        .fill(theme::CANVAS)
        .stroke(Stroke::new(1.0_f32, theme::BORDER_HI))
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
                    theme::alpha(color, 50)
                } else {
                    Color32::TRANSPARENT
                };
                if on {
                    theme::glow(ui.painter(), rect, 15, theme::alpha(color, 110), 16);
                }
                ui.painter().rect_filled(rect, CornerRadius::same(15), fill);
                if on {
                    ui.painter().rect_stroke(
                        rect,
                        CornerRadius::same(15),
                        Stroke::new(1.0_f32, theme::alpha(Color32::WHITE, 70)),
                        StrokeKind::Inside,
                    );
                }
                if r.hovered() && !on {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
                ui.painter().text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    label,
                    theme::bold_font(15.0),
                    if on { theme::CANVAS } else { theme::TEXT_DIM },
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
