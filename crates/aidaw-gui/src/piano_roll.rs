//! Piano roll for the selected clip.
//!
//! Click empty grid to add a note, drag a note to move it (time + pitch),
//! drag its right edge to change length, right-click or Delete to remove.

use aidaw_core::edit::Op;
use aidaw_core::{pitch, Note};
use eframe::egui::{self, pos2, vec2, Align2, Color32, FontId, Rect, Sense, Stroke, StrokeKind};

use crate::app::{track_color, App, Drag};

const KEYS_W: f32 = 52.0;
const RULER_H: f32 = 20.0;
const EDGE: f32 = 6.0;

enum Hit {
    Ruler(f64),
    Keys,
    NoteBody { index: usize, offset: f64 },
    NoteEdge { index: usize },
    Grid { beat: f64, pitch: u8 },
}

pub fn show(ui: &mut egui::Ui, app: &mut App) {
    let Some(clip_id) = app.selected_clip.clone() else {
        ui.centered_and_justified(|ui| {
            ui.weak("Select or double-click to create a clip in the arrangement to edit its notes")
        });
        return;
    };
    let Some((track_index, is_drums)) = app
        .project
        .tracks
        .iter()
        .position(|t| t.clips.iter().any(|c| c.id == clip_id))
        .map(|i| (i, app.project.tracks[i].instrument.is_drums()))
    else {
        return;
    };

    toolbar(ui, app, &clip_id);
    ui.separator();

    let kh = app.roll_key_height;
    let bw = app.roll_zoom;
    let clip = app
        .project
        .clip(&clip_id)
        .map(|(_, c)| c.clone())
        .expect("clip exists");
    let view_beats = (clip.length + app.project.beats_per_bar as f64 * 2.0).max(8.0);
    let size = vec2(KEYS_W + view_beats as f32 * bw, RULER_H + 128.0 * kh);

    let mut area = egui::ScrollArea::both()
        .id_salt("piano_roll")
        .auto_shrink([false, false]);
    if std::mem::take(&mut app.roll_scroll_to_notes) {
        let top = clip
            .notes
            .iter()
            .map(|n| n.pitch)
            .max()
            .map(|p| p.saturating_add(6))
            .unwrap_or(if is_drums { 52 } else { 79 });
        area = area
            .vertical_scroll_offset(((127 - top.min(127)) as f32 * kh).max(0.0))
            .horizontal_scroll_offset(0.0);
    }

    area.show(ui, |ui| {
        let (resp, painter) =
            ui.allocate_painter(size.max(ui.available_size()), Sense::click_and_drag());
        let origin = resp.rect.min;
        let visible = ui.clip_rect();
        let keys_left = visible.left().max(origin.x);
        let ruler_top = visible.top().max(origin.y);
        let x_of = |beat: f64| origin.x + KEYS_W + beat as f32 * bw;
        let beat_of = |x: f32| ((x - origin.x - KEYS_W) / bw) as f64;
        let y_of = |p: u8| origin.y + RULER_H + (127 - p) as f32 * kh;
        let pitch_of =
            |y: f32| 127 - (((y - origin.y - RULER_H) / kh).floor() as i32).clamp(0, 127);
        let visuals = ui.visuals().clone();
        let color = track_color(track_index);
        let bpb = app.project.beats_per_bar.max(1) as f64;

        // Grid.
        painter.rect_filled(resp.rect, 0.0, visuals.extreme_bg_color);
        for p in 0..=127u8 {
            let r = Rect::from_min_size(
                pos2(origin.x + KEYS_W, y_of(p)),
                vec2(resp.rect.width(), kh),
            );
            if pitch::is_black_key(p) {
                painter.rect_filled(r, 0.0, Color32::from_black_alpha(60));
            }
            if p % 12 == 0 {
                painter.line_segment(
                    [r.left_bottom(), r.right_bottom()],
                    Stroke::new(1.0_f32, Color32::from_white_alpha(30)),
                );
            }
        }
        let snap = app.snap;
        let mut b = 0.0;
        let mut i = 0u32;
        while b <= view_beats {
            let x = x_of(b);
            let col = if (b % bpb).abs() < 1e-6 {
                Color32::from_white_alpha(60)
            } else if (b % 1.0).abs() < 1e-6 {
                Color32::from_white_alpha(28)
            } else {
                Color32::from_white_alpha(10)
            };
            painter.line_segment(
                [pos2(x, origin.y + RULER_H), pos2(x, resp.rect.bottom())],
                Stroke::new(1.0_f32, col),
            );
            i += 1;
            b = i as f64 * snap;
        }
        // Shade beyond the clip end.
        let end_x = x_of(clip.length);
        painter.rect_filled(
            Rect::from_min_max(pos2(end_x, origin.y), resp.rect.right_bottom()),
            0.0,
            Color32::from_black_alpha(90),
        );
        painter.line_segment(
            [pos2(end_x, origin.y), pos2(end_x, resp.rect.bottom())],
            Stroke::new(2.0_f32, color),
        );

        // Notes.
        for (idx, n) in clip.notes.iter().enumerate() {
            let r = Rect::from_min_max(
                pos2(x_of(n.start), y_of(n.pitch) + 1.0),
                pos2(
                    x_of(n.start + n.duration).max(x_of(n.start) + 3.0),
                    y_of(n.pitch) + kh - 1.0,
                ),
            );
            let selected = app.selected_note == Some(idx);
            let shade = 0.35 + 0.65 * n.velocity as f32 / 127.0;
            let fill = if n.start >= clip.length {
                Color32::GRAY.gamma_multiply(0.4)
            } else {
                color.gamma_multiply(shade)
            };
            painter.rect_filled(r, 2.0, fill);
            let stroke = if selected {
                Stroke::new(2.0_f32, Color32::WHITE)
            } else {
                Stroke::new(1.0_f32, Color32::from_black_alpha(160))
            };
            painter.rect_stroke(r, 2.0, stroke, StrokeKind::Inside);
        }

        // Playhead (relative to the clip).
        let ph = app.playhead - clip.start;
        if ph >= 0.0 && ph <= view_beats {
            let x = x_of(ph);
            painter.line_segment(
                [pos2(x, origin.y), pos2(x, resp.rect.bottom())],
                Stroke::new(1.5_f32, Color32::from_rgb(255, 80, 80)),
            );
        }

        // Sticky ruler.
        let ruler =
            Rect::from_min_size(pos2(origin.x, ruler_top), vec2(resp.rect.width(), RULER_H));
        painter.rect_filled(ruler, 0.0, visuals.panel_fill);
        let mut b = 0.0;
        while b <= view_beats {
            let x = x_of(b);
            let text = if (b % bpb).abs() < 1e-6 {
                format!("{}", (b / bpb) as i64 + 1)
            } else {
                format!("·{}", (b % bpb) as i64 + 1)
            };
            painter.text(
                pos2(x + 2.0, ruler_top + 3.0),
                Align2::LEFT_TOP,
                text,
                FontId::monospace(10.0),
                visuals.weak_text_color(),
            );
            b += 1.0;
        }

        // Sticky keyboard.
        let keys = Rect::from_min_max(
            pos2(keys_left, origin.y + RULER_H),
            pos2(keys_left + KEYS_W, resp.rect.bottom()),
        );
        painter.rect_filled(keys, 0.0, Color32::from_gray(200));
        for p in 0..=127u8 {
            let r = Rect::from_min_size(pos2(keys_left, y_of(p)), vec2(KEYS_W, kh));
            if r.bottom() < ruler_top + RULER_H {
                continue;
            }
            if pitch::is_black_key(p) {
                painter.rect_filled(
                    Rect::from_min_size(r.min, vec2(KEYS_W * 0.62, kh)),
                    0.0,
                    Color32::from_gray(30),
                );
            } else {
                painter.line_segment(
                    [r.left_bottom(), r.right_bottom()],
                    Stroke::new(1.0_f32, Color32::from_gray(150)),
                );
            }
            let label = if is_drums {
                drum_label(p).map(String::from)
            } else {
                (p % 12 == 0).then(|| pitch::name(p))
            };
            if let Some(label) = label {
                painter.text(
                    pos2(r.right() - 3.0, r.center().y),
                    Align2::RIGHT_CENTER,
                    label,
                    FontId::proportional(kh.min(11.0)),
                    Color32::from_gray(20),
                );
            }
        }
        painter.rect_filled(
            Rect::from_min_size(pos2(keys_left, ruler_top), vec2(KEYS_W, RULER_H)),
            0.0,
            visuals.panel_fill,
        );

        // ---- interaction ----
        let hit = |notes: &[Note], p: egui::Pos2| -> Hit {
            if p.y < ruler_top + RULER_H {
                return Hit::Ruler(beat_of(p.x).max(0.0));
            }
            if p.x < keys_left + KEYS_W {
                return Hit::Keys;
            }
            let pitch_ = pitch_of(p.y) as u8;
            let beat = beat_of(p.x);
            for (index, n) in notes.iter().enumerate().rev() {
                if n.pitch != pitch_ {
                    continue;
                }
                let (x0, x1) = (
                    x_of(n.start),
                    x_of(n.start + n.duration).max(x_of(n.start) + 3.0),
                );
                if p.x >= x0 && p.x <= x1 {
                    return if x1 - p.x <= EDGE && x1 - x0 > EDGE * 1.5 {
                        Hit::NoteEdge { index }
                    } else {
                        Hit::NoteBody {
                            index,
                            offset: beat - n.start,
                        }
                    };
                }
            }
            Hit::Grid {
                beat,
                pitch: pitch_,
            }
        };

        if let Some(p) = resp.hover_pos() {
            if matches!(hit(&clip.notes, p), Hit::NoteEdge { .. }) {
                ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
            }
        }

        if resp.drag_started() {
            if let Some(p) = ui.input(|i| i.pointer.press_origin()) {
                match hit(&clip.notes, p) {
                    Hit::NoteBody { index, offset } => {
                        app.selected_note = Some(index);
                        app.drag = Some(Drag::MoveNote {
                            index,
                            grab_offset: offset,
                        });
                    }
                    Hit::NoteEdge { index } => {
                        app.selected_note = Some(index);
                        app.drag = Some(Drag::ResizeNote { index });
                    }
                    Hit::Grid { beat, pitch: p } => {
                        // Drag on empty grid: create a note and immediately resize it.
                        let index = add_note(app, &clip_id, app.snap_floor(beat).max(0.0), p);
                        app.drag = index.map(|index| Drag::ResizeNote { index });
                    }
                    Hit::Ruler(b) => app.seek(clip.start + b),
                    Hit::Keys => {}
                }
            }
        }
        if resp.dragged() {
            if let Some(p) = resp.interact_pointer_pos() {
                let beat = beat_of(p.x);
                let snapped_move = |grab: f64| app.snap_beat(beat - grab).max(0.0);
                match app.drag.clone() {
                    Some(Drag::MoveNote { index, grab_offset }) => {
                        let start = snapped_move(grab_offset);
                        let pitch_ = pitch_of(p.y) as u8;
                        if let Some(n) = app
                            .project
                            .clip_mut(&clip_id)
                            .and_then(|c| c.notes.get_mut(index))
                        {
                            n.start = start;
                            n.pitch = pitch_;
                        }
                    }
                    Some(Drag::ResizeNote { index }) => {
                        let end = app.snap_beat(beat);
                        let snap = app.snap;
                        if let Some(n) = app
                            .project
                            .clip_mut(&clip_id)
                            .and_then(|c| c.notes.get_mut(index))
                        {
                            n.duration = (end - n.start).max(snap);
                            app.last_note_length = n.duration;
                        }
                    }
                    _ => {
                        if p.y < ruler_top + RULER_H {
                            app.seek(clip.start + beat.max(0.0));
                        }
                    }
                }
            }
        }
        if resp.drag_stopped() {
            app.drag = None;
        }

        if resp.clicked() {
            if let Some(p) = resp.interact_pointer_pos() {
                match hit(&clip.notes, p) {
                    Hit::NoteBody { index, .. } | Hit::NoteEdge { index } => {
                        app.selected_note = Some(index);
                        if let Some(n) = clip.notes.get(index) {
                            app.last_note_length = n.duration;
                        }
                    }
                    Hit::Grid { beat, pitch: p } => {
                        add_note(app, &clip_id, app.snap_floor(beat).max(0.0), p);
                    }
                    Hit::Ruler(b) => app.seek(clip.start + b),
                    Hit::Keys => {}
                }
            }
        }
        if resp.secondary_clicked() {
            if let Some(p) = resp.interact_pointer_pos() {
                if let Hit::NoteBody { index, .. } | Hit::NoteEdge { index } = hit(&clip.notes, p) {
                    remove_note(app, &clip_id, index);
                }
            }
        }
        if resp.hovered() && !ui.ctx().wants_keyboard_input() {
            if ui.input(|i| i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace))
            {
                if let Some(index) = app.selected_note {
                    remove_note(app, &clip_id, index);
                }
            }
            // Ctrl + wheel: horizontal zoom.
            let (zoom, ctrl) = ui.input(|i| (i.zoom_delta(), i.modifiers.command));
            if ctrl && zoom != 1.0 {
                app.roll_zoom = (app.roll_zoom * zoom).clamp(20.0, 400.0);
            }
        }
    });
}

fn add_note(app: &mut App, clip_id: &str, start: f64, pitch_: u8) -> Option<usize> {
    let duration = if app.last_note_length > 0.0 {
        app.last_note_length
    } else {
        app.snap
    };
    let velocity = app
        .selected_note
        .and_then(|i| {
            app.project
                .clip(clip_id)
                .and_then(|(_, c)| c.notes.get(i))
                .map(|n| n.velocity)
        })
        .unwrap_or(100);
    let clip = app.project.clip_mut(clip_id)?;
    clip.notes.push(Note {
        pitch: pitch_,
        start,
        duration,
        velocity,
    });
    let index = clip.notes.len() - 1;
    app.selected_note = Some(index);
    Some(index)
}

fn remove_note(app: &mut App, clip_id: &str, index: usize) {
    if let Some(c) = app.project.clip_mut(clip_id) {
        if index < c.notes.len() {
            c.notes.remove(index);
        }
    }
    app.selected_note = None;
}

fn toolbar(ui: &mut egui::Ui, app: &mut App, clip_id: &str) {
    ui.horizontal(|ui| {
        ui.heading("Piano roll");
        if let Some(c) = app.project.clip_mut(clip_id) {
            ui.label("Clip");
            ui.add(
                egui::TextEdit::singleline(&mut c.name)
                    .hint_text(c.id.clone())
                    .desired_width(110.0),
            );
            ui.label("Length");
            ui.add(
                egui::DragValue::new(&mut c.length)
                    .range(0.25..=1024.0)
                    .speed(0.25)
                    .suffix(" beats"),
            );
        }
        ui.separator();
        ui.label("Transpose");
        for (label, st) in [("-12", -12), ("-1", -1), ("+1", 1), ("+12", 12)] {
            if ui.small_button(label).clicked() {
                app.run_op(Op::TransposeClip {
                    clip_id: clip_id.into(),
                    semitones: st,
                });
            }
        }
        if ui.small_button("Clear").clicked() {
            app.run_op(Op::RemoveNotes {
                clip_id: clip_id.into(),
                from: None,
                to: None,
                pitch: None,
            });
            app.selected_note = None;
        }
        ui.separator();
        let selected = app.selected_note;
        if let Some(n) = selected.and_then(|i| {
            app.project
                .clip_mut(clip_id)
                .and_then(|c| c.notes.get_mut(i))
        }) {
            ui.label(format!("Note {}", pitch::name(n.pitch)));
            ui.label("Vel");
            ui.add(egui::Slider::new(&mut n.velocity, 1..=127));
            ui.label("Len");
            ui.add(
                egui::DragValue::new(&mut n.duration)
                    .range(0.01..=256.0)
                    .speed(0.05),
            );
        }
    });
    ui.horizontal(|ui| {
        ui.label("Zoom");
        ui.add(egui::Slider::new(&mut app.roll_zoom, 20.0..=400.0).logarithmic(true).show_value(false));
        ui.label("Height");
        ui.add(egui::Slider::new(&mut app.roll_key_height, 8.0..=24.0).show_value(false));
        ui.weak("click grid to add · drag to move · drag edge to resize · right-click / Del to delete · Ctrl+wheel to zoom");
    });
}

fn drum_label(p: u8) -> Option<&'static str> {
    Some(match p {
        36 => "Kick",
        38 => "Snare",
        39 => "Clap",
        42 => "C.Hat",
        46 => "O.Hat",
        41 => "Tom L",
        45 => "Tom M",
        48 => "Tom H",
        49 => "Crash",
        _ => return None,
    })
}
