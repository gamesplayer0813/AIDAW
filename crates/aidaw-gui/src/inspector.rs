//! Left panel: one strip per track (volume, mute, solo) and the selected
//! track's sound, chosen from friendly presets with the raw knobs tucked away.

use aidaw_core::edit::Op;
use aidaw_core::{Instrument, Waveform};
use eframe::egui::{self, RichText};

use crate::app::{track_color, App};
use crate::theme;
use aidaw_core::presets;

pub fn show(ui: &mut egui::Ui, app: &mut App) {
    app.ui_rects.insert("inspector", ui.max_rect());
    let pro = app.pro();
    ui.horizontal(|ui| {
        theme::section(ui, if pro { "Tracks" } else { "Instruments" });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.menu_button(RichText::new("➕ Add instrument").strong(), |ui| {
                ui.label(RichText::new("Pick a sound for the new track").weak());
                for p in presets::all() {
                    if ui.button(p.name).on_hover_text(p.hint).clicked() {
                        app.add_track(&p);
                        ui.close_menu();
                    }
                }
            })
            .response
            .on_hover_text("Add a new track. Each track plays one instrument.");
        });
    });
    ui.separator();

    if app.project.tracks.is_empty() {
        ui.add_space(8.0);
        ui.label("No tracks yet. Click ➕ Add instrument above to start.");
    }

    egui::ScrollArea::vertical()
        .id_salt("tracks")
        .max_height(ui.available_height() * 0.45)
        .show(ui, |ui| {
            let mut remove = None;
            let mut preview = None;
            for (i, track) in app.project.tracks.iter_mut().enumerate() {
                let selected = app.selected_track.as_deref() == Some(track.id.as_str());
                let color = track_color(i);
                let (fill, stroke) = if selected {
                    (
                        theme::mix(theme::SURFACE, color, 0.14),
                        egui::Stroke::new(1.0_f32, theme::alpha(color, 170)),
                    )
                } else {
                    (theme::SURFACE, egui::Stroke::new(1.0_f32, theme::BORDER))
                };
                theme::card(fill, stroke).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        let (r, _) =
                            ui.allocate_exact_size(egui::vec2(4.0, 18.0), egui::Sense::hover());
                        ui.painter().rect_filled(r, 2.0, color);
                        if ui
                            .selectable_label(
                                selected,
                                RichText::new(&track.name).font(theme::bold_font(14.0)),
                            )
                            .on_hover_text("Select this track to change its sound")
                            .clicked()
                        {
                            app.selected_track = Some(track.id.clone());
                            preview = Some(i);
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            // Only on the selected track, so a stray click can't delete one.
                            if selected
                                && ui
                                    .small_button("🗑")
                                    .on_hover_text("Delete this track (Ctrl+Z brings it back)")
                                    .clicked()
                            {
                                remove = Some((track.id.clone(), track.name.clone()));
                            }
                            if pro {
                                ui.toggle_value(&mut track.solo, "S")
                                    .on_hover_text("Solo: hear ONLY this track");
                                ui.toggle_value(&mut track.mute, "M")
                                    .on_hover_text("Mute: silence this track");
                            } else {
                                ui.toggle_value(&mut track.mute, "🔇 Silence")
                                    .on_hover_text("Turn this instrument off for now");
                            }
                        });
                    });
                    if pro {
                        // Volume and pan share one row when there is room, else one row each.
                        let wide = ui.available_width() >= 420.0;
                        let per_row = if wide { 2.0 } else { 1.0 };
                        let slider = ((ui.available_width() - 110.0 * per_row - 50.0) / per_row)
                            .clamp(40.0, 140.0);
                        let volume = |ui: &mut egui::Ui, gain: &mut f32| {
                            ui.label("🔉").on_hover_text("Volume");
                            ui.spacing_mut().slider_width = slider;
                            ui.add(egui::Slider::new(gain, 0.0..=1.5).show_value(true))
                                .on_hover_text("Volume: how loud this track is (1.0 = unchanged)");
                        };
                        let pan = |ui: &mut egui::Ui, pan: &mut f32| {
                            ui.label("L/R").on_hover_text("Pan");
                            ui.spacing_mut().slider_width = slider;
                            ui.add(egui::Slider::new(pan, -1.0..=1.0).show_value(true))
                                .on_hover_text("Pan: move the sound to the left or right speaker");
                            if ui
                                .small_button("mid")
                                .on_hover_text("Centre the pan")
                                .clicked()
                            {
                                *pan = 0.0;
                            }
                        };
                        if wide {
                            ui.horizontal(|ui| {
                                volume(ui, &mut track.gain);
                                pan(ui, &mut track.pan);
                            });
                        } else {
                            ui.horizontal(|ui| volume(ui, &mut track.gain));
                            ui.horizontal(|ui| pan(ui, &mut track.pan));
                        }
                    } else {
                        ui.horizontal(|ui| {
                            ui.label("🔉").on_hover_text("Volume");
                            ui.spacing_mut().slider_width = ui.available_width() - 8.0;
                            ui.add(egui::Slider::new(&mut track.gain, 0.0..=1.5).show_value(false))
                                .on_hover_text("How loud this instrument is");
                        });
                    }
                });
                ui.add_space(2.0);
            }
            if let Some((id, name)) = remove {
                if app.run_op(Op::RemoveTrack { track_id: id }).is_some() {
                    app.status = format!("Deleted {name}. Ctrl+Z (or ↩ Undo) brings it back.");
                }
            }
            if let Some(i) = preview {
                app.preview_note(
                    i,
                    if app.project.tracks[i].instrument.is_drums() {
                        36
                    } else {
                        60
                    },
                );
            }
        });

    ui.separator();
    if pro {
        ui.horizontal(|ui| {
            ui.label("Master volume")
                .on_hover_text("Volume of the whole song");
            ui.spacing_mut().slider_width = (ui.available_width() - 70.0).clamp(40.0, 140.0);
            ui.add(egui::Slider::new(&mut app.project.master_gain, 0.0..=1.5).show_value(true));
        });
        ui.separator();
    }

    egui::ScrollArea::vertical()
        .id_salt("sound")
        .auto_shrink([false, false])
        .show(ui, |ui| sound_section(ui, app));
}

/// The selected track's name and sound (scrolls when the window is short).
fn sound_section(ui: &mut egui::Ui, app: &mut App) {
    let pro = app.pro();
    let Some(index) = app
        .selected_track
        .as_deref()
        .and_then(|id| app.project.tracks.iter().position(|t| t.id == id))
    else {
        ui.weak("Click a track above to change its sound.");
        return;
    };

    let sound = ui.scope(|ui| {
        theme::section(ui, "Sound");
        let track = &mut app.project.tracks[index];
        ui.horizontal(|ui| {
            ui.label("Track name");
            ui.add(egui::TextEdit::singleline(&mut track.name).desired_width(f32::INFINITY));
        });
        let current = presets::name_of(&track.instrument).unwrap_or("⚙ Custom");
        let mut picked = false;
        if !pro {
            ui.label(RichText::new("Pick a sound (click to hear it):").color(theme::TEXT_DIM));
            let cell = ((ui.available_width() - 6.0) / 2.0).clamp(90.0, 160.0);
            egui::Grid::new("preset_grid")
                .num_columns(2)
                .spacing([6.0, 6.0])
                .show(ui, |ui| {
                    for (i, p) in presets::all().into_iter().enumerate() {
                        let is = track.instrument == p.instrument;
                        let b = egui::Button::new(RichText::new(p.name).size(14.5))
                            .selected(is)
                            .corner_radius(10)
                            .min_size(egui::vec2(cell, 40.0));
                        if ui.add(b).on_hover_text(p.hint).clicked() {
                            track.instrument = p.instrument.clone();
                            picked = true;
                        }
                        if i % 2 == 1 {
                            ui.end_row();
                        }
                    }
                });
            return picked;
        }
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt("preset")
                .selected_text(RichText::new(current).strong())
                .width((ui.available_width() - 90.0).clamp(90.0, 170.0))
                .show_ui(ui, |ui| {
                    for p in presets::all() {
                        let is = track.instrument == p.instrument;
                        if ui
                            .selectable_label(is, p.name)
                            .on_hover_text(p.hint)
                            .clicked()
                            && !is
                        {
                            track.instrument = p.instrument.clone();
                            picked = true;
                        }
                    }
                });
            if ui
                .button("🔊 Try")
                .on_hover_text("Hear this sound")
                .clicked()
            {
                picked = true;
            }
        });
        if let Some(p) = presets::all()
            .into_iter()
            .find(|p| p.instrument == track.instrument)
        {
            ui.weak(p.hint);
        }
        picked
    });
    app.ui_rects.insert("sound", sound.response.rect);
    if sound.inner {
        let drums = app.project.tracks[index].instrument.is_drums();
        app.preview_note(index, if drums { 38 } else { 60 });
    }

    let track = &mut app.project.tracks[index];
    ui.add_space(6.0);
    match &mut track.instrument {
        Instrument::Synth {
            waveform,
            attack,
            decay,
            sustain,
            release,
            cutoff,
        } => {
            if !pro {
                return;
            }
            egui::CollapsingHeader::new("⚙ Fine-tune the sound")
                .default_open(true)
                .show(ui, |ui| {
                    ui.spacing_mut().slider_width =
                        (ui.available_width() - 160.0).clamp(50.0, 160.0);
                    egui::Grid::new("adsr").num_columns(2).show(ui, |ui| {
                        ui.label("Wave").on_hover_text("The basic tone colour");
                        egui::ComboBox::from_id_salt("wave")
                            .selected_text(waveform.label())
                            .show_ui(ui, |ui| {
                                for w in Waveform::ALL {
                                    ui.selectable_value(waveform, w, w.label());
                                }
                            });
                        ui.end_row();
                        ui.label("Fade in")
                            .on_hover_text("Attack: how slowly the note starts");
                        ui.add(
                            egui::Slider::new(attack, 0.001..=2.0)
                                .logarithmic(true)
                                .suffix(" s"),
                        );
                        ui.end_row();
                        ui.label("Drop")
                            .on_hover_text("Decay: how fast it falls to the hold level");
                        ui.add(
                            egui::Slider::new(decay, 0.001..=3.0)
                                .logarithmic(true)
                                .suffix(" s"),
                        );
                        ui.end_row();
                        ui.label("Hold level")
                            .on_hover_text("Sustain: loudness while the note is held");
                        ui.add(egui::Slider::new(sustain, 0.0..=1.0));
                        ui.end_row();
                        ui.label("Fade out")
                            .on_hover_text("Release: how long it rings after the note ends");
                        ui.add(
                            egui::Slider::new(release, 0.005..=4.0)
                                .logarithmic(true)
                                .suffix(" s"),
                        );
                        ui.end_row();
                        ui.label("Brightness")
                            .on_hover_text("Filter cutoff: lower = darker, softer");
                        ui.add(
                            egui::Slider::new(cutoff, 40.0..=20_000.0)
                                .logarithmic(true)
                                .suffix(" Hz"),
                        );
                        ui.end_row();
                    });
                });
        }
        Instrument::Drums => {
            ui.weak("Each row in the piano roll is a different drum: Kick, Snare, Clap, Hi-hats, Toms and Crash are labelled on the keyboard.");
        }
    }
}
