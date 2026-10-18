//! Left panel: one strip per track (volume, mute, solo) and the selected
//! track's sound, chosen from friendly presets with the raw knobs tucked away.

use aidaw_core::edit::Op;
use aidaw_core::{DrumKit, Instrument, Waveform};
use eframe::egui::{self, RichText};

use crate::app::{track_color, App};
use crate::i18n::{tr, trf};
use crate::theme;
use aidaw_core::presets::{self, Category, Preset};

pub fn show(ui: &mut egui::Ui, app: &mut App) {
    app.ui_rects.insert("inspector", ui.max_rect());
    let pro = app.pro();
    ui.horizontal(|ui| {
        theme::section(ui, if pro { tr("Tracks") } else { tr("Instruments") });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.menu_button(RichText::new(tr("➕ Add instrument")).strong(), |ui| {
                ui.label(RichText::new(tr("Pick a sound for the new track")).weak());
                for c in Category::ALL {
                    ui.menu_button(tr(c.label()), |ui| {
                        for p in Preset::in_category(c) {
                            if ui.button(tr(p.name)).on_hover_text(tr(p.hint)).clicked() {
                                app.add_track(p);
                                ui.close_menu();
                            }
                        }
                    });
                }
            })
            .response
            .on_hover_text(tr("Add a new track. Each track plays one instrument."));
        });
    });
    ui.separator();

    if app.project.tracks.is_empty() {
        ui.add_space(8.0);
        ui.label(tr("No tracks yet. Click ➕ Add instrument above to start."));
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
                            .on_hover_text(tr("Select this track to change its sound"))
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
                                    .on_hover_text(tr("Delete this track (Ctrl+Z brings it back)"))
                                    .clicked()
                            {
                                remove = Some((track.id.clone(), track.name.clone()));
                            }
                            if pro {
                                ui.toggle_value(&mut track.solo, "S")
                                    .on_hover_text(tr("Solo: hear ONLY this track"));
                                ui.toggle_value(&mut track.mute, "M")
                                    .on_hover_text(tr("Mute: silence this track"));
                            } else {
                                ui.toggle_value(&mut track.mute, tr("🔇 Silence"))
                                    .on_hover_text(tr("Turn this instrument off for now"));
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
                            ui.label("🔉").on_hover_text(tr("Volume"));
                            ui.spacing_mut().slider_width = slider;
                            ui.add(egui::Slider::new(gain, 0.0..=1.5).show_value(true))
                                .on_hover_text(tr(
                                    "Volume: how loud this track is (1.0 = unchanged)",
                                ));
                        };
                        let pan = |ui: &mut egui::Ui, pan: &mut f32| {
                            ui.label(tr("L/R")).on_hover_text(tr("Pan"));
                            ui.spacing_mut().slider_width = slider;
                            ui.add(egui::Slider::new(pan, -1.0..=1.0).show_value(true))
                                .on_hover_text(tr(
                                    "Pan: move the sound to the left or right speaker",
                                ));
                            if ui
                                .small_button(tr("mid"))
                                .on_hover_text(tr("Centre the pan"))
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
                            ui.label("🔉").on_hover_text(tr("Volume"));
                            ui.spacing_mut().slider_width = ui.available_width() - 8.0;
                            ui.add(egui::Slider::new(&mut track.gain, 0.0..=1.5).show_value(false))
                                .on_hover_text(tr("How loud this instrument is"));
                        });
                    }
                });
                ui.add_space(2.0);
            }
            if let Some((id, name)) = remove {
                if app.run_op(Op::RemoveTrack { track_id: id }).is_some() {
                    app.status = trf("Deleted {}. Ctrl+Z (or ↩ Undo) brings it back.", &[&name]);
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
            ui.label(tr("Master volume"))
                .on_hover_text(tr("Volume of the whole song"));
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
        ui.weak(tr("Click a track above to change its sound."));
        return;
    };

    let sound = ui.scope(|ui| {
        theme::section(ui, tr("Sound"));
        let track = &mut app.project.tracks[index];
        ui.horizontal(|ui| {
            ui.label(tr("Track name"));
            ui.add(egui::TextEdit::singleline(&mut track.name).desired_width(f32::INFINITY));
        });
        let current = presets::name_of(&track.instrument).map_or(tr("⚙ Custom"), tr);
        let mut picked = false;
        if !pro {
            ui.label(RichText::new(tr("Pick a sound (click to hear it):")).color(theme::TEXT_DIM));
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
                    if ui.selectable_label(tab == i, tr(c.label())).clicked() {
                        tab = i;
                    }
                }
            });
            ui.data_mut(|d| d.insert_temp(tab_id, tab));
            ui.add_space(4.0);
            let cell = ((ui.available_width() - 6.0) / 2.0).clamp(90.0, 160.0);
            egui::Grid::new("preset_grid")
                .num_columns(2)
                .spacing([6.0, 6.0])
                .show(ui, |ui| {
                    for (i, p) in Preset::in_category(Category::ALL[tab]).enumerate() {
                        let is = track.instrument == p.instrument;
                        let b = egui::Button::new(RichText::new(tr(p.name)).size(14.5))
                            .selected(is)
                            .corner_radius(10)
                            .min_size(egui::vec2(cell, 40.0));
                        if ui.add(b).on_hover_text(tr(p.hint)).clicked() {
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
                .height(420.0)
                .show_ui(ui, |ui| {
                    for p in presets::all() {
                        if Preset::in_category(p.category).next().map(|f| f.id) == Some(p.id) {
                            ui.label(
                                RichText::new(tr(p.category.label()))
                                    .small()
                                    .color(theme::TEXT_DIM),
                            );
                        }
                        let is = track.instrument == p.instrument;
                        if ui
                            .selectable_label(is, tr(p.name))
                            .on_hover_text(tr(p.hint))
                            .clicked()
                            && !is
                        {
                            track.instrument = p.instrument.clone();
                            picked = true;
                        }
                    }
                });
            if ui
                .button(tr("🔊 Try"))
                .on_hover_text(tr("Hear this sound"))
                .clicked()
            {
                picked = true;
            }
        });
        if let Some(p) = presets::of(&track.instrument) {
            ui.weak(tr(p.hint));
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
            egui::CollapsingHeader::new(tr("⚙ Fine-tune the sound"))
                .default_open(true)
                .show(ui, |ui| {
                    ui.spacing_mut().slider_width =
                        (ui.available_width() - 160.0).clamp(50.0, 160.0);
                    egui::Grid::new("adsr").num_columns(2).show(ui, |ui| {
                        ui.label(tr("Wave"))
                            .on_hover_text(tr("The basic tone colour"));
                        egui::ComboBox::from_id_salt("wave")
                            .selected_text(tr(synth.waveform.label()))
                            .show_ui(ui, |ui| {
                                for w in Waveform::ALL {
                                    ui.selectable_value(&mut synth.waveform, w, tr(w.label()));
                                }
                            });
                        ui.end_row();
                        ui.label(tr("Fade in"))
                            .on_hover_text(tr("Attack: how slowly the note starts"));
                        seconds(ui, &mut synth.attack, 0.001..=2.0);
                        ui.end_row();
                        ui.label(tr("Drop"))
                            .on_hover_text(tr("Decay: how fast it falls to the hold level"));
                        seconds(ui, &mut synth.decay, 0.001..=3.0);
                        ui.end_row();
                        ui.label(tr("Hold level"))
                            .on_hover_text(tr("Sustain: loudness while the note is held"));
                        slider(ui, &mut synth.sustain, 0.0..=1.0);
                        ui.end_row();
                        ui.label(tr("Fade out"))
                            .on_hover_text(tr("Release: how long it rings after the note ends"));
                        seconds(ui, &mut synth.release, 0.005..=4.0);
                        ui.end_row();
                        ui.label(tr("Brightness"))
                            .on_hover_text(tr("Filter cutoff: lower = darker, softer"));
                        ui.add(
                            egui::Slider::new(&mut synth.cutoff, 40.0..=20_000.0)
                                .logarithmic(true)
                                .suffix(" Hz"),
                        );
                        ui.end_row();
                    });
                });
            egui::CollapsingHeader::new(tr("🔊 More sound shaping"))
                .default_open(false)
                .show(ui, |ui| {
                    ui.spacing_mut().slider_width =
                        (ui.available_width() - 160.0).clamp(50.0, 160.0);
                    egui::Grid::new("synth_more").num_columns(2).show(ui, |ui| {
                        ui.label(tr("Resonance")).on_hover_text(tr(
                            "A whistling peak at the brightness point (acid, wah)",
                        ));
                        slider(ui, &mut synth.resonance, 0.0..=1.0);
                        ui.end_row();
                        ui.label(tr("Filter sweep")).on_hover_text(tr(
                            "How much brighter each note starts, in octaves (plucks, brass)",
                        ));
                        slider(ui, &mut synth.filter_env, 0.0..=8.0);
                        ui.end_row();
                        ui.label(tr("Sweep time"))
                            .on_hover_text(tr("How fast the filter sweep closes again"));
                        seconds(ui, &mut synth.filter_decay, 0.005..=4.0);
                        ui.end_row();
                        ui.label(tr("Voices")).on_hover_text(tr(
                            "Unison: stack detuned copies for a wider, fatter sound",
                        ));
                        let mut n = synth.unison.unwrap_or(match synth.waveform {
                            Waveform::Saw => 2,
                            _ => 1,
                        });
                        if ui.add(egui::Slider::new(&mut n, 1..=5)).changed() {
                            synth.unison = Some(n);
                        }
                        ui.end_row();
                        ui.label(tr("Detune"))
                            .on_hover_text(tr("How far apart the stacked voices are, in cents"));
                        let mut d = synth.detune.unwrap_or(14.0);
                        if ui
                            .add(egui::Slider::new(&mut d, 0.0..=60.0).suffix(" ct"))
                            .changed()
                        {
                            synth.detune = Some(d);
                        }
                        ui.end_row();
                        ui.label(tr("Sub"))
                            .on_hover_text(tr("Add a sine one octave lower: more weight"));
                        slider(ui, &mut synth.sub, 0.0..=1.0);
                        ui.end_row();
                        ui.label(tr("Noise"))
                            .on_hover_text(tr("Mix in hiss: breath, air, grit"));
                        slider(ui, &mut synth.noise, 0.0..=1.0);
                        ui.end_row();
                        ui.label(tr("FM")).on_hover_text(tr(
                            "Bell / electric-piano tone (sine and triangle waves)",
                        ));
                        slider(ui, &mut synth.fm, 0.0..=10.0);
                        ui.end_row();
                        ui.label(tr("FM ratio"))
                            .on_hover_text(tr("1 = warm, 2 = hollow, 3.5+ = metallic bell"));
                        slider(ui, &mut synth.fm_ratio, 0.5..=8.0);
                        ui.end_row();
                        ui.label(tr("FM fade"))
                            .on_hover_text(tr("How fast the FM tone fades (0 = never)"));
                        slider(ui, &mut synth.fm_decay, 0.0..=4.0);
                        ui.end_row();
                        ui.label(tr("Vibrato"))
                            .on_hover_text(tr("Wobble in the pitch, in cents"));
                        slider(ui, &mut synth.vibrato, 0.0..=100.0);
                        ui.end_row();
                        ui.label(tr("Vibrato speed"));
                        ui.add(
                            egui::Slider::new(&mut synth.vibrato_rate, 0.1..=12.0).suffix(" Hz"),
                        );
                        ui.end_row();
                        ui.label(tr("Pitch drop")).on_hover_text(tr(
                            "Start this many semitones higher and slide down (808, zaps)",
                        ));
                        slider(ui, &mut synth.pitch_env, -24.0..=36.0);
                        ui.end_row();
                        ui.label(tr("Drop time"));
                        seconds(ui, &mut synth.pitch_decay, 0.005..=2.0);
                        ui.end_row();
                        ui.label(tr("Drive"))
                            .on_hover_text(tr("Warmth at low values, distortion at high ones"));
                        slider(ui, &mut synth.drive, 0.0..=1.0);
                        ui.end_row();
                    });
                });
        }
        Instrument::Drums { kit } => {
            if pro {
                ui.horizontal(|ui| {
                    ui.label(tr("Kit"));
                    egui::ComboBox::from_id_salt("drum_kit")
                        .selected_text(tr(kit.label()))
                        .show_ui(ui, |ui| {
                            for k in DrumKit::ALL {
                                ui.selectable_value(kit, k, tr(k.label()));
                            }
                        });
                });
            }
            ui.weak(tr("Each row in the piano roll is a different drum: Kick, Snare, Clap, Hi-hats, Toms, cymbals and percussion (cowbell, shaker, congas...) are labelled on the keyboard."));
        }
    }
}
