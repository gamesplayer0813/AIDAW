//! Piano roll for the selected clip.
//!
//! Click empty grid to add a note, drag a note to move it (time + pitch),
//! drag its right edge to change length, right-click or Delete to remove.

use aidaw_core::edit::Op;
use aidaw_core::{pitch, Note};
use eframe::egui::{
    self, pos2, vec2, Align2, Color32, CornerRadius, FontId, Rect, RichText, Sense, Stroke,
    StrokeKind,
};

use crate::app::{track_color, App, Drag};
use crate::scale::{Scale, ROOTS};
use crate::theme;

const KEYS_W: f32 = 52.0;
const RULER_H: f32 = 22.0;
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
        app.ui_rects.insert("piano", ui.max_rect());
        ui.centered_and_justified(|ui| {
            ui.label(
                RichText::new("👆 Click a coloured clip above to see and edit its notes here.\nNo clips yet? Double-click an empty spot in a track row.")
                    .size(15.0)
                    .weak(),
            )
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

    toolbar(ui, app, &clip_id, track_index);
    ui.separator();

    // Easy mode uses taller rows so notes are easy to hit.
    let kh = if app.pro() { app.roll_key_height } else { 18.0 };
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
        app.ui_rects.insert("piano", visible.intersect(resp.rect));
        let in_key = |p: u8| !app.scale_active(track_index) || app.scale.contains(app.scale_root, p);
        let scale_on = app.scale_active(track_index);
        let keys_left = visible.left().max(origin.x);
        let ruler_top = visible.top().max(origin.y);
        let x_of = |beat: f64| origin.x + KEYS_W + beat as f32 * bw;
        let beat_of = |x: f32| ((x - origin.x - KEYS_W) / bw) as f64;
        let y_of = |p: u8| origin.y + RULER_H + (127 - p) as f32 * kh;
        let pitch_of =
            |y: f32| 127 - (((y - origin.y - RULER_H) / kh).floor() as i32).clamp(0, 127);
        let color = track_color(track_index);
        let bpb = app.project.beats_per_bar.max(1) as f64;

        // Grid.
        painter.rect_filled(resp.rect, 0.0, theme::CANVAS);
        let lit = theme::mix(theme::CANVAS, color, 0.07);
        for p in 0..=127u8 {
            let r = Rect::from_min_size(
                pos2(origin.x + KEYS_W, y_of(p)),
                vec2(resp.rect.width(), kh),
            );
            if r.bottom() < visible.top() || r.top() > visible.bottom() {
                continue;
            }
            if scale_on {
                // Light up the rows that are in the chosen key.
                if in_key(p) {
                    painter.rect_filled(r, 0.0, lit);
                } else {
                    painter.rect_filled(r, 0.0, Color32::from_black_alpha(70));
                }
            } else if pitch::is_black_key(p) {
                painter.rect_filled(r, 0.0, Color32::from_black_alpha(70));
            } else {
                painter.rect_filled(r, 0.0, Color32::from_white_alpha(3));
            }
            let line = if p % 12 == 0 {
                Color32::from_white_alpha(26)
            } else {
                Color32::from_black_alpha(90)
            };
            painter.line_segment([r.left_bottom(), r.right_bottom()], Stroke::new(1.0_f32, line));
        }
        let snap = app.grid();
        let mut b = 0.0;
        let mut i = 0u32;
        while b <= view_beats {
            let x = x_of(b);
            let col = if (b % bpb).abs() < 1e-6 {
                Color32::from_white_alpha(46)
            } else if (b % 1.0).abs() < 1e-6 {
                Color32::from_white_alpha(18)
            } else {
                Color32::from_white_alpha(6)
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
            Color32::from_black_alpha(110),
        );
        painter.line_segment(
            [pos2(end_x, origin.y), pos2(end_x, resp.rect.bottom())],
            Stroke::new(2.0_f32, color),
        );

        // Notes: rounded, brighter the harder they are played, with a light top edge.
        for (idx, n) in clip.notes.iter().enumerate() {
            let r = Rect::from_min_max(
                pos2(x_of(n.start) + 0.5, y_of(n.pitch) + 1.0),
                pos2(
                    x_of(n.start + n.duration).max(x_of(n.start) + 4.0) - 0.5,
                    y_of(n.pitch) + kh - 1.0,
                ),
            );
            if !visible.intersects(r) {
                continue;
            }
            let selected = app.selected_note == Some(idx);
            let fill = if n.start >= clip.length {
                theme::SURFACE_HI
            } else {
                theme::mix(theme::CANVAS, color, 0.45 + 0.55 * n.velocity as f32 / 127.0)
            };
            let radius = (kh / 3.0).min(5.0);
            if selected {
                theme::glow(&painter, r, radius as u8, theme::alpha(color, 150), 12);
            }
            painter.rect_filled(r, radius, fill);
            if r.width() > 2.0 * radius {
                painter.line_segment(
                    [
                        pos2(r.left() + radius, r.top() + 1.0),
                        pos2(r.right() - radius, r.top() + 1.0),
                    ],
                    Stroke::new(1.0_f32, theme::alpha(Color32::WHITE, 70)),
                );
            }
            let stroke = if selected {
                Stroke::new(1.5_f32, Color32::WHITE)
            } else {
                Stroke::new(1.0_f32, Color32::from_black_alpha(140))
            };
            painter.rect_stroke(r, radius, stroke, StrokeKind::Inside);
        }

        // Playhead (relative to the clip).
        let ph = app.playhead - clip.start;
        if ph >= 0.0 && ph <= view_beats {
            let x = x_of(ph);
            painter.line_segment(
                [pos2(x, origin.y), pos2(x, resp.rect.bottom())],
                Stroke::new(5.0_f32, theme::alpha(theme::PLAYHEAD, 40)),
            );
            painter.line_segment(
                [pos2(x, origin.y), pos2(x, resp.rect.bottom())],
                Stroke::new(1.5_f32, theme::PLAYHEAD),
            );
        }

        // Sticky ruler.
        let ruler =
            Rect::from_min_size(pos2(origin.x, ruler_top), vec2(resp.rect.width(), RULER_H));
        painter.rect_filled(ruler, 0.0, theme::PANEL);
        painter.line_segment(
            [ruler.left_bottom(), ruler.right_bottom()],
            Stroke::new(1.0_f32, theme::BORDER),
        );
        let mut b = 0.0;
        while b <= view_beats {
            let x = x_of(b);
            let bar = (b % bpb).abs() < 1e-6;
            let (text, font, col) = if bar {
                (
                    format!("{}", (b / bpb) as i64 + 1),
                    theme::bold_font(11.0),
                    theme::TEXT_DIM,
                )
            } else {
                (
                    format!("{}", (b % bpb) as i64 + 1),
                    FontId::proportional(9.5),
                    theme::TEXT_FAINT,
                )
            };
            painter.line_segment(
                [pos2(x, ruler.bottom() - if bar { 9.0 } else { 4.0 }), pos2(x, ruler.bottom())],
                Stroke::new(1.0_f32, if bar { theme::BORDER_HI } else { theme::BORDER }),
            );
            painter.text(pos2(x + 4.0, ruler_top + 3.0), Align2::LEFT_TOP, text, font, col);
            b += 1.0;
        }

        // Sticky keyboard.
        let keys = Rect::from_min_max(
            pos2(keys_left, origin.y + RULER_H),
            pos2(keys_left + KEYS_W, resp.rect.bottom()),
        );
        painter.rect_filled(keys, 0.0, theme::mix(theme::TEXT, theme::CANVAS, 0.08));
        for p in 0..=127u8 {
            let r = Rect::from_min_size(pos2(keys_left, y_of(p)), vec2(KEYS_W, kh));
            if r.bottom() < ruler_top + RULER_H || r.top() > visible.bottom() {
                continue;
            }
            if app.held_key == Some(p) {
                // The key being held down lights up in the track colour.
                painter.rect_filled(r.shrink2(vec2(0.0, 0.5)), 3.0, color);
            } else if pitch::is_black_key(p) {
                let k = Rect::from_min_size(r.min, vec2(KEYS_W * 0.62, kh));
                painter.rect_filled(
                    k,
                    CornerRadius {
                        nw: 0,
                        sw: 0,
                        ne: 3,
                        se: 3,
                    },
                    theme::SURFACE,
                );
                painter.line_segment(
                    [pos2(k.left(), k.top() + 1.5), pos2(k.right() - 3.0, k.top() + 1.5)],
                    Stroke::new(1.0_f32, theme::alpha(Color32::WHITE, 30)),
                );
            } else {
                painter.line_segment(
                    [r.left_bottom(), r.right_bottom()],
                    Stroke::new(1.0_f32, Color32::from_gray(170)),
                );
            }
            if scale_on && in_key(p) {
                painter.circle_filled(
                    pos2(r.right() - 5.0 - if is_drums { 0.0 } else { 26.0 }, r.center().y),
                    2.5,
                    color,
                );
            }
            let label = if is_drums {
                drum_label(p).map(String::from)
            } else {
                (p % 12 == 0).then(|| pitch::name(p))
            };
            if let Some(label) = label {
                painter.text(
                    pos2(r.right() - 4.0, r.center().y),
                    Align2::RIGHT_CENTER,
                    label,
                    theme::bold_font(kh.min(10.5)),
                    theme::CANVAS,
                );
            }
        }
        painter.line_segment(
            [keys.right_top(), keys.right_bottom()],
            Stroke::new(1.0_f32, theme::BORDER),
        );
        painter.rect_filled(
            Rect::from_min_size(pos2(keys_left, ruler_top), vec2(KEYS_W, RULER_H)),
            0.0,
            theme::PANEL,
        );

        if clip.notes.is_empty() {
            let hint = if is_drums {
                "Click in the grid to add drum hits.\nTry Kick on beats 1 and 3, Snare on 2 and 4, Hi-hat everywhere!"
            } else {
                "This clip is empty. Click anywhere in the grid to add a note!\nHold a piano key on the left to hear it, slide up or down to hear others."
            };
            let center = pos2(
                (visible.left() + KEYS_W + visible.right()) / 2.0,
                visible.top() + RULER_H + 40.0,
            );
            painter.text(center, Align2::CENTER_TOP, hint, FontId::proportional(15.0), theme::TEXT_DIM);
        }

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
                        let index = add_note(app, &clip_id, track_index, app.snap_floor(beat).max(0.0), p);
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
                        let pitch_ = app.fit_pitch(track_index, pitch_of(p.y) as u8);
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
                        let snap = app.grid();
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
        // Hold-to-hear: pressing a piano key sounds it until you let go, and
        // sliding up/down the keyboard plays each key you pass. Dragging a note
        // also keeps it sounding at its current pitch.
        let want = if resp.is_pointer_button_down_on() {
            if let Some(Drag::MoveNote { index, .. }) = &app.drag {
                app.project
                    .clip(&clip_id)
                    .and_then(|(_, c)| c.notes.get(*index))
                    .map(|n| n.pitch)
            } else {
                let on_keys = ui
                    .input(|i| i.pointer.press_origin())
                    .is_some_and(|o| o.x < keys_left + KEYS_W && o.y >= ruler_top + RULER_H);
                on_keys
                    .then(|| ui.input(|i| i.pointer.latest_pos()))
                    .flatten()
                    .map(|p| pitch_of(p.y.max(ruler_top + RULER_H)) as u8)
            }
        } else {
            None
        };
        if want != app.held_key {
            match want {
                Some(p) => app.hold_note(track_index, p),
                None => app.release_note(),
            }
        }

        if resp.drag_stopped() {
            if matches!(app.drag, Some(Drag::MoveNote { .. } | Drag::ResizeNote { .. })) {
                app.ev_note_drags += 1;
            }
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
                        add_note(app, &clip_id, track_index, app.snap_floor(beat).max(0.0), p);
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

fn add_note(
    app: &mut App,
    clip_id: &str,
    track_index: usize,
    start: f64,
    pitch_: u8,
) -> Option<usize> {
    let pitch_ = app.fit_pitch(track_index, pitch_);
    app.preview_note(track_index, pitch_);
    let duration = if app.last_note_length > 0.0 {
        app.last_note_length
    } else {
        app.grid()
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

fn toolbar(ui: &mut egui::Ui, app: &mut App, clip_id: &str, track_index: usize) {
    if app.pro() {
        pro_toolbar(ui, app, clip_id, track_index);
    } else {
        easy_toolbar(ui, app, clip_id, track_index);
    }
}

/// Easy mode: plain words, octave moves only, three loudness buttons, and a
/// "mood" instead of a key + scale (notes always stay in key).
fn easy_toolbar(ui: &mut egui::Ui, app: &mut App, clip_id: &str, track_index: usize) {
    let bpb = app.project.beats_per_bar.max(1) as f64;
    let drums = app
        .project
        .tracks
        .get(track_index)
        .is_some_and(|t| t.instrument.is_drums());
    ui.horizontal(|ui| {
        theme::section(ui, "Notes").on_hover_text(
            "Each row is a piano key (low at the bottom, high at the top). Left to right is time.",
        );
        if let Some(c) = app.project.clip_mut(clip_id) {
            ui.label("Part");
            ui.add(
                egui::TextEdit::singleline(&mut c.name)
                    .hint_text(c.id.clone())
                    .desired_width(110.0),
            );
            ui.label("Length")
                .on_hover_text("How many bars this part lasts");
            let mut bars = (c.length / bpb).max(1.0).round();
            let unit = if bars == 1.0 { " bar" } else { " bars" };
            if ui
                .add(
                    egui::DragValue::new(&mut bars)
                        .range(1.0..=64.0)
                        .speed(0.05)
                        .fixed_decimals(0)
                        .suffix(unit),
                )
                .changed()
            {
                c.length = bars * bpb;
            }
        }
        ui.separator();
        if !drums {
            for (label, st, tip) in [
                (
                    "⬇ Lower",
                    -12,
                    "Play every note in this part one octave lower",
                ),
                (
                    "⬆ Higher",
                    12,
                    "Play every note in this part one octave higher",
                ),
            ] {
                if ui.button(label).on_hover_text(tip).clicked() {
                    app.run_op(Op::TransposeClip {
                        clip_id: clip_id.into(),
                        semitones: st,
                    });
                }
            }
        }
        if ui
            .button("🗑 Clear")
            .on_hover_text("Remove every note in this part (Ctrl+Z to undo)")
            .clicked()
        {
            app.run_op(Op::RemoveNotes {
                clip_id: clip_id.into(),
                from: None,
                to: None,
                pitch: None,
            });
            app.selected_note = None;
        }
        let selected = app.selected_note;
        if let Some(n) = selected.and_then(|i| {
            app.project
                .clip_mut(clip_id)
                .and_then(|c| c.notes.get_mut(i))
        }) {
            ui.separator();
            ui.label(format!("Note {}", pitch::name(n.pitch)));
            for (label, v) in [("Soft", 50u8), ("Medium", 90), ("Loud", 120)] {
                let on = match v {
                    50 => n.velocity < 70,
                    90 => (70..105).contains(&n.velocity),
                    _ => n.velocity >= 105,
                };
                if ui
                    .selectable_label(on, label)
                    .on_hover_text("How hard this note is played")
                    .clicked()
                {
                    n.velocity = v;
                }
            }
        }
    });
    ui.horizontal(|ui| {
        let key = ui.scope(|ui| {
            ui.add_enabled_ui(!drums, |ui| {
                ui.label("🎵 Mood").on_hover_text(
                    "Pick a feeling. Lit-up rows fit the mood, and every note you draw lands on one of them, so it always sounds right.",
                );
                for (label, root, sc, tip) in [
                    ("😊 Happy", 0u8, Scale::Major, "C major: bright and cheerful"),
                    ("😢 Sad", 9, Scale::Minor, "A minor: dark and emotional"),
                    (
                        "👌 Can't go wrong",
                        0,
                        Scale::Pentatonic,
                        "C pentatonic: almost anything you draw sounds nice",
                    ),
                    ("🎷 Bluesy", 9, Scale::Blues, "A blues: cool and gritty"),
                ] {
                    let on = app.scale == sc && app.scale_root == root;
                    if ui.selectable_label(on, label).on_hover_text(tip).clicked() {
                        app.scale = sc;
                        app.scale_root = root;
                    }
                }
            })
            .response
            .on_disabled_hover_text("Drums don't have a mood: every row is a different drum");
        });
        app.ui_rects.insert("key", key.response.rect);
    });
    theme::hint(ui, "click to add a note · drag to move · drag the right edge to make longer · right-click to delete · hold a piano key to hear it");
}

fn pro_toolbar(ui: &mut egui::Ui, app: &mut App, clip_id: &str, track_index: usize) {
    ui.horizontal(|ui| {
        theme::section(ui, "Piano roll").on_hover_text(
            "Each row is a piano key (low at the bottom, high at the top). Left to right is time.",
        );
        if let Some(c) = app.project.clip_mut(clip_id) {
            ui.label("Clip");
            ui.add(
                egui::TextEdit::singleline(&mut c.name)
                    .hint_text(c.id.clone())
                    .desired_width(110.0),
            );
            ui.label("Length")
                .on_hover_text("How long this clip is. 4 beats = 1 bar.");
            ui.add(
                egui::DragValue::new(&mut c.length)
                    .range(0.25..=1024.0)
                    .speed(0.25)
                    .suffix(" beats"),
            );
        }
        ui.separator();
        ui.label("Move all notes")
            .on_hover_text("Transpose: shift every note in this clip up or down");
        for (label, st, tip) in [
            ("⏬", -12, "Down one octave"),
            ("🔽", -1, "Down a little (one semitone)"),
            ("🔼", 1, "Up a little (one semitone)"),
            ("⏫", 12, "Up one octave"),
        ] {
            if ui.small_button(label).on_hover_text(tip).clicked() {
                app.run_op(Op::TransposeClip {
                    clip_id: clip_id.into(),
                    semitones: st,
                });
            }
        }
        if ui
            .small_button("🗑 Clear")
            .on_hover_text("Remove every note in this clip (Ctrl+Z to undo)")
            .clicked()
        {
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
            ui.label("Loudness")
                .on_hover_text("Velocity: how hard the note is played");
            ui.add(egui::Slider::new(&mut n.velocity, 1..=127));
            ui.label("Length").on_hover_text("Note length in beats");
            ui.add(
                egui::DragValue::new(&mut n.duration)
                    .range(0.01..=256.0)
                    .speed(0.05),
            );
        }
    });
    ui.horizontal(|ui| {
        let drums = app.project.tracks.get(track_index).is_some_and(|t| t.instrument.is_drums());
        let key = ui.scope(|ui| {
            ui.add_enabled_ui(!drums, |ui| {
                ui.label("🎵 Key").on_hover_text(
                    "Notes in the same key sound good together. Lit-up rows are in the key.",
                );
                egui::ComboBox::from_id_salt("scale_root")
                    .width(48.0)
                    .selected_text(ROOTS[app.scale_root as usize % 12])
                    .show_ui(ui, |ui| {
                        for (i, r) in ROOTS.iter().enumerate() {
                            ui.selectable_value(&mut app.scale_root, i as u8, *r);
                        }
                    });
                egui::ComboBox::from_id_salt("scale")
                    .width(190.0)
                    .selected_text(app.scale.label())
                    .show_ui(ui, |ui| {
                        for sc in Scale::ALL {
                            ui.selectable_value(&mut app.scale, sc, sc.label());
                        }
                    });
                ui.add_enabled(
                    app.scale != Scale::Off,
                    egui::Checkbox::new(&mut app.scale_lock, "Stay in key"),
                )
                .on_hover_text("New and moved notes jump to the nearest note in the key, so it always sounds right.");
            })
            .response
            .on_disabled_hover_text("Drums don't have a key");
        });
        app.ui_rects.insert("key", key.response.rect);
        ui.separator();
        ui.label("Zoom");
        ui.add(egui::Slider::new(&mut app.roll_zoom, 20.0..=400.0).logarithmic(true).show_value(false));
        ui.label("Height");
        ui.add(egui::Slider::new(&mut app.roll_key_height, 8.0..=24.0).show_value(false));
    });
    theme::hint(ui, "click the grid to add a note · drag to move · drag the right edge to lengthen · right-click to delete · hold or slide on piano keys to hear them");
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
