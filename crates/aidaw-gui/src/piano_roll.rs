//! Piano roll for the selected clip.
//!
//! Click empty grid to add a note, drag a note to move it (time + pitch),
//! drag its right edge to change length, right-click or Delete to remove.
//! Shift+drag on empty grid selects several notes (Ctrl+drag adds to the
//! selection, Ctrl+click picks single notes); dragging one of them moves or
//! lengthens them all. The editing keys live in `App::edit_shortcuts`.

use std::collections::BTreeSet;

use aidaw_core::edit::Op;
use aidaw_core::{pitch, Note};
use eframe::egui::{
    self, pos2, vec2, Align2, Color32, CornerRadius, FontId, Rect, RichText, Sense, Stroke,
    StrokeKind,
};

use crate::app::{notes_text, track_color, App, Drag, EditFocus};
use crate::editing;
use crate::i18n::{tr, trf};
use crate::icons::{self, Icon};
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
                RichText::new(tr("👆 Click a coloured clip above to see and edit its notes here.\nNo clips yet? Double-click an empty spot in a track row."))
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
    // After a keyboard edit (paste, duplicate, arrows), bring the selected
    // notes into view if they ended up outside it. Offsets are measured from
    // the corner of the note grid, under the sticky ruler and keyboard.
    if std::mem::take(&mut app.roll_reveal) {
        let pitches = app
            .selected_notes
            .iter()
            .filter_map(|&i| clip.notes.get(i).map(|n| n.pitch));
        let (lo, hi) = (pitches.clone().min(), pitches.max());
        let beats = editing::span(&clip, &app.selected_notes);
        if let (Some((offset, view)), Some((start, end)), Some(lo), Some(hi)) =
            (app.roll_view, beats, lo, hi)
        {
            let (x0, x1) = (start as f32 * bw, end as f32 * bw);
            if x0 < offset.x || x1 > offset.x + view.x - KEYS_W {
                area = area.horizontal_scroll_offset((x0 - bw).max(0.0));
            }
            let (y0, y1) = ((127 - hi) as f32 * kh, (128 - lo) as f32 * kh);
            if y0 < offset.y || y1 > offset.y + view.y - RULER_H {
                area = area.vertical_scroll_offset((y0 - 3.0 * kh).max(0.0));
            }
        }
    }

    let shown = area.show(ui, |ui| {
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
        // Key rows counted from the top (pitch 127), not rounded.
        let row_of = |y: f32| (y - origin.y - RULER_H) / kh;
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
        // Selected notes glow; their outline is white while the editing keys act on them.
        let focused = app.edit_focus == EditFocus::Notes;
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
            let selected = app.selected_notes.contains(&idx);
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
            let stroke = if selected && focused {
                Stroke::new(1.5_f32, Color32::WHITE)
            } else if selected {
                Stroke::new(1.5_f32, theme::alpha(Color32::WHITE, 110))
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
                tr("Click in the grid to add drum hits.\nTry Kick on beats 1 and 3, Snare on 2 and 4, Hi-hat everywhere!")
            } else {
                tr("This clip is empty. Click anywhere in the grid to add a note!\nHold a piano key on the left to hear it, slide up or down to hear others.")
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

        let (shift, ctrl) = ui.input(|i| (i.modifiers.shift, i.modifiers.command));
        if resp.drag_started() {
            if let Some(p) = ui.input(|i| i.pointer.press_origin()) {
                match hit(&clip.notes, p) {
                    Hit::NoteBody { index, offset } => {
                        grab(app, index, shift || ctrl);
                        let from = app
                            .selected_notes
                            .iter()
                            .filter_map(|&i| clip.notes.get(i).map(|n| (i, n.start, n.pitch)))
                            .collect();
                        app.drag = Some(Drag::MoveNotes {
                            anchor: index,
                            grab_offset: offset,
                            from,
                        });
                    }
                    Hit::NoteEdge { index } => {
                        grab(app, index, shift || ctrl);
                        let from = app
                            .selected_notes
                            .iter()
                            .filter_map(|&i| clip.notes.get(i).map(|n| (i, n.duration)))
                            .collect();
                        app.drag = Some(Drag::ResizeNotes { anchor: index, from });
                    }
                    Hit::Grid { beat, .. } if shift || ctrl => {
                        // Box select: Shift starts afresh, Ctrl adds to the selection.
                        app.edit_focus = EditFocus::Notes;
                        let keep = if ctrl {
                            app.selected_notes.clone()
                        } else {
                            BTreeSet::new()
                        };
                        app.drag = Some(Drag::SelectNotes {
                            beat,
                            row: row_of(p.y),
                            keep,
                        });
                    }
                    Hit::Grid { beat, pitch: p } => {
                        // Drag on empty grid: create a note and immediately resize it.
                        let index = add_note(app, &clip_id, track_index, app.snap_floor(beat).max(0.0), p);
                        app.drag = index.and_then(|index| {
                            let length = app.project.clip(&clip_id)?.1.notes.get(index)?.duration;
                            Some(Drag::ResizeNotes {
                                anchor: index,
                                from: vec![(index, length)],
                            })
                        });
                    }
                    Hit::Ruler(b) => app.seek(clip.start + b),
                    Hit::Keys => {}
                }
            }
        }
        if resp.dragged() {
            if let Some(p) = resp.interact_pointer_pos() {
                let beat = beat_of(p.x);
                match app.drag.clone() {
                    Some(Drag::MoveNotes {
                        anchor,
                        grab_offset,
                        from,
                    }) => {
                        if let Some(&(_, start, pitch_)) = from.iter().find(|f| f.0 == anchor) {
                            // Time: the grabbed note follows the grid, the rest keep
                            // their distance to it, and none goes before the clip.
                            let earliest = from.iter().map(|f| f.1).fold(f64::INFINITY, f64::min);
                            let moved_by = (app.snap_beat(beat - grab_offset) - start).max(-earliest);
                            // Pitch: every note moves by as many notes of the key as
                            // the grabbed one, so a chord stays a chord in key. Counted
                            // from the grabbed note's row, so a sideways drag leaves
                            // every pitch alone, even one outside the key.
                            let (key, root) = (app.key_for(track_index), app.scale_root);
                            let target = key.snap(root, pitch_of(p.y) as u8);
                            let steps = key.steps_between(root, key.snap(root, pitch_), target);
                            let pitches: Option<Vec<u8>> =
                                from.iter().map(|f| key.step(root, f.2, steps)).collect();
                            if let Some(c) = app.project.clip_mut(&clip_id) {
                                for (k, &(i, s, _)) in from.iter().enumerate() {
                                    if let Some(n) = c.notes.get_mut(i) {
                                        n.start = s + moved_by;
                                        if let Some(p) = &pitches {
                                            n.pitch = p[k];
                                        }
                                    }
                                }
                            }
                        }
                    }
                    Some(Drag::ResizeNotes { anchor, from }) => {
                        let end = app.snap_beat(beat);
                        let snap = app.grid();
                        if let Some(c) = app.project.clip_mut(&clip_id) {
                            let grabbed = c.notes.get(anchor).map(|n| n.start);
                            let before = from.iter().find(|f| f.0 == anchor).map(|f| f.1);
                            if let (Some(start), Some(before)) = (grabbed, before) {
                                let length = (end - start).max(snap);
                                for &(i, len) in &from {
                                    if let Some(n) = c.notes.get_mut(i) {
                                        n.duration = (len + length - before).max(snap.min(len));
                                    }
                                }
                                app.last_note_length = length;
                            }
                        }
                    }
                    Some(Drag::SelectNotes { beat: b0, row, keep }) => {
                        let mut sel = keep;
                        sel.extend(editing::in_box(&clip.notes, (b0, beat), (row, row_of(p.y))));
                        app.selected_notes = sel;
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
            if let Some(Drag::MoveNotes { anchor, .. }) = &app.drag {
                app.project
                    .clip(&clip_id)
                    .and_then(|(_, c)| c.notes.get(*anchor))
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

        // The selection box being drawn.
        if let (Some(Drag::SelectNotes { beat, row, .. }), Some(p)) =
            (&app.drag, resp.interact_pointer_pos())
        {
            let grid_area = Rect::from_min_max(
                pos2(keys_left + KEYS_W, ruler_top + RULER_H),
                visible.max,
            );
            let corner = pos2(x_of(*beat), origin.y + RULER_H + row * kh);
            let r = Rect::from_two_pos(corner, p);
            let painter = painter.with_clip_rect(grid_area.intersect(painter.clip_rect()));
            painter.rect_filled(r, 2.0, theme::alpha(theme::ACCENT, 28));
            painter.rect_stroke(
                r,
                2.0,
                Stroke::new(1.0_f32, theme::alpha(theme::ACCENT, 170)),
                StrokeKind::Inside,
            );
        }

        if resp.drag_stopped() {
            if matches!(app.drag, Some(Drag::MoveNotes { .. } | Drag::ResizeNotes { .. })) {
                app.ev_note_drags += 1;
            }
            app.drag = None;
        }

        if resp.clicked() {
            if let Some(p) = resp.interact_pointer_pos() {
                match hit(&clip.notes, p) {
                    Hit::NoteBody { index, .. } | Hit::NoteEdge { index } => {
                        app.edit_focus = EditFocus::Notes;
                        if shift || ctrl {
                            // Ctrl/Shift+click: add the note to the selection or take it out.
                            if !app.selected_notes.remove(&index) {
                                app.selected_notes.insert(index);
                            }
                        } else {
                            app.selected_notes = BTreeSet::from([index]);
                        }
                        if let Some(n) = clip.notes.get(index) {
                            app.last_note_length = n.duration;
                        }
                    }
                    Hit::Grid { .. } if shift || ctrl => {
                        app.edit_focus = EditFocus::Notes;
                        app.selected_notes.clear();
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
        if resp.hovered() {
            // Ctrl + wheel: horizontal zoom.
            let (zoom, ctrl) = ui.input(|i| (i.zoom_delta(), i.modifiers.command));
            if ctrl && zoom != 1.0 {
                app.roll_zoom = (app.roll_zoom * zoom).clamp(20.0, 400.0);
            }
        }
    });
    app.roll_view = Some((shown.state.offset, shown.inner_rect.size()));
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
    // A new note is played as hard as the selected one.
    let velocity = app
        .selected_notes
        .first()
        .and_then(|&i| {
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
    app.selected_notes = BTreeSet::from([index]);
    app.edit_focus = EditFocus::Notes;
    Some(index)
}

fn remove_note(app: &mut App, clip_id: &str, index: usize) {
    if let Some(c) = app.project.clip_mut(clip_id) {
        if index < c.notes.len() {
            c.notes.remove(index);
            app.selected_notes =
                editing::after_delete(&app.selected_notes, &BTreeSet::from([index]));
        }
    }
}

/// Pressing on a note: with Ctrl/Shift it joins the selection, otherwise a
/// note outside the selection becomes the only one selected (pressing on a
/// selected note keeps the others, so they can all be dragged together).
fn grab(app: &mut App, index: usize, add: bool) {
    app.edit_focus = EditFocus::Notes;
    if add {
        app.selected_notes.insert(index);
    } else if !app.selected_notes.contains(&index) {
        app.selected_notes = BTreeSet::from([index]);
    }
}

/// How many notes of the clip are selected, and the first of them (the one
/// whose values the toolbar shows).
fn first_selected(app: &App, clip_id: &str) -> Option<(usize, Note)> {
    let (_, c) = app.project.clip(clip_id)?;
    let count = app
        .selected_notes
        .iter()
        .filter(|&&i| i < c.notes.len())
        .count();
    let first = app.selected_notes.iter().find_map(|&i| c.notes.get(i))?;
    Some((count, first.clone()))
}

/// Change every selected note of the clip.
fn for_selected(app: &mut App, clip_id: &str, change: impl Fn(&mut Note)) {
    if let Some(c) = app.project.clip_mut(clip_id) {
        for &i in &app.selected_notes {
            if let Some(n) = c.notes.get_mut(i) {
                change(n);
            }
        }
    }
}

/// "Note C4", or "3 notes" when several are selected.
fn selection_label(count: usize, first: &Note) -> String {
    if count == 1 {
        trf("Note {}", &[&pitch::name(first.pitch)])
    } else {
        notes_text(count)
    }
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
    ui.horizontal_wrapped(|ui| {
        theme::section(ui, tr("Notes")).on_hover_text(tr(
            "Each row is a piano key (low at the bottom, high at the top). Left to right is time.",
        ));
        if let Some(c) = app.project.clip_mut(clip_id) {
            ui.label(tr("Part"));
            ui.add(
                egui::TextEdit::singleline(&mut c.name)
                    .hint_text(c.id.clone())
                    .desired_width(110.0),
            );
            ui.label(tr("Length"))
                .on_hover_text(tr("How many bars this part lasts"));
            let mut bars = (c.length / bpb).max(1.0).round();
            let unit = if bars == 1.0 { tr(" bar") } else { tr(" bars") };
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
            for (icon, label, st, tip) in [
                (
                    Icon::ArrowDown,
                    tr("Lower"),
                    -12,
                    tr("Play every note in this part one octave lower"),
                ),
                (
                    Icon::ArrowUp,
                    tr("Higher"),
                    12,
                    tr("Play every note in this part one octave higher"),
                ),
            ] {
                if ui
                    .add(icons::button(icon, label))
                    .on_hover_text(tip)
                    .clicked()
                {
                    app.run_op(Op::TransposeClip {
                        clip_id: clip_id.into(),
                        semitones: st,
                    });
                }
            }
        }
        if ui
            .add(icons::button(Icon::Trash, tr("Clear")))
            .on_hover_text(tr("Remove every note in this part (Ctrl+Z to undo)"))
            .clicked()
        {
            app.run_op(Op::RemoveNotes {
                clip_id: clip_id.into(),
                from: None,
                to: None,
                pitch: None,
            });
            app.selected_notes.clear();
        }
        if let Some((count, first)) = first_selected(app, clip_id) {
            ui.separator();
            ui.label(selection_label(count, &first));
            let tip = if count == 1 {
                tr("How hard this note is played")
            } else {
                tr("How hard the selected notes are played")
            };
            for (label, v) in [(tr("Soft"), 50u8), (tr("Medium"), 90), (tr("Loud"), 120)] {
                let on = match v {
                    50 => first.velocity < 70,
                    90 => (70..105).contains(&first.velocity),
                    _ => first.velocity >= 105,
                };
                if ui.selectable_label(on, label).on_hover_text(tip).clicked() {
                    for_selected(app, clip_id, |n| n.velocity = v);
                }
            }
        }
    });
    ui.horizontal_wrapped(|ui| {
        let key = ui.scope(|ui| {
            ui.add_enabled_ui(!drums, |ui| {
                ui.label(tr("🎵 Mood")).on_hover_text(tr(
                    "Pick a feeling. Lit-up rows fit the mood, and every note you draw lands on one of them, so it always sounds right.",
                ));
                for (label, root, sc, tip) in [
                    (tr("😊 Happy"), 0u8, Scale::Major, tr("C major: bright and cheerful")),
                    (tr("😢 Sad"), 9, Scale::Minor, tr("A minor: dark and emotional")),
                    (
                        tr("👌 Can't go wrong"),
                        0,
                        Scale::Pentatonic,
                        tr("C pentatonic: almost anything you draw sounds nice"),
                    ),
                    (tr("🎷 Bluesy"), 9, Scale::Blues, tr("A blues: cool and gritty")),
                ] {
                    let on = app.scale == sc && app.scale_root == root;
                    if ui.selectable_label(on, label).on_hover_text(tip).clicked() {
                        app.scale = sc;
                        app.scale_root = root;
                    }
                }
            })
            .response
            .on_disabled_hover_text(tr("Drums don't have a mood: every row is a different drum"));
        });
        app.ui_rects.insert("key", key.response.rect);
    });
    theme::hint(ui, tr("click to add a note · drag to move · drag the right edge to make longer · right-click to delete · hold a piano key to hear it · Shift+drag to select several"));
}

fn pro_toolbar(ui: &mut egui::Ui, app: &mut App, clip_id: &str, track_index: usize) {
    ui.horizontal_wrapped(|ui| {
        theme::section(ui, tr("Piano roll")).on_hover_text(tr(
            "Each row is a piano key (low at the bottom, high at the top). Left to right is time.",
        ));
        if let Some(c) = app.project.clip_mut(clip_id) {
            ui.label(tr("Clip"));
            ui.add(
                egui::TextEdit::singleline(&mut c.name)
                    .hint_text(c.id.clone())
                    .desired_width(110.0),
            );
            ui.label(tr("Length"))
                .on_hover_text(tr("How long this clip is. 4 beats = 1 bar."));
            ui.add(
                egui::DragValue::new(&mut c.length)
                    .range(0.25..=1024.0)
                    .speed(0.25)
                    .suffix(tr(" beats")),
            );
        }
        ui.separator();
        ui.label(tr("Move all notes"))
            .on_hover_text(tr("Transpose: shift every note in this clip up or down"));
        for (icon, st, tip) in [
            (Icon::ChevronsDown, -12, tr("Down one octave")),
            (Icon::ChevronDown, -1, tr("Down a little (one semitone)")),
            (Icon::ChevronUp, 1, tr("Up a little (one semitone)")),
            (Icon::ChevronsUp, 12, tr("Up one octave")),
        ] {
            if ui
                .add(icons::icon_button(icon, 14.0).small())
                .on_hover_text(tip)
                .clicked()
            {
                app.run_op(Op::TransposeClip {
                    clip_id: clip_id.into(),
                    semitones: st,
                });
            }
        }
        if ui
            .add(icons::button(Icon::Trash, tr("Clear")).small())
            .on_hover_text(tr("Remove every note in this clip (Ctrl+Z to undo)"))
            .clicked()
        {
            app.run_op(Op::RemoveNotes {
                clip_id: clip_id.into(),
                from: None,
                to: None,
                pitch: None,
            });
            app.selected_notes.clear();
        }
        if ui
            .add(icons::button(Icon::Magnet, tr("Quantize")).small())
            .on_hover_text(tr(
                "Snap note starts to the grid (Q): the selected notes, or every note in the clip when none is selected",
            ))
            .clicked()
        {
            app.quantize();
        }
        if ui
            .add(icons::button(Icon::Wave, tr("Humanize")).small())
            .on_hover_text(tr(
                "Make the notes sound played: nudge their timing and loudness a little at random. Works on the selected notes, or the whole clip when none is selected",
            ))
            .clicked()
        {
            app.humanize();
        }
        ui.separator();
        if let Some((count, first)) = first_selected(app, clip_id) {
            ui.label(selection_label(count, &first));
            ui.label(tr("Loudness"))
                .on_hover_text(tr("Velocity: how hard the note is played"));
            let mut velocity = first.velocity;
            if ui.add(egui::Slider::new(&mut velocity, 1..=127)).changed() {
                for_selected(app, clip_id, |n| n.velocity = velocity);
            }
            ui.label(tr("Length"))
                .on_hover_text(tr("Note length in beats"));
            let mut length = first.duration;
            if ui
                .add(
                    egui::DragValue::new(&mut length)
                        .range(0.01..=256.0)
                        .speed(0.05),
                )
                .changed()
            {
                for_selected(app, clip_id, |n| n.duration = length);
            }
        }
    });
    // In a narrow window the zoom sliders get a row of their own.
    let narrow = ui.available_width() < 780.0;
    ui.horizontal_wrapped(|ui| {
        let drums = app.project.tracks.get(track_index).is_some_and(|t| t.instrument.is_drums());
        let key = ui.scope(|ui| {
            ui.add_enabled_ui(!drums, |ui| {
                ui.label(tr("🎵 Key")).on_hover_text(tr(
                    "Notes in the same key sound good together. Lit-up rows are in the key.",
                ));
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
                    egui::Checkbox::new(&mut app.scale_lock, tr("Stay in key")),
                )
                .on_hover_text(tr("New and moved notes jump to the nearest note in the key, so it always sounds right."));
            })
            .response
            .on_disabled_hover_text(tr("Drums don't have a key"));
        });
        app.ui_rects.insert("key", key.response.rect);
        if !narrow {
            ui.separator();
            zoom_controls(ui, app);
        }
    });
    if narrow {
        ui.horizontal(|ui| zoom_controls(ui, app));
    }
    theme::hint(ui, tr("click to add a note · drag to move, right edge to lengthen · right-click deletes · Shift+drag selects · Ctrl+C / V / D copy, paste, repeat · Q quantizes"));
}

fn zoom_controls(ui: &mut egui::Ui, app: &mut App) {
    ui.spacing_mut().slider_width = 80.0;
    ui.label(tr("Zoom"));
    ui.add(
        egui::Slider::new(&mut app.roll_zoom, 20.0..=400.0)
            .logarithmic(true)
            .show_value(false),
    );
    ui.label(tr("Height"));
    ui.add(egui::Slider::new(&mut app.roll_key_height, 8.0..=24.0).show_value(false));
}

fn drum_label(p: u8) -> Option<&'static str> {
    Some(match p {
        36 => tr("Kick"),
        38 => tr("Snare"),
        39 => tr("Clap"),
        42 => tr("C.Hat"),
        46 => tr("O.Hat"),
        41 => tr("Tom L"),
        45 => tr("Tom M"),
        48 => tr("Tom H"),
        49 => tr("Crash"),
        37 => tr("Rim"),
        51 => tr("Ride"),
        53 => tr("Ride bell"),
        54 => tr("Tamb."),
        55 => tr("Splash"),
        56 => tr("Cowbell"),
        60 => tr("Bongo H"),
        61 => tr("Bongo L"),
        62 => tr("Conga M"),
        63 => tr("Conga H"),
        64 => tr("Conga L"),
        65 => tr("Timb. H"),
        66 => tr("Timb. L"),
        67 => tr("Agogo H"),
        68 => tr("Agogo L"),
        70 => tr("Shaker"),
        75 => tr("Claves"),
        76 => tr("Block H"),
        77 => tr("Block L"),
        81 => tr("Triangle"),
        _ => return None,
    })
}
