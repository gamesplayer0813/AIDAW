//! Arrangement view: one row per track, clips on a beat timeline.
//!
//! Click a clip to edit it in the piano roll, drag to move, drag its right
//! edge to resize, double-click an empty spot to create a clip. Clicking
//! here points the editing keys (Delete, Ctrl+C/X/V/D, arrows) at the
//! selected clip. Click the ruler to move the playhead.

use aidaw_core::presets;
use eframe::egui::{
    self, pos2, vec2, Align2, Color32, CornerRadius, FontId, Rect, Sense, Stroke, StrokeKind,
};

use crate::app::{track_color, App, Drag, EditFocus};
use crate::i18n::tr;
use crate::theme;

const HEADER_W: f32 = 150.0;
const RULER_H: f32 = 26.0;
const ROW_H: f32 = 58.0;
/// Height of the name band on top of a clip.
const CLIP_BAND: f32 = 15.0;
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
    ui.horizontal_wrapped(|ui| {
        if !app.pro() {
            theme::section(ui, tr("Your song")).on_hover_text(tr(
                "Each row is one instrument. The coloured blocks are parts of the song.\nClick a part to see and edit its notes below.",
            ));
            theme::hint(ui, tr("click a part to edit its notes · double-click an empty spot for a new part · drag to move · Ctrl+D repeats the part"));
            return;
        }
        theme::section(ui, tr("Song")).on_hover_text(tr(
            "Each row is one instrument (a track). The coloured blocks are clips: pieces of music.\nClick a clip to edit its notes in the piano roll below.",
        ));
        ui.add(
            egui::Slider::new(&mut app.arrange_zoom, 8.0..=120.0)
                .logarithmic(true)
                .show_value(false)
                .text(tr("zoom")),
        );
        theme::hint(ui, tr("click a clip to edit it · double-click an empty spot for a new clip · drag to move · drag the right edge to make longer · Ctrl+D duplicates"));
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

            // Background, rows and grid.
            painter.rect_filled(resp.rect, 0.0, theme::CANVAS);
            for i in 0..rows {
                let r =
                    Rect::from_min_size(pos2(origin.x, row_top(i)), vec2(resp.rect.width(), ROW_H));
                if i % 2 == 1 {
                    painter.rect_filled(r, 0.0, Color32::from_white_alpha(3));
                }
                painter.line_segment(
                    [r.left_bottom(), r.right_bottom()],
                    Stroke::new(1.0_f32, theme::alpha(theme::BORDER, 150)),
                );
            }
            let bottom = resp.rect.bottom();
            let ruler = Rect::from_min_size(origin, vec2(resp.rect.width(), RULER_H));
            painter.rect_filled(ruler, 0.0, theme::PANEL);
            // Where live playback loops back to the start.
            if let Some(end) = app.project.loop_end_beats() {
                let band = Rect::from_min_max(
                    pos2(x_of(0.0), ruler.bottom() - 4.0),
                    pos2(x_of(end), ruler.bottom()),
                );
                painter.rect_filled(band, 2.0, theme::alpha(theme::ACCENT, 110));
            }
            let mut b = 0.0;
            while b <= total_beats {
                let x = x_of(b);
                let is_bar = (b % bpb).abs() < 1e-9;
                let col = if is_bar {
                    Color32::from_white_alpha(22)
                } else {
                    Color32::from_white_alpha(7)
                };
                painter.line_segment(
                    [pos2(x, origin.y + RULER_H), pos2(x, bottom)],
                    Stroke::new(1.0_f32, col),
                );
                let tick = if is_bar { 10.0 } else { 4.0 };
                painter.line_segment(
                    [pos2(x, ruler.bottom() - tick), pos2(x, ruler.bottom())],
                    Stroke::new(
                        1.0_f32,
                        if is_bar {
                            theme::BORDER_HI
                        } else {
                            theme::BORDER
                        },
                    ),
                );
                if is_bar {
                    painter.text(
                        pos2(x + 4.0, origin.y + 4.0),
                        Align2::LEFT_TOP,
                        format!("{}", (b / bpb) as i64 + 1),
                        theme::bold_font(11.0),
                        theme::TEXT_DIM,
                    );
                }
                b += 1.0;
            }
            painter.line_segment(
                [ruler.left_bottom(), ruler.right_bottom()],
                Stroke::new(1.0_f32, theme::BORDER),
            );

            // Clips: a coloured name band on top, a tinted body with a mini
            // view of the notes.
            for (i, track) in app.project.tracks.iter().enumerate() {
                let color = track_color(i);
                let top = row_top(i);
                for clip in &track.clips {
                    let r = Rect::from_min_max(
                        pos2(x_of(clip.start) + 1.0, top + 5.0),
                        pos2(x_of(clip.end()) - 1.0, top + ROW_H - 5.0),
                    );
                    let selected = app.selected_clip.as_deref() == Some(clip.id.as_str());
                    // White outline: the editing keys act on this clip.
                    let keys_here = selected && app.edit_focus == EditFocus::Clips;
                    if selected {
                        theme::glow(&painter, r, 7, theme::alpha(color, 120), 16);
                    }
                    painter.rect_filled(
                        r,
                        7.0,
                        theme::mix(theme::CANVAS, color, if selected { 0.3 } else { 0.18 }),
                    );
                    let band = Rect::from_min_size(r.min, vec2(r.width(), CLIP_BAND));
                    painter.rect_filled(
                        band,
                        CornerRadius {
                            nw: 7,
                            ne: 7,
                            sw: 0,
                            se: 0,
                        },
                        if selected {
                            color
                        } else {
                            theme::mix(theme::CANVAS, color, 0.72)
                        },
                    );
                    // Mini note preview.
                    if !clip.notes.is_empty() {
                        let (lo, hi) = clip.notes.iter().fold((127u8, 0u8), |(lo, hi), n| {
                            (lo.min(n.pitch), hi.max(n.pitch))
                        });
                        let span = (hi - lo).max(1) as f32;
                        let inner = Rect::from_min_max(
                            pos2(r.left() + 3.0, band.bottom() + 5.0),
                            pos2(r.right() - 3.0, r.bottom() - 5.0),
                        );
                        let ink = theme::mix(color, Color32::WHITE, 0.3);
                        for n in &clip.notes {
                            if n.start >= clip.length {
                                continue;
                            }
                            let y = inner.bottom() - (n.pitch - lo) as f32 / span * inner.height();
                            let x0 = x_of(clip.start + n.start) + 1.0;
                            let x1 = (x_of(clip.start + (n.start + n.duration).min(clip.length))
                                - 1.0)
                                .max(x0 + 2.0);
                            painter.rect_filled(
                                Rect::from_min_max(pos2(x0, y - 1.25), pos2(x1, y + 1.25)),
                                1.0,
                                ink,
                            );
                        }
                    }
                    painter.rect_stroke(
                        r,
                        7.0,
                        if keys_here {
                            Stroke::new(1.5_f32, Color32::WHITE)
                        } else if selected {
                            Stroke::new(1.5_f32, theme::alpha(Color32::WHITE, 110))
                        } else {
                            Stroke::new(1.0_f32, theme::alpha(color, 140))
                        },
                        StrokeKind::Inside,
                    );
                    let label = if clip.name.is_empty() {
                        &clip.id
                    } else {
                        &clip.name
                    };
                    painter
                        .with_clip_rect(band.shrink2(vec2(4.0, 0.0)).intersect(painter.clip_rect()))
                        .text(
                            band.left_center() + vec2(7.0, 0.0),
                            Align2::LEFT_CENTER,
                            label,
                            theme::bold_font(10.5),
                            theme::CANVAS,
                        );
                }
            }

            // Friendly hint in rows that have nothing in them yet.
            for (i, track) in app.project.tracks.iter().enumerate() {
                if track.clips.is_empty() {
                    painter.text(
                        pos2(x_of(0.0) + 12.0, row_top(i) + ROW_H / 2.0),
                        Align2::LEFT_CENTER,
                        tr("Double-click here to add a clip"),
                        FontId::proportional(13.0),
                        theme::TEXT_FAINT,
                    );
                }
            }
            if rows == 0 {
                painter.text(
                    pos2(origin.x + 20.0, origin.y + RULER_H + 24.0),
                    Align2::LEFT_CENTER,
                    tr("No tracks yet. Click + Add instrument on the left."),
                    FontId::proportional(15.0),
                    theme::TEXT_DIM,
                );
            }

            // Track headers (drawn last so they cover clips scrolled under them).
            painter.rect_filled(
                Rect::from_min_max(
                    pos2(origin.x, origin.y + RULER_H),
                    pos2(origin.x + HEADER_W, row_top(rows)),
                ),
                0.0,
                theme::PANEL,
            );
            for (i, track) in app.project.tracks.iter().enumerate() {
                let color = track_color(i);
                let r = Rect::from_min_size(pos2(origin.x, row_top(i)), vec2(HEADER_W, ROW_H));
                let card = r.shrink2(vec2(6.0, 4.0));
                let selected = app.selected_track.as_deref() == Some(track.id.as_str());
                painter.rect_filled(
                    card,
                    8.0,
                    if selected {
                        theme::mix(theme::SURFACE, color, 0.16)
                    } else {
                        theme::SURFACE
                    },
                );
                if selected {
                    painter.rect_stroke(
                        card,
                        8.0,
                        Stroke::new(1.0_f32, theme::alpha(color, 170)),
                        StrokeKind::Inside,
                    );
                }
                painter.rect_filled(
                    Rect::from_min_size(card.min + vec2(7.0, 9.0), vec2(4.0, card.height() - 18.0)),
                    2.0,
                    color,
                );
                let text = painter.with_clip_rect(card.shrink(2.0).intersect(painter.clip_rect()));
                text.text(
                    card.left_top() + vec2(19.0, 9.0),
                    Align2::LEFT_TOP,
                    &track.name,
                    theme::bold_font(13.0),
                    theme::TEXT,
                );
                let sound = presets::name_of(&track.instrument)
                    .map(|n| presets::plain_name(tr(n)))
                    .unwrap_or(tr("Custom sound"));
                text.text(
                    card.left_top() + vec2(19.0, 28.0),
                    Align2::LEFT_TOP,
                    sound,
                    FontId::proportional(11.0),
                    theme::TEXT_DIM,
                );
                let mut x = card.right() - 6.0;
                for (on, flag, c) in [
                    (track.solo, "S", theme::CYAN),
                    (track.mute, "M", theme::WARN),
                ] {
                    if on {
                        x = theme::pill(&painter, pos2(x, card.top() + 8.0), flag, c).left() - 4.0;
                    }
                }
            }
            painter.line_segment(
                [
                    pos2(origin.x + HEADER_W, origin.y + RULER_H),
                    pos2(origin.x + HEADER_W, bottom),
                ],
                Stroke::new(1.0_f32, theme::BORDER),
            );

            // Playhead, with a marker in the ruler.
            let px = x_of(app.playhead);
            painter.line_segment(
                [pos2(px, origin.y + RULER_H), pos2(px, bottom)],
                Stroke::new(5.0_f32, theme::alpha(theme::PLAYHEAD, 40)),
            );
            painter.line_segment(
                [pos2(px, origin.y + RULER_H), pos2(px, bottom)],
                Stroke::new(1.5_f32, theme::PLAYHEAD),
            );
            painter.add(egui::Shape::convex_polygon(
                vec![
                    pos2(px - 6.0, origin.y + RULER_H - 10.0),
                    pos2(px + 6.0, origin.y + RULER_H - 10.0),
                    pos2(px, origin.y + RULER_H),
                ],
                theme::PLAYHEAD,
                Stroke::NONE,
            ));

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
                    let target = hit(&app.project, p);
                    if !matches!(target, Hit::Ruler(_) | Hit::Nothing) {
                        app.edit_focus = EditFocus::Clips;
                    }
                    match target {
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
                            // Easy mode moves parts by whole bars so they always line up.
                            let start = if app.pro() {
                                app.snap_beat(beat - grab_offset)
                            } else {
                                ((beat - grab_offset) / bpb).round() * bpb
                            }
                            .max(0.0);
                            if let Some(c) = app.project.clip_mut(&clip_id) {
                                c.start = start;
                            }
                        }
                        Some(Drag::ResizeClip { clip_id }) => {
                            let (snapped, snap) = if app.pro() {
                                (app.snap_beat(beat), app.grid())
                            } else {
                                ((beat / bpb).round() * bpb, bpb)
                            };
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
                    let target = hit(&app.project, p);
                    if !matches!(target, Hit::Ruler(_) | Hit::Nothing) {
                        app.edit_focus = EditFocus::Clips;
                    }
                    match target {
                        Hit::Ruler(b) => app.seek(b),
                        Hit::ClipBody {
                            clip_id, track_id, ..
                        }
                        | Hit::ClipEdge { clip_id, track_id } => {
                            select_clip(app, &clip_id, &track_id)
                        }
                        Hit::Header { track_id } => {
                            // Show this track's notes, not the previous track's clip.
                            let keep = app.selected_clip.as_deref().is_some_and(|id| {
                                app.project.clip(id).is_some_and(|(t, _)| t.id == track_id)
                            });
                            let first = app
                                .project
                                .track(&track_id)
                                .and_then(|t| t.clips.first())
                                .map(|c| c.id.clone());
                            match first {
                                _ if keep => app.selected_track = Some(track_id),
                                Some(clip_id) => select_clip(app, &clip_id, &track_id),
                                None => {
                                    app.selected_clip = None;
                                    app.selected_notes.clear();
                                    app.selected_track = Some(track_id);
                                }
                            }
                        }
                        Hit::Row { track_id, .. } => {
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
        });
}

/// Open a clip in the piano roll. Its notes start unselected, since the
/// editing keys now act on the clip itself.
fn select_clip(app: &mut App, clip_id: &str, track_id: &str) {
    if app.selected_clip.as_deref() != Some(clip_id) {
        app.roll_scroll_to_notes = true;
    }
    app.selected_notes.clear();
    app.selected_clip = Some(clip_id.to_string());
    app.selected_track = Some(track_id.to_string());
}
