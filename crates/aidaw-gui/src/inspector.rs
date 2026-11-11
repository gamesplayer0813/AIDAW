//! Left panel: one strip per track (volume, mute, solo) and the selected
//! track's sound, chosen from friendly presets with the raw knobs tucked away.

use aidaw_core::edit::Op;
use aidaw_core::{DrumKit, Instrument, Param, PluginRef, Track, Waveform};
use aidaw_plugin::editor;
use aidaw_plugin::rack::Key;
use aidaw_plugin::scan::{Kind, PluginInfo};
use eframe::egui::{self, RichText, SliderClamping};

use crate::app::{track_color, App};
use crate::i18n::{tr, trf};
use crate::icons::{self, Icon};
use crate::theme;
use aidaw_core::presets::{self, Category, Preset};

/// A slider for a setting automation can move. While the setting has
/// points it shows the value at the playhead and can't be dragged (the lane
/// in the song view decides). Values outside the slider's range that the AI
/// set are kept as they are until the slider is moved.
fn knob(
    ui: &mut egui::Ui,
    value: &mut f32,
    automated: Option<f32>,
    make: impl for<'a> FnOnce(&'a mut f32) -> egui::Slider<'a>,
) -> egui::Response {
    match automated {
        None => ui.add(make(value).clamping(SliderClamping::Edits)),
        Some(v) => {
            let mut shown = v;
            ui.add_enabled(false, make(&mut shown).clamping(SliderClamping::Edits))
                .on_disabled_hover_text(tr(
                    "Automated: this follows its line in the song view. Clear or remove the lane to set it here.",
                ))
        }
    }
}

pub fn show(ui: &mut egui::Ui, app: &mut App) {
    app.ui_rects.insert("inspector", ui.max_rect());
    let playhead = app.playhead;
    let pro = app.pro();
    ui.horizontal(|ui| {
        theme::section(ui, if pro { tr("Tracks") } else { tr("Instruments") });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let plus = icons::image(Icon::Plus, 16.0, theme::TEXT);
            ui.menu_image_text_button(plus, RichText::new(tr("Add instrument")).strong(), |ui| {
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
        ui.label(tr("No tracks yet. Click + Add instrument above to start."));
    }

    egui::ScrollArea::vertical()
        .id_salt("tracks")
        .max_height(ui.available_height() * 0.45)
        .show(ui, |ui| {
            let mut remove = None;
            let mut preview = None;
            for (i, track) in app.project.tracks.iter_mut().enumerate() {
                let auto_gain = track.automated(Param::Volume, playhead);
                let auto_pan = track.automated(Param::Pan, playhead);
                let selected = app.selected_track.as_deref() == Some(track.id.as_str());
                let color = track_color(track);
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
                                    .add(icons::icon_button(Icon::Trash, 14.0).small())
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
                            icons::label(ui, Icon::Volume).on_hover_text(tr("Volume"));
                            ui.spacing_mut().slider_width = slider;
                            knob(ui, gain, auto_gain, |v| {
                                egui::Slider::new(v, 0.0..=1.5).show_value(true)
                            })
                            .on_hover_text(tr("Volume: how loud this track is (1.0 = unchanged)"));
                        };
                        let pan = |ui: &mut egui::Ui, pan: &mut f32| {
                            ui.label(tr("L/R")).on_hover_text(tr("Pan"));
                            ui.spacing_mut().slider_width = slider;
                            knob(ui, pan, auto_pan, |v| {
                                egui::Slider::new(v, -1.0..=1.0).show_value(true)
                            })
                            .on_hover_text(tr("Pan: move the sound to the left or right speaker"));
                            if ui
                                .add_enabled(
                                    auto_pan.is_none(),
                                    egui::Button::new(tr("mid")).small(),
                                )
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
                            icons::label(ui, Icon::Volume).on_hover_text(tr("Volume"));
                            ui.spacing_mut().slider_width = ui.available_width() - 8.0;
                            knob(ui, &mut track.gain, auto_gain, |v| {
                                egui::Slider::new(v, 0.0..=1.5).show_value(false)
                            })
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
        let auto = |p: Param| app.project.automated(p, playhead);
        let (auto_master, auto_size, auto_tone) = (
            auto(Param::MasterVolume),
            auto(Param::ReverbSize),
            auto(Param::ReverbTone),
        );
        ui.horizontal(|ui| {
            ui.label(tr("Master volume"))
                .on_hover_text(tr("Volume of the whole song"));
            ui.spacing_mut().slider_width = (ui.available_width() - 70.0).clamp(40.0, 140.0);
            knob(ui, &mut app.project.master_gain, auto_master, |v| {
                egui::Slider::new(v, 0.0..=1.5).show_value(true)
            });
        });
        ui.horizontal(|ui| {
            ui.label(tr("Reverb room"))
                .on_hover_text(tr("Size of the shared reverb: small room .. big hall"));
            ui.spacing_mut().slider_width = (ui.available_width() - 70.0).clamp(40.0, 140.0);
            knob(ui, &mut app.project.reverb.size, auto_size, |v| {
                egui::Slider::new(v, 0.0..=1.0).fixed_decimals(2)
            });
        });
        ui.horizontal(|ui| {
            ui.label(tr("Reverb tone"))
                .on_hover_text(tr("Dark, soft echoes .. bright, airy ones"));
            ui.spacing_mut().slider_width = (ui.available_width() - 70.0).clamp(40.0, 140.0);
            knob(ui, &mut app.project.reverb.tone, auto_tone, |v| {
                egui::Slider::new(v, 0.0..=1.0).fixed_decimals(2)
            });
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

    let playhead = app.playhead;
    let auto = |p: Param| app.project.tracks[index].automated(p, playhead);
    let auto_reverb = auto(Param::Reverb);
    let synth_auto = [
        Param::Cutoff,
        Param::Resonance,
        Param::Drive,
        Param::Fm,
        Param::Vibrato,
        Param::Sub,
        Param::Noise,
    ]
    .map(auto);
    let sound = ui.scope(|ui| {
        theme::section(ui, tr("Sound"));
        let track = &mut app.project.tracks[index];
        ui.horizontal(|ui| {
            ui.label(tr("Track name"));
            ui.add(egui::TextEdit::singleline(&mut track.name).desired_width(f32::INFINITY));
        });
        ui.horizontal(|ui| {
            ui.label(tr("Colour"))
                .on_hover_text(tr("The colour of this track's row and clips"));
            color_swatches(ui, track);
        });
        ui.horizontal(|ui| {
            ui.label(tr("Reverb")).on_hover_text(tr(
                "Echo of a room around this sound: 0 = dry and close, more = farther away and more spacious",
            ));
            ui.spacing_mut().slider_width =
                (ui.available_width() - if pro { 70.0 } else { 8.0 }).clamp(40.0, 200.0);
            knob(ui, &mut track.reverb, auto_reverb, |v| {
                egui::Slider::new(v, 0.0..=1.0)
                    .show_value(pro)
                    .fixed_decimals(2)
            });
        });
        let current = match track.instrument.plugin() {
            Some(p) => p.name.clone(),
            None => presets::name_of(&track.instrument)
                .map_or(tr("⚙ Custom"), tr)
                .to_string(),
        };
        let mut picked = false;
        let instruments: Vec<&PluginInfo> = app.plugins.installed(Kind::Instrument).collect();
        if !pro {
            ui.label(RichText::new(tr("Pick a sound (click to hear it):")).color(theme::TEXT_DIM));
            // Which category tab is open; starts on the current sound's.
            // The last tab holds the instrument plugins, when there are any.
            let plugin_tab = Category::ALL.len();
            let with_plugins = !instruments.is_empty() || track.instrument.plugin().is_some();
            let tab_id = ui.id().with("sound_tab").with(index);
            let mut tab = ui
                .data(|d| d.get_temp::<usize>(tab_id))
                .or_else(|| {
                    if track.instrument.plugin().is_some() {
                        return Some(plugin_tab);
                    }
                    let c = presets::of(&track.instrument)?.category;
                    Category::ALL.iter().position(|x| *x == c)
                })
                .unwrap_or(0);
            if tab == plugin_tab && !with_plugins {
                tab = 0;
            }
            ui.horizontal_wrapped(|ui| {
                for (i, c) in Category::ALL.into_iter().enumerate() {
                    if ui.selectable_label(tab == i, tr(c.label())).clicked() {
                        tab = i;
                    }
                }
                if with_plugins
                    && ui
                        .selectable_label(tab == plugin_tab, tr("Plugins"))
                        .on_hover_text(tr("VST3 instruments installed on this computer"))
                        .clicked()
                {
                    tab = plugin_tab;
                }
            });
            ui.data_mut(|d| d.insert_temp(tab_id, tab));
            ui.add_space(4.0);
            if tab == plugin_tab {
                if instruments.is_empty() {
                    ui.weak(tr("No VST3 instruments found on this computer."));
                }
                for info in &instruments {
                    let is = uses(&track.instrument, info);
                    let b = egui::Button::new(RichText::new(&info.name).size(14.5))
                        .truncate()
                        .selected(is)
                        .corner_radius(10);
                    if ui
                        .add_sized([ui.available_width(), 36.0], b)
                        .on_hover_text(about(info))
                        .clicked()
                        && !is
                    {
                        track.instrument = Instrument::Vst3(info.to_ref());
                        picked = true;
                    }
                }
                return picked;
            }
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
                    if !instruments.is_empty() {
                        ui.label(RichText::new(tr("Plugins")).small().color(theme::TEXT_DIM));
                    }
                    for info in &instruments {
                        let is = uses(&track.instrument, info);
                        if ui
                            .add(egui::SelectableLabel::new(is, &info.name))
                            .on_hover_text(about(info))
                            .clicked()
                            && !is
                        {
                            track.instrument = Instrument::Vst3(info.to_ref());
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

    ui.add_space(6.0);
    plugin_card(ui, app, index);
    instrument_settings(ui, &mut app.project.tracks[index], pro, synth_auto);
    effects(ui, app, index);
}

/// Whether `instrument` is the plugin `info`.
fn uses(instrument: &Instrument, info: &PluginInfo) -> bool {
    instrument
        .plugin()
        .is_some_and(|p| p.uid.eq_ignore_ascii_case(&info.uid))
}

/// Who makes a plugin of the song, for its tooltip.
fn about_ref(p: &PluginRef) -> String {
    if p.vendor.is_empty() {
        p.name.clone()
    } else {
        format!("{}  ·  {}", p.name, p.vendor)
    }
}

/// What a plugin is, for its tooltip: "Surge Synth Team · Instrument|Synth".
fn about(info: &PluginInfo) -> String {
    [info.vendor.as_str(), info.category.as_str()]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("  ·  ")
}

/// Why a plugin of the song is not playing, if it is not, with a way to
/// the Plugins window.
fn plugin_problem(ui: &mut egui::Ui, app: &App, key: &Key, p: &PluginRef) {
    let Some(f) = app.plugins.rack.failure(key) else {
        return;
    };
    let why = if app.plugins.store.locate(p).is_err() {
        tr("⚠ Not installed on this computer, so it is silent").to_string()
    } else {
        trf("⚠ Not playing: {}", &[&f.error])
    };
    ui.add(egui::Label::new(RichText::new(why).small().color(theme::DANGER)).wrap());
    if ui
        .small_button(tr("Plugins…"))
        .on_hover_text(tr(
            "Installed it in another folder? Add that folder in the Plugins window.",
        ))
        .clicked()
    {
        // Opened by the caller, which may borrow the app mutably.
        ui.data_mut(|d| d.insert_temp(egui::Id::new("open_plugins_window"), true));
    }
}

/// The button that shows a running plugin's own window. True if clicked.
fn window_button(ui: &mut egui::Ui, app: &App, key: &Key) -> bool {
    let running = app.plugins.rack.slot(key).is_some();
    let shown = app.plugins.editor_open(key);
    let button = icons::button(
        Icon::Window,
        if shown {
            tr("Window is open")
        } else {
            tr("Open window")
        },
    )
    .selected(shown);
    let resp = ui.add_enabled(running && editor::SUPPORTED, button);
    let resp = if editor::SUPPORTED {
        resp.on_hover_text(tr("Show the plugin's own controls in a window of its own"))
    } else {
        resp.on_disabled_hover_text(tr("Plugin windows open on Windows only"))
    };
    resp.clicked()
}

/// The plugin playing the track, when it is one.
fn plugin_card(ui: &mut egui::Ui, app: &mut App, index: usize) {
    let track = &app.project.tracks[index];
    let Some(p) = track.instrument.plugin() else {
        return;
    };
    let key = Key::instrument(&track.id);
    let mut open = false;
    theme::card(theme::SURFACE, egui::Stroke::new(1.0_f32, theme::BORDER)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            icons::label(ui, Icon::Plug);
            ui.add(egui::Label::new(RichText::new(&p.name).strong()).truncate());
        });
        if !p.vendor.is_empty() {
            ui.weak(&p.vendor);
        }
        plugin_problem(ui, app, &key, p);
        open = window_button(ui, app, &key);
    });
    take_plugins_window_request(ui, app);
    if open {
        app.open_plugin_window(&key);
    }
}

/// A "Plugins…" button inside a borrowed part of the inspector asked for
/// the Plugins window.
fn take_plugins_window_request(ui: &mut egui::Ui, app: &mut App) {
    let id = egui::Id::new("open_plugins_window");
    if ui.data_mut(|d| d.remove_temp::<bool>(id)).unwrap_or(false) {
        app.plugins.show_window = true;
    }
}

/// What can be done to an effect, from its row.
enum FxAction {
    Switch(String, bool),
    Move(String, usize),
    Remove(String),
    Open(Key),
    Add(PluginInfo),
}

/// The track's effect plugins, in the order its sound goes through them.
fn effects(ui: &mut egui::Ui, app: &mut App, index: usize) {
    let track = &app.project.tracks[index];
    let installed: Vec<&PluginInfo> = app.plugins.installed(Kind::Effect).collect();
    if track.effects.is_empty() && installed.is_empty() {
        return;
    }
    ui.add_space(8.0);
    theme::section(ui, tr("Effects"));
    if track.effects.is_empty() {
        ui.weak(tr(
            "Effects change this track's sound: echo, reverb, distortion, EQ…",
        ));
    }
    let mut action = None;
    let last = track.effects.len().saturating_sub(1);
    for (i, e) in track.effects.iter().enumerate() {
        let key = Key::effect(&track.id, &e.id);
        let p = e.plugin();
        theme::card(theme::SURFACE, egui::Stroke::new(1.0_f32, theme::BORDER)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let on = !e.bypass;
                let color = if on { theme::OK } else { theme::TEXT_FAINT };
                if ui
                    .add(egui::Button::image(icons::image(Icon::Power, 15.0, color)))
                    .on_hover_text(if on {
                        tr("On: click to switch this effect off")
                    } else {
                        tr("Off: click to switch this effect on")
                    })
                    .clicked()
                {
                    action = Some(FxAction::Switch(e.id.clone(), !e.bypass));
                }
                let name = RichText::new(&p.name).strong();
                let name = if on {
                    name
                } else {
                    name.color(theme::TEXT_FAINT)
                };
                ui.add(egui::Label::new(name).truncate())
                    .on_hover_text(about_ref(p));
            });
            plugin_problem(ui, app, &key, p);
            ui.horizontal(|ui| {
                if window_button(ui, app, &key) {
                    action = Some(FxAction::Open(key.clone()));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add(icons::icon_button(Icon::Trash, 14.0).small())
                        .on_hover_text(tr("Remove this effect (Ctrl+Z brings it back)"))
                        .clicked()
                    {
                        action = Some(FxAction::Remove(e.id.clone()));
                    }
                    if ui
                        .add_enabled(i < last, icons::icon_button(Icon::ArrowDown, 14.0).small())
                        .on_hover_text(tr("Later in the chain"))
                        .clicked()
                    {
                        action = Some(FxAction::Move(e.id.clone(), i + 1));
                    }
                    if ui
                        .add_enabled(i > 0, icons::icon_button(Icon::ArrowUp, 14.0).small())
                        .on_hover_text(tr("Earlier in the chain"))
                        .clicked()
                    {
                        action = Some(FxAction::Move(e.id.clone(), i - 1));
                    }
                });
            });
        });
        ui.add_space(2.0);
    }
    let plus = icons::image(Icon::Plus, 16.0, theme::TEXT);
    ui.menu_image_text_button(plus, tr("Add effect"), |ui| {
        if installed.is_empty() {
            ui.weak(tr("No VST3 effects found on this computer."));
        }
        for info in &installed {
            if ui
                .add(egui::Button::new(&info.name).truncate())
                .on_hover_text(about(info))
                .clicked()
            {
                action = Some(FxAction::Add((*info).clone()));
                ui.close_menu();
            }
        }
    })
    .response
    .on_hover_text(tr("Run this track's sound through an effect plugin"));
    let track_id = track.id.clone();
    take_plugins_window_request(ui, app);
    match action {
        None => {}
        Some(FxAction::Switch(effect_id, bypass)) => {
            app.run_op(Op::UpdateEffect {
                effect_id,
                bypass: Some(bypass),
                index: None,
            });
        }
        Some(FxAction::Move(effect_id, to)) => {
            app.run_op(Op::UpdateEffect {
                effect_id,
                bypass: None,
                index: Some(to),
            });
        }
        Some(FxAction::Remove(effect_id)) => {
            if app.run_op(Op::RemoveEffect { effect_id }).is_some() {
                app.status = tr("Effect removed. Ctrl+Z brings it back.").into();
            }
        }
        Some(FxAction::Open(key)) => app.open_plugin_window(&key),
        Some(FxAction::Add(info)) => {
            let added = app.run_op(Op::AddEffect {
                track_id,
                plugin: info.to_ref(),
                index: None,
                bypass: false,
            });
            if added.is_some() {
                app.status = trf("Added {}", &[&info.name]);
            }
        }
    }
}

/// The settings of a built-in instrument: synth knobs (Pro mode) or the
/// drum kit. `automated` has where automation puts the synth's cutoff,
/// resonance, drive, fm, vibrato, sub and noise at the playhead.
fn instrument_settings(
    ui: &mut egui::Ui,
    track: &mut Track,
    pro: bool,
    automated: [Option<f32>; 7],
) {
    let [a_cutoff, a_resonance, a_drive, a_fm, a_vibrato, a_sub, a_noise] = automated;
    match &mut track.instrument {
        Instrument::Synth(synth) => {
            if !pro {
                return;
            }
            let slider = |ui: &mut egui::Ui, v: &mut f32, r: std::ops::RangeInclusive<f32>| {
                ui.add(egui::Slider::new(v, r).clamping(SliderClamping::Edits));
            };
            // The same for a setting automation can move.
            let moving = |ui: &mut egui::Ui,
                          v: &mut f32,
                          r: std::ops::RangeInclusive<f32>,
                          auto: Option<f32>| {
                knob(ui, v, auto, |v| egui::Slider::new(v, r));
            };
            let seconds = |ui: &mut egui::Ui, v: &mut f32, r: std::ops::RangeInclusive<f32>| {
                ui.add(
                    egui::Slider::new(v, r)
                        .logarithmic(true)
                        .suffix(" s")
                        .clamping(SliderClamping::Edits),
                );
            };
            egui::CollapsingHeader::new(tr("⚙ Fine-tune the sound"))
                .default_open(true)
                .show(ui, |ui| {
                    // Room for the label column and the value box: a grid wider
                    // than the panel makes the panel creep wider every frame.
                    ui.spacing_mut().slider_width =
                        (ui.available_width() - 190.0).clamp(50.0, 160.0);
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
                        knob(ui, &mut synth.cutoff, a_cutoff, |v| {
                            egui::Slider::new(v, 40.0..=20_000.0)
                                .logarithmic(true)
                                .suffix(" Hz")
                        });
                        ui.end_row();
                    });
                });
            egui::CollapsingHeader::new(tr("🔊 More sound shaping"))
                .default_open(false)
                .show(ui, |ui| {
                    ui.spacing_mut().slider_width =
                        (ui.available_width() - 190.0).clamp(50.0, 160.0);
                    egui::Grid::new("synth_more").num_columns(2).show(ui, |ui| {
                        ui.label(tr("Resonance")).on_hover_text(tr(
                            "A whistling peak at the brightness point (acid, wah)",
                        ));
                        moving(ui, &mut synth.resonance, 0.0..=1.0, a_resonance);
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
                        moving(ui, &mut synth.sub, 0.0..=1.0, a_sub);
                        ui.end_row();
                        ui.label(tr("Noise"))
                            .on_hover_text(tr("Mix in hiss: breath, air, grit"));
                        moving(ui, &mut synth.noise, 0.0..=1.0, a_noise);
                        ui.end_row();
                        ui.label(tr("FM")).on_hover_text(tr(
                            "Bell / electric-piano tone (sine and triangle waves)",
                        ));
                        moving(ui, &mut synth.fm, 0.0..=10.0, a_fm);
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
                        moving(ui, &mut synth.vibrato, 0.0..=100.0, a_vibrato);
                        ui.end_row();
                        ui.label(tr("Vibrato speed"));
                        ui.add(
                            egui::Slider::new(&mut synth.vibrato_rate, 0.1..=12.0)
                                .suffix(" Hz")
                                .clamping(SliderClamping::Edits),
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
                        moving(ui, &mut synth.drive, 0.0..=1.0, a_drive);
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
        Instrument::Vst3(_) => {}
    }
}

/// The palette as little dots, the track's colour ringed; a click picks
/// one. Returns whether the colour changed.
pub fn color_swatches(ui: &mut egui::Ui, track: &mut aidaw_core::Track) -> bool {
    use aidaw_core::color;
    let current = track.rgb();
    let custom = !color::PALETTE.iter().any(|(_, c)| *c == current);
    let mut changed = false;
    ui.spacing_mut().item_spacing.x = 3.0;
    let swatches = custom
        .then_some(("", current))
        .into_iter()
        .chain(color::PALETTE.iter().copied());
    for (name, rgb) in swatches {
        let (rect, resp) = ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::click());
        let fill = egui::Color32::from_rgb(rgb[0], rgb[1], rgb[2]);
        let painter = ui.painter();
        painter.circle_filled(rect.center(), 6.0, fill);
        if rgb == current {
            painter.circle_stroke(rect.center(), 8.0, egui::Stroke::new(1.5_f32, theme::TEXT));
        } else if resp.hovered() {
            painter.circle_stroke(
                rect.center(),
                8.0,
                egui::Stroke::new(1.0_f32, theme::alpha(theme::TEXT, 120)),
            );
        }
        let resp = resp
            .on_hover_text(color_label(name))
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        if resp.clicked() && !name.is_empty() && rgb != current {
            // The colour the track would get anyway is stored as no choice.
            track.color = (rgb != track.auto_rgb()).then(|| color::hex(rgb));
            changed = true;
        }
    }
    changed
}

fn color_label(name: &str) -> &'static str {
    match name {
        "blue" => tr("Blue"),
        "orange" => tr("Orange"),
        "green" => tr("Green"),
        "purple" => tr("Purple"),
        "yellow" => tr("Yellow"),
        "cyan" => tr("Cyan"),
        "pink" => tr("Pink"),
        "violet" => tr("Violet"),
        _ => tr("This track's own colour (picked by the AI)"),
    }
}
