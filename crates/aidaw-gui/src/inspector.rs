//! Left panel: mixer strip per track and the selected track's instrument settings.

use aidaw_core::edit::Op;
use aidaw_core::{Instrument, Waveform};
use eframe::egui::{self, RichText};

use crate::app::{track_color, App};

pub fn show(ui: &mut egui::Ui, app: &mut App) {
    ui.horizontal(|ui| {
        ui.heading("Tracks");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("+ Drums").clicked() {
                app.add_track(true);
            }
            if ui.button("+ Synth").clicked() {
                app.add_track(false);
            }
        });
    });
    ui.separator();

    egui::ScrollArea::vertical()
        .id_salt("tracks")
        .max_height(ui.available_height() * 0.5)
        .show(ui, |ui| {
            let mut remove = None;
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
                                .clicked()
                            {
                                app.selected_track = Some(track.id.clone());
                            }
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui.small_button("✖").on_hover_text("Delete track").clicked()
                                    {
                                        remove = Some(track.id.clone());
                                    }
                                    ui.toggle_value(&mut track.solo, "S").on_hover_text("Solo");
                                    ui.toggle_value(&mut track.mute, "M").on_hover_text("Mute");
                                },
                            );
                        });
                        ui.horizontal(|ui| {
                            ui.label("Vol");
                            ui.add(egui::Slider::new(&mut track.gain, 0.0..=1.5).show_value(false));
                            ui.label("Pan");
                            ui.add(egui::Slider::new(&mut track.pan, -1.0..=1.0).show_value(false));
                        });
                    });
            }
            if let Some(id) = remove {
                app.run_op(Op::RemoveTrack { track_id: id });
            }
        });

    ui.separator();
    ui.horizontal(|ui| {
        ui.label("Master");
        ui.add(egui::Slider::new(&mut app.project.master_gain, 0.0..=1.5));
    });
    ui.separator();

    let Some(track) = app
        .selected_track
        .clone()
        .and_then(|id| app.project.track_mut(&id))
    else {
        ui.weak("Select a track");
        return;
    };
    ui.heading("Instrument");
    ui.horizontal(|ui| {
        ui.label("Name");
        ui.text_edit_singleline(&mut track.name);
    });
    let current = match &track.instrument {
        Instrument::Drums => "Drum kit".to_string(),
        Instrument::Synth { waveform, .. } => format!("Synth ({})", waveform.label()),
    };
    egui::ComboBox::from_label("Type")
        .selected_text(current)
        .show_ui(ui, |ui| {
            if ui
                .selectable_label(track.instrument.is_drums(), "Drum kit")
                .clicked()
            {
                track.instrument = Instrument::Drums;
            }
            for w in Waveform::ALL {
                let is =
                    matches!(track.instrument, Instrument::Synth { waveform, .. } if waveform == w);
                if ui
                    .selectable_label(is, format!("Synth ({})", w.label()))
                    .clicked()
                {
                    match &mut track.instrument {
                        Instrument::Synth { waveform, .. } => *waveform = w,
                        other => {
                            *other = Instrument::default_synth();
                            if let Instrument::Synth { waveform, .. } = other {
                                *waveform = w;
                            }
                        }
                    }
                }
            }
        });
    match &mut track.instrument {
        Instrument::Synth {
            attack,
            decay,
            sustain,
            release,
            cutoff,
            ..
        } => {
            egui::Grid::new("adsr").num_columns(2).show(ui, |ui| {
                ui.label("Attack");
                ui.add(
                    egui::Slider::new(attack, 0.001..=2.0)
                        .logarithmic(true)
                        .suffix(" s"),
                );
                ui.end_row();
                ui.label("Decay");
                ui.add(
                    egui::Slider::new(decay, 0.001..=3.0)
                        .logarithmic(true)
                        .suffix(" s"),
                );
                ui.end_row();
                ui.label("Sustain");
                ui.add(egui::Slider::new(sustain, 0.0..=1.0));
                ui.end_row();
                ui.label("Release");
                ui.add(
                    egui::Slider::new(release, 0.005..=4.0)
                        .logarithmic(true)
                        .suffix(" s"),
                );
                ui.end_row();
                ui.label("Cutoff");
                ui.add(
                    egui::Slider::new(cutoff, 40.0..=20_000.0)
                        .logarithmic(true)
                        .suffix(" Hz"),
                );
                ui.end_row();
            });
        }
        Instrument::Drums => {
            ui.weak("GM drum map: 36 kick · 38 snare · 39 clap · 42 closed hat · 46 open hat · 41/45/48 toms · 49 crash");
        }
    }
}
