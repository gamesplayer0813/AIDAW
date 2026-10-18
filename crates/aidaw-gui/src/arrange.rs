//! Arrangement view: one row per track, clips on a beat timeline.
//!
//! Click a clip to edit it in the piano roll, drag to move, drag its right
//! edge to resize, double-click an empty spot to create a clip, Delete to
//! remove the selected clip. Click the ruler to move the playhead.

use eframe::egui::{self, pos2, vec2, Align2, Color32, FontId, Rect, Sense, Stroke, StrokeKind};

use crate::app::{track_color, App, Drag};

const HEADER_W: f32 = 120.0;
const RULER_H: f32 = 22.0;
const ROW_H: f32 = 54.0;
const EDGE: f32 = 6.0;

enum Hit {
    Ruler(f64),
    ClipBody {
        clip_id: String,
        track_id: String,
        offset: f64,
    },
    ClipEdge {
        clip_id: String,
        track_id: String,
    },
    Row {
        track_id: String,
        beat: f64,
    },
    Header {
        track_id: String,
    },
    Nothing,
}

pub fn show(ui: &mut egui::Ui, app: &mut App) {
    ui.horizontal(|ui| {
        ui.heading("Song").on_hover_text(
            "Each row is one instrument (a track). The coloured blocks are clips: pieces of music.\nClick a clip to edit its notes in the piano roll below.",
        );
        ui.add(
            egui::Slider::new(&mut app.arrange_zoom, 8.0..=120.0)
                .logarithmic(true)
                .show_value(false)
                .text("zoom"),
        );
        ui.weak("click a clip to edit it · double-click an empty spot for a new clip · drag to move · drag the right edge to make longer");
    });

    let bpb = app.project.beats_per_bar.max(1) as f64;
    let total_beats = ((app.project.length_beats() / bpb).ceil() + 8.0) * bpb;
    let bw = app.arrange_zoom;
    let rows = app.project.tracks.len();
    let size = vec2(
        HEADER_W + total_beats as f32 * bw,
        RULER_H + rows as f32 * ROW_H + ROW_H,
    );

    egui::ScrollArea::both()
        .id_salt("arrange")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let (resp, painter) =
                ui.allocate_painter(size.max(ui.available_size()), Sense::click_and_drag());
            app.ui_rects
                .insert("arrange", ui.clip_rect().intersect(resp.rect));
            let origin = resp.rect.min;
            let x_of = |beat: f64| origin.x + HEADER_W + beat as f32 * bw;
            let beat_of = |x: f32| ((x - origin.x - HEADER_W) / bw) as f64;
            let row_top = |i: usize| origin.y + RULER_H + i as f32 * ROW_H;
            let visuals = ui.visuals().clone();

            // Background, grid and ruler.
            painter.rect_filled(resp.rect, 0.0, visuals.extreme_bg_color);
            for i in 0..rows {
                let r =
                    Rect::from_min_size(pos2(origin.x, row_top(i)), vec2(resp.rect.width(), ROW_H));
                if i % 2 == 1 {
                    painter.rect_filled(r, 0.0, Color32::from_white_alpha(4));
                }
            }
            let bottom = resp.rect.bottom();
            let mut b = 0.0;
            while b <= total_beats {
                let x = x_of(b);
                let is_bar = (b % bpb).abs() < 1e-9;
                let col = if is_bar {
                    Color32::from_white_alpha(40)
                } else {
                    Color32::from_white_alpha(14)
                };
                painter.line_segment(
                    [pos2(x, origin.y + RULER_H), pos2(x, bottom)],
                    Stroke::new(1.0_f32, col),
                );
                if is_bar {
                    painter.text(
                        pos2(x + 3.0, origin.y + 4.0),
                        Align2::LEFT_TOP,
                        format!("{}", (b / bpb) as i64 + 1),
                        FontId::monospace(11.0),
                        visuals.text_color(),
                    );
                }
                b += 1.0;
            }
            painter.line_segment(
                [
                    pos2(origin.x, origin.y + RULER_H),
                    pos2(resp.rect.right(), origin.y + RULER_H),
                ],
                Stroke::new(1.0_f32, Color32::from_white_alpha(60)),
            );

            // Clips.
            for (i, track) in app.project.tracks.iter().enumerate() {
                let color = track_color(i);
                let top = row_top(i);
                for clip in &track.clips {
                    let r = Rect::from_min_max(
                        pos2(x_of(clip.start), top + 3.0),
                        pos2(x_of(clip.end()), top + ROW_H - 3.0),
                    );
                    let selected = app.selected_clip.as_deref() == Some(clip.id.as_str());
                    painter.rect_filled(
                        r,
                        4.0,
                        color.gamma_multiply(if selected { 0.55 } else { 0.32 }),
                    );
                    painter.rect_stroke(
                        r,
                        4.0,
                        Stroke::new(if selected { 2.0_f32 } else { 1.0 }, color),
                        StrokeKind::Inside,
                    );
                    // Mini note preview.
                    if !clip.notes.is_empty() {
                        let (lo, hi) = clip.notes.iter().fold((127u8, 0u8), |(lo, hi), n| {
                            (lo.min(n.pitch), hi.max(n.pitch))
                        });
                        let span = (hi - lo).max(1) as f32;
                        let inner = r.shrink2(vec2(2.0, 14.0));
                        for n in &clip.notes {
                            if n.start >= clip.length {
                                continue;
                            }
                            let y = inner.bottom()
                                - (n.pitch - lo) as f32 / span * (inner.height() - 2.0);
                            let x0 = x_of(clip.start + n.start);
                            let x1 = x_of(clip.start + (n.start + n.duration).min(clip.length))
                                .max(x0 + 1.0);
                            painter.line_segment(
                                [pos2(x0, y), pos2(x1, y)],
                                Stroke::new(2.0_f32, color),
                            );
                        }
                    }
                    let label = if clip.name.is_empty() {
                        &clip.id
                    } else {
                        &clip.name
                    };
                    painter.text(
                        r.left_top() + vec2(4.0, 1.0),
                        Align2::LEFT_TOP,
                        label,
                        FontId::proportional(11.0),
                        visuals.strong_text_color(),
                    );
                }
            }

            // Friendly hint in rows that have nothing in them yet.
            for (i, track) in app.project.tracks.iter().enumerate() {
                if track.clips.is_empty() {
                    painter.text(
                        pos2(x_of(0.0) + 10.0, row_top(i) + ROW_H / 2.0),
                        Align2::LEFT_CENTER,
                        "Double-click here to add a clip",
                        FontId::proportional(13.0),
                        visuals.weak_text_color(),
                    );
                }
            }
            if rows == 0 {
                painter.text(
                    pos2(origin.x + 20.0, origin.y + RULER_H + 24.0),
                    Align2::LEFT_CENTER,
                    "No tracks yet. Click ➕ Add instrument on the left.",
                    FontId::proportional(15.0),
                    visuals.text_color(),
                );
            }

            // Track headers (drawn last so they cover clips scrolled under them).
            for (i, track) in app.project.tracks.iter().enumerate() {
                let r = Rect::from_min_size(pos2(origin.x, row_top(i)), vec2(HEADER_W, ROW_H));
                let selected = app.selected_track.as_deref() == Some(track.id.as_str());
                painter.rect_filled(
                    r,
                    0.0,
                    if selected {
                        visuals.selection.bg_fill.gamma_multiply(0.5)
                    } else {
                        visuals.panel_fill
                    },
                );
                painter.rect_filled(
                    Rect::from_min_size(r.min, vec2(4.0, ROW_H)),
                    0.0,
                    track_color(i),
                );
                painter.text(
                    r.left_top() + vec2(10.0, 8.0),
                    Align2::LEFT_TOP,
                    &track.name,
                    FontId::proportional(13.0),
                    visuals.strong_text_color(),
                );
                let flags = format!(
                    "{}{}",
                    if track.mute { "M " } else { "" },
                    if track.solo { "S" } else { "" }
                );
                painter.text(
                    r.left_bottom() + vec2(10.0, -8.0),
                    Align2::LEFT_BOTTOM,
                    flags,
                    FontId::monospace(11.0),
                    Color32::YELLOW,
                );
            }

            // Playhead.
            let px = x_of(app.playhead);
            painter.line_segment(
                [pos2(px, origin.y), pos2(px, bottom)],
                Stroke::new(1.5_f32, Color32::from_rgb(255, 80, 80)),
            );

            // ---- interaction ----
            let hit = |project: &aidaw_core::Project, p: egui::Pos2| -> Hit {
                if p.y < origin.y + RULER_H {
                    return Hit::Ruler(beat_of(p.x).max(0.0));
                }
                let row = ((p.y - origin.y - RULER_H) / ROW_H) as usize;
                let Some(track) = project.tracks.get(row) else {
                    return Hit::Nothing;
                };
                if p.x < origin.x + HEADER_W {
                    return Hit::Header {
                        track_id: track.id.clone(),
                    };
                }
                let beat = beat_of(p.x);
                for clip in track.clips.iter().rev() {
                    let (x0, x1) = (x_of(clip.start), x_of(clip.end()));
                    if p.x >= x0 && p.x <= x1 {
                        let ids = (clip.id.clone(), track.id.clone());
                        return if x1 - p.x <= EDGE {
                            Hit::ClipEdge {
                                clip_id: ids.0,
                                track_id: ids.1,
                            }
                        } else {
                            Hit::ClipBody {
                                clip_id: ids.0,
                                track_id: ids.1,
                                offset: beat - clip.start,
                            }
                        };
                    }
                }
                Hit::Row {
                    track_id: track.id.clone(),
                    beat,
                }
            };

            if let Some(p) = resp.hover_pos() {
                if matches!(hit(&app.project, p), Hit::ClipEdge { .. }) {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
                }
            }

            if resp.drag_started() {
                if let Some(p) = ui.input(|i| i.pointer.press_origin()) {
                    match hit(&app.project, p) {
                        Hit::ClipBody {
                            clip_id,
                            track_id,
                            offset,
                        } => {
                            select_clip(app, &clip_id, &track_id);
                            app.drag = Some(Drag::MoveClip {
                                clip_id,
                                grab_offset: offset,
                            });
                        }
                        Hit::ClipEdge { clip_id, track_id } => {
                            select_clip(app, &clip_id, &track_id);
                            app.drag = Some(Drag::ResizeClip { clip_id });
                        }
                        Hit::Ruler(b) => app.seek(b),
                        _ => {}
                    }
                }
            }
            if resp.dragged() {
                if let Some(p) = resp.interact_pointer_pos() {
                    let beat = beat_of(p.x);
                    match app.drag.clone() {
                        Some(Drag::MoveClip {
                            clip_id,
                            grab_offset,
                        }) => {
                            let start = app.snap_beat(beat - grab_offset).max(0.0);
                            if let Some(c) = app.project.clip_mut(&clip_id) {
                                c.start = start;
                            }
                        }
                        Some(Drag::ResizeClip { clip_id }) => {
                            let snapped = app.snap_beat(beat);
                            let snap = app.snap;
                            if let Some(c) = app.project.clip_mut(&clip_id) {
                                c.length = (snapped - c.start).max(snap);
                            }
                        }
                        _ => {
                            if p.y < origin.y + RULER_H {
                                app.seek(beat.max(0.0));
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
                    match hit(&app.project, p) {
                        Hit::Ruler(b) => app.seek(b),
                        Hit::ClipBody {
                            clip_id, track_id, ..
                        }
                        | Hit::ClipEdge { clip_id, track_id } => {
                            select_clip(app, &clip_id, &track_id)
                        }
                        Hit::Row { track_id, .. } | Hit::Header { track_id } => {
                            app.selected_track = Some(track_id);
                        }
                        Hit::Nothing => {}
                    }
                }
            }
            if resp.double_clicked() {
                if let Some(p) = resp.interact_pointer_pos() {
                    if let Hit::Row { track_id, beat } = hit(&app.project, p) {
                        let start = (beat / bpb).floor() * bpb;
                        app.add_clip(&track_id, start.max(0.0));
                    }
                }
            }
            if resp.hovered() || resp.has_focus() {
                let del = ui.input(|i| {
                    i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace)
                });
                if del && !ui.ctx().wants_keyboard_input() {
                    app.delete_selected_clip();
                }
            }
        });
}

fn select_clip(app: &mut App, clip_id: &str, track_id: &str) {
    if app.selected_clip.as_deref() != Some(clip_id) {
        app.selected_note = None;
        app.roll_scroll_to_notes = true;
    }
    app.selected_clip = Some(clip_id.to_string());
    app.selected_track = Some(track_id.to_string());
}
