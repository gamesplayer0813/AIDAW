//! Left panel: one strip per track (volume, mute, solo) and the selected
//! track's sound, chosen from friendly presets with the raw knobs tucked away.

use aidaw_core::edit::Op;
use aidaw_core::{Instrument, Waveform};
use eframe::egui::{self, RichText};

use crate::app::{track_color, App};
use crate::presets;

pub fn show(ui: &mut egui::Ui, app: &mut App) {
    app.ui_rects.insert("inspector", ui.max_rect());
    ui.horizontal(|ui| {
        ui.heading("Tracks");
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
                egui::Frame::group(ui.style())
                    .fill(if selected {
                        ui.visuals().selection.bg_fill.gamma_multiply(0.35)
                    } else {
                        ui.visuals().panel_fill
                    })
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let (r, _) =
                                ui.allocate_exact_size(egui::vec2(6.0, 18.0), egui::Sense::hover());
                            ui.painter().rect_filled(r, 2.0, track_color(i));
                            if ui
                                .selectable_label(selected, RichText::new(&track.name).strong())
                                .on_hover_text("Select this track to change its sound")
                                .clicked()
                            {
                                app.selected_track = Some(track.id.clone());
                                preview = Some(i);
                            }
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui
                                        .small_button("🗑")
                                        .on_hover_text("Delete this track (Ctrl+Z brings it back)")
                                        .clicked()
                                    {
                                        remove = Some(track.id.clone());
                                    }
                                    ui.toggle_value(&mut track.solo, "S")
                                        .on_hover_text("Solo: hear ONLY this track");
                                    ui.toggle_value(&mut track.mute, "M")
                                        .on_hover_text("Mute: silence this track");
                                },
                            );
                        });
                        ui.horizontal(|ui| {
                            ui.label("🔉").on_hover_text("Volume");
                            ui.add(egui::Slider::new(&mut track.gain, 0.0..=1.5).show_value(false))
                                .on_hover_text("Volume: how loud this track is");
                            ui.label("L/R").on_hover_text("Pan");
                            ui.add(egui::Slider::new(&mut track.pan, -1.0..=1.0).show_value(false))
                                .on_hover_text("Pan: move the sound to the left or right speaker");
                        });
                    });
            }
            if let Some(id) = remove {
                app.run_op(Op::RemoveTrack { track_id: id });
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
    ui.horizontal(|ui| {
        ui.label("Master volume")
            .on_hover_text("Volume of the whole song");
        ui.add(egui::Slider::new(&mut app.project.master_gain, 0.0..=1.5).show_value(false));
    });
    ui.separator();

    let Some(index) = app
        .selected_track
        .as_deref()
        .and_then(|id| app.project.tracks.iter().position(|t| t.id == id))
    else {
        ui.weak("Click a track above to change its sound.");
        return;
    };

    let sound = ui.scope(|ui| {
        ui.heading("Sound");
        let track = &mut app.project.tracks[index];
        ui.horizontal(|ui| {
            ui.label("Track name");
            ui.text_edit_singleline(&mut track.name);
        });
        let current = presets::name_of(&track.instrument).unwrap_or("⚙ Custom");
        let mut picked = false;
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt("preset")
                .selected_text(RichText::new(current).strong())
                .width(170.0)
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
            egui::CollapsingHeader::new("⚙ Fine-tune the sound (advanced)").show(ui, |ui| {
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
