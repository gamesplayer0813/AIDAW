//! Left panel: one strip per track (volume, mute, solo) and the selected
//! track's sound, chosen from friendly presets with the raw knobs tucked away.

use aidaw_core::edit::Op;
use aidaw_core::{DrumKit, Instrument, Waveform};
use eframe::egui::{self, RichText};

use crate::app::{track_color, App};
use crate::theme;
use aidaw_core::presets::{self, Category, Preset};

pub fn show(ui: &mut egui::Ui, app: &mut App) {
    app.ui_rects.insert("inspector", ui.max_rect());
    let pro = app.pro();
    ui.horizontal(|ui| {
        theme::section(ui, if pro { "Tracks" } else { "Instruments" });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.menu_button(RichText::new("➕ Add instrument").strong(), |ui| {
                ui.label(RichText::new("Pick a sound for the new track").weak());
                for c in Category::ALL {
                    ui.menu_button(c.label(), |ui| {
                        for p in Preset::in_category(c) {
                            if ui.button(p.name).on_hover_text(p.hint).clicked() {
                                app.add_track(p);
                                ui.close_menu();
                            }
                        }
                    });
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
                    ui.horizontal(|ui| {
                        ui.label("🔉").on_hover_text("Volume");
                        if pro {
                            ui.add(egui::Slider::new(&mut track.gain, 0.0..=1.5).show_value(true))
                                .on_hover_text("Volume: how loud this track is (1.0 = unchanged)");
                            ui.label("L/R").on_hover_text("Pan");
                            ui.add(egui::Slider::new(&mut track.pan, -1.0..=1.0).show_value(true))
                                .on_hover_text("Pan: move the sound to the left or right speaker");
                            if ui
                                .small_button("mid")
                                .on_hover_text("Centre the pan")
                                .clicked()
                            {
                                track.pan = 0.0;
                            }
                        } else {
                            ui.spacing_mut().slider_width = ui.available_width() - 8.0;
                            ui.add(egui::Slider::new(&mut track.gain, 0.0..=1.5).show_value(false))
                                .on_hover_text("How loud this instrument is");
                        }
                    });
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
            ui.add(egui::Slider::new(&mut app.project.master_gain, 0.0..=1.5).show_value(true));
        });
        ui.separator();
    }

    let Some(index) = app
        .selected_track
        .as_deref()
        .and_then(|id| app.project.tracks.iter().position(|t| t.id == id))
    else {
        ui.weak("Click a track above to change its sound.");
        return;
    };

    // The sound section can be taller than the window (many presets, many
    // knobs), so it scrolls on its own.
    egui::ScrollArea::vertical()
        .id_salt("sound")
        .show(ui, |ui| sound_section(ui, app, index, pro));
}

/// The selected track's name, preset picker and synth knobs.
fn sound_section(ui: &mut egui::Ui, app: &mut App, index: usize, pro: bool) {
    let sound = ui.scope(|ui| {
        theme::section(ui, "Sound");
        let track = &mut app.project.tracks[index];
        ui.horizontal(|ui| {
            ui.label("Track name");
            ui.text_edit_singleline(&mut track.name);
        });
        let current = presets::name_of(&track.instrument).unwrap_or("⚙ Custom");
        let mut picked = false;
        if !pro {
            ui.label(RichText::new("Pick a sound (click to hear it):").color(theme::TEXT_DIM));
            // Which category tab is open; starts on the current sound's.
            let tab_id = ui.id().with("sound_tab").with(index);
            let mut tab = ui
                .data(|d| d.get_temp::<usize>(tab_id))
                .or_else(|| {
                    let c = presets::of(&track.instrument)?.category;
                    Category::ALL.iter().position(|x| *x == c)
                })
                .unwrap_or(0);
            ui.horizontal_wrapped(|ui| {
                for (i, c) in Category::ALL.into_iter().enumerate() {
                    if ui.selectable_label(tab == i, c.label()).clicked() {
                        tab = i;
                    }
                }
            });
            ui.data_mut(|d| d.insert_temp(tab_id, tab));
            ui.add_space(4.0);
            egui::Grid::new("preset_grid")
                .num_columns(2)
                .spacing([6.0, 6.0])
                .show(ui, |ui| {
                    for (i, p) in Preset::in_category(Category::ALL[tab]).enumerate() {
                        let is = track.instrument == p.instrument;
                        let b = egui::Button::new(RichText::new(p.name).size(14.5))
                            .selected(is)
                            .corner_radius(10)
                            .min_size(egui::vec2(126.0, 40.0));
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
                .width(170.0)
                .height(420.0)
                .show_ui(ui, |ui| {
                    for p in presets::all() {
                        if Preset::in_category(p.category).next().map(|f| f.id) == Some(p.id) {
                            ui.label(
                                RichText::new(p.category.label())
                                    .small()
                                    .color(theme::TEXT_DIM),
                            );
                        }
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
        if let Some(p) = presets::of(&track.instrument) {
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
        Instrument::Synth(synth) => {
            if !pro {
                return;
            }
            let slider = |ui: &mut egui::Ui, v: &mut f32, r: std::ops::RangeInclusive<f32>| {
                ui.add(egui::Slider::new(v, r));
            };
            let seconds = |ui: &mut egui::Ui, v: &mut f32, r: std::ops::RangeInclusive<f32>| {
                ui.add(egui::Slider::new(v, r).logarithmic(true).suffix(" s"));
            };
            egui::CollapsingHeader::new("⚙ Fine-tune the sound")
                .default_open(true)
                .show(ui, |ui| {
                    egui::Grid::new("adsr").num_columns(2).show(ui, |ui| {
                        ui.label("Wave").on_hover_text("The basic tone colour");
                        egui::ComboBox::from_id_salt("wave")
                            .selected_text(synth.waveform.label())
                            .show_ui(ui, |ui| {
                                for w in Waveform::ALL {
                                    ui.selectable_value(&mut synth.waveform, w, w.label());
                                }
                            });
                        ui.end_row();
                        ui.label("Fade in")
                            .on_hover_text("Attack: how slowly the note starts");
                        seconds(ui, &mut synth.attack, 0.001..=2.0);
                        ui.end_row();
                        ui.label("Drop")
                            .on_hover_text("Decay: how fast it falls to the hold level");
                        seconds(ui, &mut synth.decay, 0.001..=3.0);
                        ui.end_row();
                        ui.label("Hold level")
                            .on_hover_text("Sustain: loudness while the note is held");
                        slider(ui, &mut synth.sustain, 0.0..=1.0);
                        ui.end_row();
                        ui.label("Fade out")
                            .on_hover_text("Release: how long it rings after the note ends");
                        seconds(ui, &mut synth.release, 0.005..=4.0);
                        ui.end_row();
                        ui.label("Brightness")
                            .on_hover_text("Filter cutoff: lower = darker, softer");
                        ui.add(
                            egui::Slider::new(&mut synth.cutoff, 40.0..=20_000.0)
                                .logarithmic(true)
                                .suffix(" Hz"),
                        );
                        ui.end_row();
                    });
                });
            egui::CollapsingHeader::new("🔊 More sound shaping")
                .default_open(false)
                .show(ui, |ui| {
                    egui::Grid::new("synth_more").num_columns(2).show(ui, |ui| {
                        ui.label("Resonance")
                            .on_hover_text("A whistling peak at the brightness point (acid, wah)");
                        slider(ui, &mut synth.resonance, 0.0..=1.0);
                        ui.end_row();
                        ui.label("Filter sweep").on_hover_text(
                            "How much brighter each note starts, in octaves (plucks, brass)",
                        );
                        slider(ui, &mut synth.filter_env, 0.0..=8.0);
                        ui.end_row();
                        ui.label("Sweep time")
                            .on_hover_text("How fast the filter sweep closes again");
                        seconds(ui, &mut synth.filter_decay, 0.005..=4.0);
                        ui.end_row();
                        ui.label("Voices").on_hover_text(
                            "Unison: stack detuned copies for a wider, fatter sound",
                        );
                        let mut n = synth.unison.unwrap_or(match synth.waveform {
                            Waveform::Saw => 2,
                            _ => 1,
                        });
                        if ui.add(egui::Slider::new(&mut n, 1..=5)).changed() {
                            synth.unison = Some(n);
                        }
                        ui.end_row();
                        ui.label("Detune")
                            .on_hover_text("How far apart the stacked voices are, in cents");
                        let mut d = synth.detune.unwrap_or(14.0);
                        if ui
                            .add(egui::Slider::new(&mut d, 0.0..=60.0).suffix(" ct"))
                            .changed()
                        {
                            synth.detune = Some(d);
                        }
                        ui.end_row();
                        ui.label("Sub")
                            .on_hover_text("Add a sine one octave lower: more weight");
                        slider(ui, &mut synth.sub, 0.0..=1.0);
                        ui.end_row();
                        ui.label("Noise")
                            .on_hover_text("Mix in hiss: breath, air, grit");
                        slider(ui, &mut synth.noise, 0.0..=1.0);
                        ui.end_row();
                        ui.label("FM")
                            .on_hover_text("Bell / electric-piano tone (sine and triangle waves)");
                        slider(ui, &mut synth.fm, 0.0..=10.0);
                        ui.end_row();
                        ui.label("FM ratio")
                            .on_hover_text("1 = warm, 2 = hollow, 3.5+ = metallic bell");
                        slider(ui, &mut synth.fm_ratio, 0.5..=8.0);
                        ui.end_row();
                        ui.label("FM fade")
                            .on_hover_text("How fast the FM tone fades (0 = never)");
                        slider(ui, &mut synth.fm_decay, 0.0..=4.0);
                        ui.end_row();
                        ui.label("Vibrato")
                            .on_hover_text("Wobble in the pitch, in cents");
                        slider(ui, &mut synth.vibrato, 0.0..=100.0);
                        ui.end_row();
                        ui.label("Vibrato speed");
                        ui.add(
                            egui::Slider::new(&mut synth.vibrato_rate, 0.1..=12.0).suffix(" Hz"),
                        );
                        ui.end_row();
                        ui.label("Pitch drop").on_hover_text(
                            "Start this many semitones higher and slide down (808, zaps)",
                        );
                        slider(ui, &mut synth.pitch_env, -24.0..=36.0);
                        ui.end_row();
                        ui.label("Drop time");
                        seconds(ui, &mut synth.pitch_decay, 0.005..=2.0);
                        ui.end_row();
                        ui.label("Drive")
                            .on_hover_text("Warmth at low values, distortion at high ones");
                        slider(ui, &mut synth.drive, 0.0..=1.0);
                        ui.end_row();
                    });
                });
        }
        Instrument::Drums { kit } => {
            if pro {
                ui.horizontal(|ui| {
                    ui.label("Kit");
                    egui::ComboBox::from_id_salt("drum_kit")
                        .selected_text(kit.label())
                        .show_ui(ui, |ui| {
                            for k in DrumKit::ALL {
                                ui.selectable_value(kit, k, k.label());
                            }
                        });
                });
            }
            ui.weak("Each row in the piano roll is a different drum: Kick, Snare, Clap, Hi-hats, Toms, cymbals and percussion (cowbell, shaker, congas...) are labelled on the keyboard.");
        }
    }
}
