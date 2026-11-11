//! Arrangement view: one row per track, clips on a beat timeline.
//!
//! Click a clip to edit it in the piano roll, drag to move (Alt+drag
//! copies), drag its right edge to resize, double-click an empty spot to
//! create a clip. Clicking here points the editing keys (Delete,
//! Ctrl+C/X/V/D, arrows) at the selected clip. Click the ruler to move the
//! playhead; the thin strip on top of it holds the loop region. Drag a
//! track's name to reorder the tracks; right-click anything for more.

use aidaw_core::{presets, LoopRegion, Project};
use eframe::egui::{
    self, pos2, vec2, Align2, Color32, CornerRadius, CursorIcon, FontId, Pos2, Rect, Sense, Stroke,
    StrokeKind,
};

use crate::app::{track_color, App, Drag, EditFocus};
use crate::i18n::{tr, trf};
use crate::icons::{self, Icon};
use crate::theme;

const HEADER_W: f32 = 150.0;
/// The loop strip on top of the ruler.
const LOOP_H: f32 = 12.0;
const RULER_H: f32 = LOOP_H + 24.0;
const ROW_H: f32 = 58.0;
/// Height of the name band on top of a clip.
const CLIP_BAND: f32 = 15.0;
const EDGE: f32 = 6.0;

/// Where on the loop region the pointer is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum LoopPart {
    Outside,
    Inside,
    Start,
    End,
}

enum Hit {
    /// The loop strip on top of the ruler.
    Loop {
        beat: f64,
        part: LoopPart,
    },
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

/// What was right-clicked, kept while its menu is open.
#[derive(Clone, Debug)]
enum Menu {
    Clip {
        clip_id: String,
        beat: f64,
    },
    /// A track's header, or an empty spot in its row (`beat`).
    Track {
        track_id: String,
        beat: Option<f64>,
    },
    Ruler {
        beat: f64,
    },
    Nothing,
}

pub fn show(ui: &mut egui::Ui, app: &mut App) {
    ui.horizontal_wrapped(|ui| {
        if !app.pro() {
            theme::section(ui, tr("Your song")).on_hover_text(tr(
                "Each row is one instrument. The coloured blocks are parts of the song.\nClick a part to see and edit its notes below.",
            ));
            theme::hint(ui, tr("click a part to edit its notes · double-click an empty spot for a new part · drag to move · Ctrl+D repeats the part · S cuts it at the playhead · right-click for more"));
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
        theme::hint(ui, tr("click a clip to edit it · double-click for a new clip · drag to move, Alt+drag to copy · drag the right edge to make longer · S splits · right-click for more"));
    });

    let bpb = app.project.beats_per_bar.max(1) as f64;
    let loop_end = app.project.transport.loop_region.map_or(0.0, |r| r.end);
    let total_beats = ((app.project.length_beats().max(loop_end) / bpb).ceil() + 8.0) * bpb;
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

            // The AI moved the playhead: bring it into view.
            if std::mem::take(&mut app.reveal_playhead) {
                let x = x_of(app.playhead);
                ui.scroll_to_rect(
                    Rect::from_min_max(pos2(x - 60.0, origin.y), pos2(x + 240.0, origin.y + RULER_H)),
                    None,
                );
            }

            let region = app.project.transport.loop_region.filter(LoopRegion::is_valid);
            let active = app.project.active_loop();

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
            // The part of the song that repeats, lightly tinted.
            if let Some((start, end)) = active {
                painter.rect_filled(
                    Rect::from_min_max(pos2(x_of(start), origin.y + RULER_H), pos2(x_of(end), bottom)),
                    0.0,
                    theme::alpha(theme::ACCENT, 12),
                );
            }
            let ruler = Rect::from_min_size(origin, vec2(resp.rect.width(), RULER_H));
            painter.rect_filled(ruler, 0.0, theme::PANEL);
            let strip = Rect::from_min_size(origin, vec2(resp.rect.width(), LOOP_H));
            painter.rect_filled(strip, 0.0, theme::mix(theme::PANEL, theme::CANVAS, 0.6));
            painter.text(
                pos2(origin.x + 8.0, strip.center().y),
                Align2::LEFT_CENTER,
                tr("LOOP"),
                theme::bold_font(8.5),
                theme::TEXT_FAINT,
            );
            if let Some(r) = region {
                let band = Rect::from_min_max(
                    pos2(x_of(r.start), strip.top() + 2.0),
                    pos2(x_of(r.end), strip.bottom() - 1.0),
                );
                if r.on {
                    painter.rect_filled(band, 3.0, theme::alpha(theme::ACCENT, 215));
                } else {
                    // Kept for later: an outline only.
                    painter.rect_filled(band, 3.0, theme::alpha(theme::ACCENT, 26));
                    painter.rect_stroke(
                        band,
                        3.0,
                        Stroke::new(1.0_f32, theme::alpha(theme::ACCENT, 130)),
                        StrokeKind::Inside,
                    );
                }
                if band.width() > 24.0 {
                    let mark = Rect::from_center_size(
                        pos2(band.left() + 9.0, band.center().y),
                        vec2(8.0, 8.0),
                    );
                    let ink = if r.on {
                        theme::CANVAS
                    } else {
                        theme::alpha(theme::ACCENT, 220)
                    };
                    icons::paint(ui, mark, Icon::Repeat, ink);
                }
            }
            // Without a loop, live playback goes back to the start here.
            if active.is_none() {
                if let Some(end) = app.project.loop_end_beats() {
                    let band = Rect::from_min_max(
                        pos2(x_of(0.0), ruler.bottom() - 4.0),
                        pos2(x_of(end), ruler.bottom()),
                    );
                    painter.rect_filled(band, 2.0, theme::alpha(theme::ACCENT, 110));
                }
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
                        pos2(x + 4.0, strip.bottom() + 4.0),
                        Align2::LEFT_TOP,
                        format!("{}", (b / bpb) as i64 + 1),
                        theme::bold_font(11.0),
                        theme::TEXT_DIM,
                    );
                }
                b += 1.0;
            }
            painter.line_segment(
                [strip.left_bottom(), strip.right_bottom()],
                Stroke::new(1.0_f32, theme::alpha(theme::BORDER, 120)),
            );
            painter.line_segment(
                [ruler.left_bottom(), ruler.right_bottom()],
                Stroke::new(1.0_f32, theme::BORDER),
            );

            // Clips: a coloured name band on top, a tinted body with a mini
            // view of the notes.
            for (i, track) in app.project.tracks.iter().enumerate() {
                let color = track_color(track);
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
            let moving = match &app.drag {
                Some(Drag::MoveTrack { track_id }) => Some(track_id.as_str()),
                _ => None,
            };
            for (i, track) in app.project.tracks.iter().enumerate() {
                let color = track_color(track);
                let r = Rect::from_min_size(pos2(origin.x, row_top(i)), vec2(HEADER_W, ROW_H));
                let card = r.shrink2(vec2(6.0, 4.0));
                let selected = app.selected_track.as_deref() == Some(track.id.as_str());
                let lifted = moving == Some(track.id.as_str());
                if lifted {
                    theme::glow(&painter, card, 8, theme::alpha(color, 110), 14);
                }
                painter.rect_filled(
                    card,
                    8.0,
                    if selected || lifted {
                        theme::mix(theme::SURFACE, color, 0.16)
                    } else {
                        theme::SURFACE
                    },
                );
                if selected || lifted {
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
                let sound = match track.instrument.plugin() {
                    Some(p) => p.name.as_str(),
                    None => presets::name_of(&track.instrument)
                        .map(|n| presets::plain_name(tr(n)))
                        .unwrap_or(tr("Custom sound")),
                };
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

            // The loop's ends, over the clips.
            if let Some((start, end)) = active {
                for x in [x_of(start), x_of(end)] {
                    painter.line_segment(
                        [pos2(x, strip.top()), pos2(x, bottom)],
                        Stroke::new(1.0_f32, theme::alpha(theme::ACCENT, 140)),
                    );
                }
            }

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
            let hit = |project: &Project, p: Pos2| -> Hit {
                if p.y < origin.y + LOOP_H {
                    let beat = beat_of(p.x).max(0.0);
                    let part = match project.transport.loop_region.filter(LoopRegion::is_valid) {
                        Some(r) => {
                            let (x0, x1) = (x_of(r.start), x_of(r.end));
                            let edge = EDGE.min((x1 - x0) / 3.0);
                            if (p.x - x0).abs() <= edge {
                                LoopPart::Start
                            } else if (p.x - x1).abs() <= edge {
                                LoopPart::End
                            } else if p.x > x0 && p.x < x1 {
                                LoopPart::Inside
                            } else {
                                LoopPart::Outside
                            }
                        }
                        None => LoopPart::Outside,
                    };
                    return Hit::Loop { beat, part };
                }
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

            // Cursor and a tip for the loop strip.
            let mut tip = None;
            let cursor = match &app.drag {
                Some(Drag::LoopMove { .. } | Drag::MoveTrack { .. }) => Some(CursorIcon::Grabbing),
                Some(Drag::LoopEdge { .. } | Drag::LoopNew { .. }) => {
                    Some(CursorIcon::ResizeHorizontal)
                }
                Some(_) => None,
                None => resp.hover_pos().and_then(|p| match hit(&app.project, p) {
                    Hit::ClipEdge { .. } => Some(CursorIcon::ResizeHorizontal),
                    Hit::Header { .. } => Some(CursorIcon::Grab),
                    Hit::Loop { part, .. } => {
                        tip = Some(if part == LoopPart::Outside {
                            tr("Drag here to loop a part of the song (L turns the loop on and off)")
                        } else {
                            tr("The loop: drag it to move it, drag an end to change it, click it to turn it on or off")
                        });
                        match part {
                            LoopPart::Start | LoopPart::End => Some(CursorIcon::ResizeHorizontal),
                            LoopPart::Inside => Some(CursorIcon::Grab),
                            LoopPart::Outside => None,
                        }
                    }
                    _ => None,
                }),
            };
            if let Some(c) = cursor {
                ui.ctx().set_cursor_icon(c);
            }
            let resp = match tip {
                Some(t) => resp.on_hover_text_at_pointer(t),
                None => resp,
            };

            // Drags are left-button only: the right button opens menus.
            if resp.drag_started_by(egui::PointerButton::Primary) {
                if let Some(p) = ui.input(|i| i.pointer.press_origin()) {
                    let target = hit(&app.project, p);
                    if !matches!(target, Hit::Ruler(_) | Hit::Loop { .. } | Hit::Nothing) {
                        app.edit_focus = EditFocus::Clips;
                    }
                    match target {
                        Hit::ClipBody {
                            clip_id,
                            track_id,
                            offset,
                        } => {
                            // Alt+drag leaves the clip where it is and drags a copy.
                            let copy = ui
                                .input(|i| i.modifiers.alt)
                                .then(|| app.copy_clip_in_place(&clip_id))
                                .flatten();
                            let (dragged, copy_of) = match copy {
                                Some(id) => (id, Some(clip_id)),
                                None => (clip_id, None),
                            };
                            select_clip(app, &dragged, &track_id);
                            app.drag = Some(Drag::MoveClip {
                                clip_id: dragged,
                                grab_offset: offset,
                                copy_of,
                            });
                        }
                        Hit::ClipEdge { clip_id, track_id } => {
                            select_clip(app, &clip_id, &track_id);
                            app.drag = Some(Drag::ResizeClip { clip_id });
                        }
                        Hit::Header { track_id } => {
                            select_track(app, &track_id);
                            app.drag = Some(Drag::MoveTrack { track_id });
                        }
                        Hit::Loop { beat, part } => {
                            let start = app.project.transport.loop_region.map_or(0.0, |r| r.start);
                            app.drag = Some(match part {
                                LoopPart::Inside => Drag::LoopMove {
                                    grab: beat - start,
                                },
                                LoopPart::Start => Drag::LoopEdge { start: true },
                                LoopPart::End => Drag::LoopEdge { start: false },
                                LoopPart::Outside => Drag::LoopNew {
                                    anchor: app.snap_bar_or_grid(beat),
                                },
                            });
                        }
                        Hit::Ruler(b) => app.seek(b),
                        _ => {}
                    }
                }
            }
            if resp.dragged_by(egui::PointerButton::Primary) {
                if let Some(p) = resp.interact_pointer_pos() {
                    let beat = beat_of(p.x);
                    match app.drag.clone() {
                        Some(Drag::MoveClip {
                            clip_id,
                            grab_offset,
                            ..
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
                        Some(Drag::MoveTrack { track_id }) => {
                            let row = ((p.y - origin.y - RULER_H) / ROW_H).floor().max(0.0) as usize;
                            let row = row.min(rows.saturating_sub(1));
                            let now = app.project.tracks.iter().position(|t| t.id == track_id);
                            if now.is_some_and(|i| i != row) {
                                app.move_track(&track_id, row);
                            }
                        }
                        Some(Drag::LoopNew { anchor }) => {
                            let at = app.snap_bar_or_grid(beat.max(0.0));
                            let (start, end) = if at < anchor {
                                (at, anchor)
                            } else {
                                (anchor, at.max(anchor + app.bar_or_grid()))
                            };
                            app.set_loop(start, end);
                            app.status = app.loop_status();
                        }
                        Some(Drag::LoopMove { grab }) => {
                            if let Some(r) = app.project.transport.loop_region {
                                let start = app.snap_bar_or_grid(beat - grab).max(0.0);
                                app.set_loop(start, start + (r.end - r.start));
                                app.status = app.loop_status();
                            }
                        }
                        Some(Drag::LoopEdge { start }) => {
                            if let Some(r) = app.project.transport.loop_region {
                                let (at, step) =
                                    (app.snap_bar_or_grid(beat.max(0.0)), app.bar_or_grid());
                                if start {
                                    app.set_loop(at.min(r.end - step), r.end);
                                } else {
                                    app.set_loop(r.start, at.max(r.start + step));
                                }
                                app.status = app.loop_status();
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
            if resp.drag_stopped_by(egui::PointerButton::Primary) {
                if let Some(Drag::MoveClip {
                    clip_id,
                    copy_of: Some(original),
                    ..
                }) = app.drag.take()
                {
                    drop_copy(app, &clip_id, &original);
                }
                app.drag = None;
            }

            if resp.clicked() {
                if let Some(p) = resp.interact_pointer_pos() {
                    let target = hit(&app.project, p);
                    if !matches!(target, Hit::Ruler(_) | Hit::Loop { .. } | Hit::Nothing) {
                        app.edit_focus = EditFocus::Clips;
                    }
                    match target {
                        Hit::Loop {
                            part: LoopPart::Outside,
                            beat,
                        }
                        | Hit::Ruler(beat) => {
                            app.seek(beat);
                            let outside = app
                                .project
                                .active_loop()
                                .filter(|&(start, end)| beat < start || beat >= end);
                            if let (Some((start, _)), false) = (outside, app.is_playing()) {
                                let bar = (start / bpb).floor() as i64 + 1;
                                app.status = trf(
                                    "The loop is on, so Play starts at bar {} (L turns the loop off)",
                                    &[&bar],
                                );
                            }
                        }
                        Hit::Loop { .. } => app.toggle_loop(),
                        Hit::ClipBody {
                            clip_id, track_id, ..
                        }
                        | Hit::ClipEdge { clip_id, track_id } => {
                            select_clip(app, &clip_id, &track_id)
                        }
                        Hit::Header { track_id } => select_track(app, &track_id),
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

            // Right-click menus. What was clicked is kept while the menu is open.
            let menu_id = ui.id().with("arrange_menu");
            if resp.secondary_clicked() {
                if let Some(p) = resp.interact_pointer_pos() {
                    let menu = match hit(&app.project, p) {
                        Hit::ClipBody {
                            clip_id, track_id, ..
                        }
                        | Hit::ClipEdge { clip_id, track_id } => {
                            select_clip(app, &clip_id, &track_id);
                            app.edit_focus = EditFocus::Clips;
                            Menu::Clip {
                                clip_id,
                                beat: beat_of(p.x),
                            }
                        }
                        Hit::Header { track_id } => {
                            app.selected_track = Some(track_id.clone());
                            Menu::Track {
                                track_id,
                                beat: None,
                            }
                        }
                        Hit::Row { track_id, beat } => {
                            app.selected_track = Some(track_id.clone());
                            Menu::Track {
                                track_id,
                                beat: Some(beat),
                            }
                        }
                        Hit::Loop { beat, .. } | Hit::Ruler(beat) => Menu::Ruler { beat },
                        Hit::Nothing => Menu::Nothing,
                    };
                    ui.data_mut(|d| d.insert_temp(menu_id, menu));
                }
            }
            resp.context_menu(|ui| {
                let menu = ui
                    .data(|d| d.get_temp::<Menu>(menu_id))
                    .unwrap_or(Menu::Nothing);
                context_menu(ui, app, menu);
            });
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

/// Select a track and show its notes, not the previous track's clip.
fn select_track(app: &mut App, track_id: &str) {
    let keep = app
        .selected_clip
        .as_deref()
        .is_some_and(|id| app.project.clip(id).is_some_and(|(t, _)| t.id == track_id));
    let first = app
        .project
        .track(track_id)
        .and_then(|t| t.clips.first())
        .map(|c| c.id.clone());
    match first {
        _ if keep => app.selected_track = Some(track_id.to_string()),
        Some(clip_id) => select_clip(app, &clip_id, track_id),
        None => {
            app.selected_clip = None;
            app.selected_notes.clear();
            app.selected_track = Some(track_id.to_string());
        }
    }
}

/// The end of an Alt+drag: a copy dropped right on its original would
/// only double it, so it goes away again.
fn drop_copy(app: &mut App, copy: &str, original: &str) {
    let start = |id: &str| app.project.clip(id).map(|(_, c)| c.start);
    let (Some(a), Some(b)) = (start(copy), start(original)) else {
        return;
    };
    if (a - b).abs() < 1e-9 {
        if let Some(t) = app
            .project
            .tracks
            .iter_mut()
            .find(|t| t.clips.iter().any(|c| c.id == copy))
        {
            t.clips.retain(|c| c.id != copy);
        }
        if let Some((t, _)) = app.project.clip(original) {
            let track_id = t.id.clone();
            select_clip(app, original, &track_id);
        }
    } else {
        let bar = (a / app.bar()).floor() as i64 + 1;
        app.status = trf("Copied the clip to bar {}", &[&bar]);
    }
}

fn context_menu(ui: &mut egui::Ui, app: &mut App, menu: Menu) {
    match menu {
        Menu::Clip { clip_id, beat } => {
            if app.project.clip(&clip_id).is_none() {
                ui.close_menu();
                return;
            }
            if ui
                .add(icons::button(Icon::Scissors, tr("Cut in two here")))
                .on_hover_text(tr("S cuts the clip at the playhead"))
                .clicked()
            {
                app.split(Some(&clip_id), beat);
                ui.close_menu();
            }
            if ui
                .add(icons::button(Icon::Repeat, tr("Loop this clip")))
                .on_hover_text(tr(
                    "Play just this clip over and over (L turns the loop off)",
                ))
                .clicked()
            {
                app.loop_clip(&clip_id);
                ui.close_menu();
            }
            if ui.button(tr("Duplicate  (Ctrl+D)")).clicked() {
                app.duplicate_clip();
                ui.close_menu();
            }
            if ui
                .add(icons::button(Icon::Trash, tr("Delete  (Del)")))
                .clicked()
            {
                app.delete_selected_clip();
                ui.close_menu();
            }
        }
        Menu::Track { track_id, beat } => {
            let count = app.project.tracks.len();
            let Some(index) = app.project.tracks.iter().position(|t| t.id == track_id) else {
                ui.close_menu();
                return;
            };
            if let Some(beat) = beat {
                if ui
                    .add(icons::button(Icon::Plus, tr("New clip here")))
                    .clicked()
                {
                    let bar = app.bar();
                    app.add_clip(&track_id, ((beat / bar).floor() * bar).max(0.0));
                    ui.close_menu();
                }
                ui.separator();
            }
            ui.horizontal(|ui| {
                ui.label(tr("Colour"));
                let track = &mut app.project.tracks[index];
                if crate::inspector::color_swatches(ui, track) {
                    ui.close_menu();
                }
            });
            if ui
                .add_enabled(index > 0, icons::button(Icon::ArrowUp, tr("Move up")))
                .clicked()
            {
                app.move_track(&track_id, index - 1);
            }
            if ui
                .add_enabled(
                    index + 1 < count,
                    icons::button(Icon::ArrowDown, tr("Move down")),
                )
                .on_hover_text(tr("You can also drag a track's name up or down"))
                .clicked()
            {
                app.move_track(&track_id, index + 1);
            }
            ui.separator();
            if ui
                .add(icons::button(Icon::Trash, tr("Delete this track")))
                .clicked()
            {
                let name = app.project.tracks[index].name.clone();
                if app
                    .run_op(aidaw_core::edit::Op::RemoveTrack { track_id })
                    .is_some()
                {
                    app.status = trf("Deleted {}. Ctrl+Z (or ↩ Undo) brings it back.", &[&name]);
                }
                ui.close_menu();
            }
        }
        Menu::Ruler { beat } => {
            let has = app
                .project
                .transport
                .loop_region
                .is_some_and(|r| r.is_valid());
            let on = app.project.active_loop().is_some();
            let label = if on {
                tr("Turn the loop off  (L)")
            } else {
                tr("Turn the loop on  (L)")
            };
            if ui.add(icons::button(Icon::Repeat, label)).clicked() {
                app.toggle_loop();
                ui.close_menu();
            }
            let bar = app.bar();
            let start = (beat / bar).floor() * bar;
            let number = (start / bar) as i64 + 1;
            if ui.button(trf("Loop bar {}", &[&number])).clicked() {
                app.set_loop(start, start + bar);
                app.status = app.loop_status();
                ui.close_menu();
            }
            if has && ui.button(tr("Remove the loop")).clicked() {
                app.remove_loop();
                ui.close_menu();
            }
            ui.separator();
            if ui.button(tr("Move the playhead here")).clicked() {
                app.seek(beat);
                ui.close_menu();
            }
        }
        Menu::Nothing => ui.close_menu(),
    }
}
