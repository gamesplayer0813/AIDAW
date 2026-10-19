//! The Easy / Pro switch: one big, always-visible control in the top bar.
//!
//! Easy mode hides everything a first-timer doesn't need; Pro shows every
//! control. Switching never changes the song, only what is on screen.

use eframe::egui::{self, Color32, CornerRadius, Sense, Stroke, StrokeKind};

use crate::app::App;
use crate::i18n::{self, tr, Lang};
use crate::icons::{self, Icon};

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
            for (pro, icon, label, color, tip) in [
                (
                    false,
                    Icon::Sprout,
                    tr("Easy"),
                    EASY,
                    tr("Easy mode: only the essentials, big buttons, everything stays in key.\nYou can switch any time; your song is not changed."),
                ),
                (
                    true,
                    Icon::Sliders,
                    tr("Pro"),
                    PRO,
                    tr("Pro mode: every control (grid, pan, solo, note velocity, sound shaping, zoom…).\nYou can switch any time; your song is not changed."),
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
                let ink = if on { theme::CANVAS } else { theme::TEXT_DIM };
                let galley = ui.painter().layout_no_wrap(label.into(), theme::bold_font(15.0), ink);
                let left = rect.center().x - (16.0 + 6.0 + galley.size().x) / 2.0;
                let icon_rect = egui::Rect::from_center_size(
                    egui::pos2(left + 8.0, rect.center().y),
                    egui::vec2(16.0, 16.0),
                );
                icons::paint(ui, icon_rect, icon, ink);
                let pos = egui::pos2(left + 22.0, rect.center().y - galley.size().y / 2.0);
                ui.painter().galley(pos, galley, ink);
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
        tr("Switched to Pro mode: every control is shown. Your song is unchanged.").into()
    } else {
        tr("Switched to Easy mode: just the essentials. Your song is unchanged.").into()
    };
}

/// The EN | 中文 language switch. Each label is written in its own language
/// so it can be found whichever one is showing.
pub fn lang_switch(ui: &mut egui::Ui, app: &mut App) {
    let frame = egui::Frame::new()
        .fill(theme::CANVAS)
        .stroke(Stroke::new(1.0_f32, theme::BORDER_HI))
        .corner_radius(CornerRadius::same(14))
        .inner_margin(egui::Margin::same(2));
    frame
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            ui.horizontal(|ui| {
                let mut langs = [(Lang::En, "EN"), (Lang::Zh, "中文")];
                // Keep EN on the left inside right-to-left layouts too.
                if ui.layout().prefer_right_to_left() {
                    langs.reverse();
                }
                for (lang, label) in langs {
                    let on = i18n::get() == lang;
                    let (rect, r) = ui.allocate_exact_size(egui::vec2(40.0, 24.0), Sense::click());
                    let fill = if on {
                        theme::SURFACE_HI
                    } else if r.hovered() {
                        theme::SURFACE
                    } else {
                        Color32::TRANSPARENT
                    };
                    ui.painter().rect_filled(rect, CornerRadius::same(12), fill);
                    if r.hovered() && !on {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    }
                    ui.painter().text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        label,
                        theme::bold_font(12.5),
                        if on { theme::TEXT } else { theme::TEXT_DIM },
                    );
                    if r.clicked() && !on {
                        set_lang(app, lang);
                    }
                }
            });
        })
        .response
        .on_hover_text(tr("Language: English / 中文"));
}

/// Switch the interface language and remember it.
pub fn set_lang(app: &mut App, lang: Lang) {
    i18n::set(lang);
    app.settings.lang = Some(lang);
    app.settings.save();
    app.status = match lang {
        Lang::En => tr("Language: English"),
        Lang::Zh => tr("Language: 中文"),
    }
    .into();
}
