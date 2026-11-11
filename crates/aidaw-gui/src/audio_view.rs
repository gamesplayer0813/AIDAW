//! The editor for an audio clip (a recording), shown where the piano roll
//! is: the whole sound file as a waveform, with the part the clip plays lit
//! up. Drag the lit part's edges to trim it, the small handles on its top
//! corners to fade in and out, anywhere else to move the playhead. The
//! toolbar sets the clip's volume (and, in Pro mode, exact numbers).

use aidaw_core::{AudioRef, Clip};
use eframe::egui::{
    self, pos2, vec2, Align2, Color32, FontId, Rect, RichText, Sense, Stroke, StrokeKind,
};

use crate::app::{track_color, App, EditFocus};
use crate::i18n::{tr, trf};
use crate::icons::{self, Icon};
use crate::theme;

const RULER_H: f32 = 20.0;
const SIDE: f32 = 10.0;
/// How close (in points) the pointer must be to grab an edge or a handle.
const GRAB: f32 = 7.0;
const HANDLE: f32 = 9.0;
/// Shortest clip trimming leaves, in seconds.
const MIN_SECONDS: f64 = 0.05;

#[derive(Clone, Copy, PartialEq, Debug)]
enum Grab {
    TrimStart,
    TrimEnd,
    FadeIn,
    FadeOut,
    Seek,
}

pub fn show(ui: &mut egui::Ui, app: &mut App, clip_id: &str, track_index: usize) {
    let Some(clip) = app.project.clip(clip_id).map(|(_, c)| c.clone()) else {
        return;
    };
    let Some(a) = clip.audio.clone() else {
        return;
    };
    toolbar(ui, app, clip_id, &clip, &a);
    ui.separator();

    let spb = app.project.seconds_per_beat();
    let data = app.audio.get(&a.file).cloned();
    let color = track_color(&app.project.tracks[track_index]);
    let start = a.offset;
    let end = a.offset + clip.length * spb;
    let total = data
        .as_ref()
        .map_or(end, |d| d.seconds())
        .max(end)
        .max(0.001);

    let (resp, painter) = ui.allocate_painter(ui.available_size(), Sense::click_and_drag());
    let rect = resp.rect;
    app.ui_rects.insert("piano", rect);
    let wave = Rect::from_min_max(
        pos2(rect.left() + SIDE, rect.top() + RULER_H + 6.0),
        pos2(rect.right() - SIDE, rect.bottom() - 8.0),
    );
    let x_of = |t: f64| wave.left() + (t / total) as f32 * wave.width();
    let t_of = |x: f32| ((x - wave.left()) / wave.width().max(1.0)) as f64 * total;

    // Background, ruler with seconds.
    painter.rect_filled(rect, 0.0, theme::CANVAS);
    let ruler = Rect::from_min_size(rect.min, vec2(rect.width(), RULER_H));
    painter.rect_filled(ruler, 0.0, theme::PANEL);
    let step = [0.1, 0.25, 0.5, 1.0, 2.0, 5.0, 10.0, 15.0, 30.0, 60.0, 120.0]
        .into_iter()
        .find(|s| (*s / total) as f32 * wave.width() >= 56.0)
        .unwrap_or(300.0);
    let mut t = 0.0;
    while t <= total + 1e-9 {
        let x = x_of(t);
        painter.line_segment(
            [pos2(x, ruler.bottom() - 6.0), pos2(x, ruler.bottom())],
            Stroke::new(1.0_f32, theme::BORDER_HI),
        );
        painter.line_segment(
            [pos2(x, wave.top()), pos2(x, wave.bottom())],
            Stroke::new(1.0_f32, Color32::from_white_alpha(6)),
        );
        painter.text(
            pos2(x + 3.0, ruler.top() + 3.0),
            Align2::LEFT_TOP,
            seconds_label(t, step),
            FontId::proportional(10.5),
            theme::TEXT_DIM,
        );
        t += step;
    }

    // The part the clip plays.
    let lit = Rect::from_min_max(
        pos2(x_of(start), wave.top()),
        pos2(x_of(end), wave.bottom()),
    );
    painter.rect_filled(lit, 4.0, theme::mix(theme::CANVAS, color, 0.12));
    let mid = wave.center().y;
    painter.line_segment(
        [pos2(wave.left(), mid), pos2(wave.right(), mid)],
        Stroke::new(1.0_f32, Color32::from_white_alpha(10)),
    );

    // Waveform: one line per column, at the clip's volume and fades.
    match &data {
        Some(d) => {
            let visible = painter.clip_rect().intersect(wave);
            let ink = theme::mix(color, Color32::WHITE, 0.25);
            let dim = theme::alpha(theme::TEXT_FAINT, 150);
            let mut x = visible.left().floor();
            while x < visible.right() {
                let (t0, t1) = (t_of(x), t_of(x + 1.0));
                let inside = t1 > start && t0 < end;
                let env = if inside {
                    envelope(&a, start, end, (t0 + t1) / 2.0)
                } else {
                    1.0
                };
                let level = (d.peak_between(t0, t1) * a.gain * env).min(1.0);
                let h = (level * wave.height() / 2.0).max(0.5);
                let clipped = d.peak_between(t0, t1) * a.gain >= 1.0;
                let c = if !inside {
                    dim
                } else if clipped {
                    theme::DANGER
                } else {
                    ink
                };
                painter.line_segment(
                    [pos2(x + 0.5, mid - h), pos2(x + 0.5, mid + h)],
                    Stroke::new(1.0_f32, c),
                );
                x += 1.0;
            }
        }
        None => {
            let failed = app.audio_failed.contains(&a.file);
            let (text, c) = if failed {
                (
                    trf(
                        "Can't find {}. Put the file back next to the song, or delete this clip.",
                        &[&a.file],
                    ),
                    theme::DANGER,
                )
            } else {
                (tr("Loading the recording…").to_string(), theme::TEXT_DIM)
            };
            painter.text(
                wave.center(),
                Align2::CENTER_CENTER,
                text,
                FontId::proportional(14.0),
                c,
            );
        }
    }

    // Fades: a shaded corner and a line, with a handle to drag.
    let fade_in_x = x_of(start + a.fade_in);
    let fade_out_x = x_of(end - a.fade_out);
    let shade = theme::alpha(Color32::BLACK, 90);
    for (x0, x1, rising) in [
        (lit.left(), fade_in_x, true),
        (fade_out_x, lit.right(), false),
    ] {
        if x1 - x0 < 1.0 {
            continue;
        }
        let (low, high) = if rising { (x0, x1) } else { (x1, x0) };
        painter.add(egui::Shape::convex_polygon(
            vec![
                pos2(low, lit.top()),
                pos2(high, lit.top()),
                pos2(low, lit.bottom()),
            ],
            shade,
            Stroke::NONE,
        ));
        painter.line_segment(
            [pos2(low, lit.bottom()), pos2(high, lit.top())],
            Stroke::new(1.5_f32, theme::WARN),
        );
    }
    let handles = [
        (Grab::FadeIn, pos2(fade_in_x, lit.top())),
        (Grab::FadeOut, pos2(fade_out_x, lit.top())),
    ];
    for (_, p) in handles {
        let r = Rect::from_center_size(p + vec2(0.0, HANDLE / 2.0), vec2(HANDLE, HANDLE));
        painter.rect_filled(r, 2.0, theme::WARN);
        painter.rect_stroke(
            r,
            2.0,
            Stroke::new(1.0_f32, theme::CANVAS),
            StrokeKind::Inside,
        );
    }
    // Trim edges.
    for x in [lit.left(), lit.right()] {
        painter.line_segment(
            [pos2(x, wave.top()), pos2(x, wave.bottom())],
            Stroke::new(2.0_f32, color),
        );
    }
    painter.rect_stroke(
        lit,
        4.0,
        Stroke::new(1.0_f32, theme::alpha(color, 160)),
        StrokeKind::Inside,
    );

    // Playhead, when it is inside the clip.
    let rel = app.playhead - clip.start;
    if (0.0..=clip.length).contains(&rel) {
        let x = x_of(start + rel * spb);
        painter.line_segment(
            [pos2(x, ruler.top()), pos2(x, wave.bottom())],
            Stroke::new(1.5_f32, theme::PLAYHEAD),
        );
    }

    // ---- interaction ----
    let grab_at = |p: egui::Pos2| -> Grab {
        let near = |x: f32| (p.x - x).abs() <= GRAB;
        let top = p.y <= lit.top() + HANDLE + GRAB;
        if top && near(fade_in_x) {
            Grab::FadeIn
        } else if top && near(fade_out_x) {
            Grab::FadeOut
        } else if near(lit.left()) {
            Grab::TrimStart
        } else if near(lit.right()) {
            Grab::TrimEnd
        } else {
            Grab::Seek
        }
    };
    if let Some(p) = resp.hover_pos() {
        if !matches!(grab_at(p), Grab::Seek) {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        }
    }
    let grab_id = resp.id.with("grab");
    if resp.drag_started() || resp.clicked() {
        app.edit_focus = EditFocus::Clips;
    }
    if resp.drag_started() {
        if let Some(p) = ui.input(|i| i.pointer.press_origin()) {
            ui.data_mut(|d| d.insert_temp(grab_id, grab_at(p)));
        }
    }
    let pointer = resp.interact_pointer_pos();
    let grab = if resp.dragged() {
        ui.data(|d| d.get_temp::<Grab>(grab_id))
    } else if resp.clicked() {
        Some(Grab::Seek)
    } else {
        None
    };
    if resp.drag_stopped() {
        ui.data_mut(|d| d.remove::<Grab>(grab_id));
    }
    let (Some(grab), Some(p)) = (grab, pointer) else {
        return;
    };
    let t = t_of(p.x).clamp(0.0, total);
    let free = ui.input(|i| i.modifiers.shift);
    let grid = app.grid();
    let snap = |beat: f64| {
        if free {
            beat
        } else {
            (beat / grid).round() * grid
        }
    };
    let mut seek = None;
    let Some(c) = app.project.clip_mut(clip_id) else {
        return;
    };
    let Some(a) = c.audio.as_mut() else {
        return;
    };
    match grab {
        Grab::TrimStart => {
            // The end stays where it is on the timeline; the start moves.
            let earliest = (start - c.start * spb).max(0.0);
            let t = t.clamp(earliest, end - MIN_SECONDS);
            let beat = snap(c.start + (t - start) / spb).max(0.0);
            let t = (start + (beat - c.start) * spb).clamp(earliest, end - MIN_SECONDS);
            c.start = (c.start + (t - start) / spb).max(0.0);
            c.length = (end - t) / spb;
            a.offset = t;
        }
        Grab::TrimEnd => {
            let t = t.max(start + MIN_SECONDS);
            let beat = snap(c.start + (t - start) / spb);
            c.length = (beat - c.start).max(MIN_SECONDS / spb);
        }
        Grab::FadeIn => a.fade_in = (t - start).clamp(0.0, (end - start - a.fade_out).max(0.0)),
        Grab::FadeOut => a.fade_out = (end - t).clamp(0.0, (end - start - a.fade_in).max(0.0)),
        Grab::Seek => {
            if (start..=end).contains(&t) {
                seek = Some(c.start + (t - start) / spb);
            }
        }
    }
    let region = c.length * spb;
    a.fade_in = a.fade_in.min(region);
    a.fade_out = a.fade_out.min(region - a.fade_in);
    if let Some(beat) = seek {
        app.seek(beat);
    }
}

/// The clip's volume at file time `t` from its fades (0..1).
fn envelope(a: &AudioRef, start: f64, end: f64, t: f64) -> f32 {
    let mut g = 1.0f64;
    if a.fade_in > 0.0 {
        g = g.min((t - start) / a.fade_in);
    }
    if a.fade_out > 0.0 {
        g = g.min((end - t) / a.fade_out);
    }
    g.clamp(0.0, 1.0) as f32
}

fn seconds_label(t: f64, step: f64) -> String {
    if t >= 60.0 {
        format!("{}:{:02}", (t / 60.0) as u64, (t % 60.0) as u64)
    } else if step < 1.0 {
        format!("{t:.2}s")
    } else {
        format!("{t:.0}s")
    }
}

/// A gain as decibels: "0.0 dB", "-6.0 dB", "-∞ dB".
fn db(gain: f64) -> String {
    if gain <= 1e-5 {
        "-∞ dB".into()
    } else {
        format!("{:+.1} dB", 20.0 * gain.log10())
    }
}

fn toolbar(ui: &mut egui::Ui, app: &mut App, clip_id: &str, clip: &Clip, a: &AudioRef) {
    let pro = app.pro();
    let spb = app.project.seconds_per_beat();
    let mut audio = a.clone();
    ui.horizontal_wrapped(|ui| {
        icons::label(ui, Icon::Wave);
        let name = if clip.name.is_empty() { &clip.id } else { &clip.name };
        ui.label(RichText::new(name).font(theme::bold_font(14.0)))
            .on_hover_text(&a.file);
        let seconds = clip.length * spb;
        ui.label(
            RichText::new(trf("{}s of recording", &[&format!("{seconds:.1}")]))
                .size(12.0)
                .color(theme::TEXT_DIM),
        );
        ui.add_space(8.0);
        icons::label(ui, Icon::Volume).on_hover_text(tr("Volume"));
        ui.spacing_mut().slider_width = 120.0;
        let mut gain = audio.gain as f64;
        if ui
            .add(
                egui::Slider::new(&mut gain, 0.0..=aidaw_core::edit::MAX_CLIP_GAIN as f64)
                    .logarithmic(true)
                    .smallest_positive(0.01)
                    .custom_formatter(|v, _| db(v)),
            )
            .on_hover_text(tr("How loud this recording plays (0 dB = as recorded)"))
            .changed()
        {
            audio.gain = gain as f32;
        }
        let most = (seconds / 2.0).max(0.0);
        ui.label(tr("Fade in")).on_hover_text(tr(
            "Seconds the recording takes to fade in (or drag the yellow handle)",
        ));
        ui.add(
            egui::DragValue::new(&mut audio.fade_in)
                .range(0.0..=most)
                .speed(0.01)
                .max_decimals(2)
                .suffix(" s"),
        );
        ui.label(tr("Fade out")).on_hover_text(tr(
            "Seconds the recording takes to fade out at the end of the clip",
        ));
        ui.add(
            egui::DragValue::new(&mut audio.fade_out)
                .range(0.0..=most)
                .speed(0.01)
                .max_decimals(2)
                .suffix(" s"),
        );
        if pro {
            ui.label(tr("Starts at")).on_hover_text(tr(
                "Where in the file the clip begins, in seconds (drag the left edge to trim)",
            ));
            ui.add(
                egui::DragValue::new(&mut audio.offset)
                    .range(0.0..=f64::MAX)
                    .speed(0.005)
                    .max_decimals(3)
                    .suffix(" s"),
            );
        }
        theme::hint(
            ui,
            tr("drag the edges to trim · yellow handles fade · click to move the playhead · Shift: off the grid"),
        );
    });
    if audio != *a {
        if let Some(c) = app.project.clip_mut(clip_id) {
            c.audio = Some(audio);
        }
        app.edit_focus = EditFocus::Clips;
    }
}
