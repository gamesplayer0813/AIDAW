//! Automation lanes in the song view: one setting of a track (or of the
//! whole song) drawn as a line that moves over time, right under its track.
//!
//! Click a lane to add a point, drag a point to move it (Shift: only up and
//! down, Alt: off the grid), right-click a point to delete it, and
//! right-click the line to choose how it moves on (straight, smooth, steps)
//! or to clear or remove the lane.

use aidaw_core::automation::{self, Lane, Param, Point, Shape};
use aidaw_core::{Instrument, Project};
use eframe::egui::{self, pos2, vec2, Align2, Color32, FontId, Pos2, Rect, Stroke};

use crate::app::App;
use crate::i18n::tr;
use crate::theme;

/// Height of a lane, of the row with the "Automate…" button under a track's
/// lanes, and of the Master row.
pub const LANE_H: f32 = 46.0;
pub const ADD_H: f32 = 26.0;
pub const MASTER_H: f32 = 30.0;
/// Room above and below the line inside a lane.
const PAD: f32 = 6.0;
/// How near (in points) the pointer has to be to grab a point.
pub const GRAB: f32 = 7.0;

/// One lane: a track's (by id), or with no track the whole song's.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LaneKey {
    pub track_id: Option<String>,
    pub param: Param,
}

impl LaneKey {
    pub fn new(track_id: Option<&str>, param: Param) -> Self {
        LaneKey {
            track_id: track_id.map(str::to_string),
            param,
        }
    }

    fn lanes<'a>(&self, p: &'a Project) -> Option<&'a [Lane]> {
        match &self.track_id {
            Some(id) => p.track(id).map(|t| t.automation.as_slice()),
            None => Some(&p.automation),
        }
    }

    fn lanes_mut<'a>(&self, p: &'a mut Project) -> Option<&'a mut Vec<Lane>> {
        match &self.track_id {
            Some(id) => p.track_mut(id).map(|t| &mut t.automation),
            None => Some(&mut p.automation),
        }
    }

    /// Which of `lanes` is this one: the one playback follows (the first of
    /// its setting with points), else the first of its setting.
    fn index(&self, lanes: &[Lane]) -> Option<usize> {
        lanes
            .iter()
            .position(|l| l.param == self.param && !l.points.is_empty())
            .or_else(|| lanes.iter().position(|l| l.param == self.param))
    }

    pub fn lane<'a>(&self, p: &'a Project) -> Option<&'a Lane> {
        let lanes = self.lanes(p)?;
        lanes.get(self.index(lanes)?)
    }

    pub fn lane_mut<'a>(&self, p: &'a mut Project) -> Option<&'a mut Lane> {
        let lanes = self.lanes_mut(p)?;
        let i = self.index(lanes)?;
        lanes.get_mut(i)
    }

    /// Whether this lane does anything (a synth setting on a drum track
    /// doesn't).
    pub fn usable(&self, p: &Project) -> bool {
        match &self.track_id {
            Some(id) => p
                .track(id)
                .is_some_and(|t| self.param.suits_track(&t.instrument)),
            None => self.param.is_master(),
        }
    }

    /// The value the setting has without automation (the slider's).
    pub fn fixed_value(&self, p: &Project) -> Option<f32> {
        let Some(id) = &self.track_id else {
            return Some(match self.param {
                Param::MasterVolume => p.master_gain,
                Param::ReverbSize => p.reverb.size,
                Param::ReverbTone => p.reverb.tone,
                _ => return None,
            });
        };
        let t = p.track(id)?;
        let synth = match &t.instrument {
            Instrument::Synth(s) => Some(s),
            Instrument::Drums { .. } | Instrument::Audio => None,
        };
        Some(match self.param {
            Param::Volume => t.gain,
            Param::Pan => t.pan,
            Param::Reverb => t.reverb,
            Param::Cutoff => synth?.cutoff,
            Param::Resonance => synth?.resonance,
            Param::Drive => synth?.drive,
            Param::Fm => synth?.fm,
            Param::Vibrato => synth?.vibrato,
            Param::Sub => synth?.sub,
            Param::Noise => synth?.noise,
            _ => return None,
        })
    }

    /// The value at `beat`: automated, or else the fixed one.
    pub fn value_at(&self, p: &Project, beat: f64) -> Option<f32> {
        self.lane(p)
            .and_then(|l| l.value_at(beat))
            .or_else(|| self.fixed_value(p))
    }
}

/// A right-click menu on a lane: which lane, and the point whose line the
/// click was on (`None` before the first point).
#[derive(Clone, Debug)]
pub struct LaneMenu {
    pub key: LaneKey,
    pub segment: Option<usize>,
}

/// One horizontal band of the song view, top to bottom.
#[derive(Clone, Debug)]
pub enum Band {
    Track(usize),
    /// A lane under track `track`, or under Master (`None`).
    Lane {
        track: Option<usize>,
        key: LaneKey,
    },
    /// The row with the "Automate…" button that adds a lane.
    AddLane {
        track: Option<usize>,
    },
    Master,
}

#[derive(Clone, Debug)]
pub struct Row {
    pub band: Band,
    /// Below the ruler.
    pub top: f32,
    pub h: f32,
}

/// The lanes of a track, or of the song.
fn lanes_of<'a>(p: &'a Project, track_id: Option<&str>) -> &'a [Lane] {
    match track_id {
        Some(id) => p.track(id).map_or(&[], |t| t.automation.as_slice()),
        None => &p.automation,
    }
}

/// The settings with a lane in `lanes`, once each, in file order.
pub fn shown_params(lanes: &[Lane]) -> Vec<Param> {
    let mut out: Vec<Param> = Vec::new();
    for l in lanes {
        if l.param != Param::Unknown && !out.contains(&l.param) {
            out.push(l.param);
        }
    }
    out
}

/// Settings that could get a new lane: right for the track (or the song)
/// and without a lane yet.
pub fn addable(p: &Project, track_id: Option<&str>) -> Vec<Param> {
    let have = shown_params(lanes_of(p, track_id));
    Param::ALL
        .into_iter()
        .filter(|param| !have.contains(param))
        .filter(|param| match track_id.and_then(|id| p.track(id)) {
            Some(t) => param.suits_track(&t.instrument),
            None => track_id.is_none() && param.is_master(),
        })
        .collect()
}

/// Whether the song has lanes of its own (master volume, reverb room).
pub fn song_has_lanes(p: &Project) -> bool {
    !shown_params(&p.automation).is_empty()
}

/// The rows of the song view: each track, the lanes of the open ones, then
/// Master with its lanes.
pub fn layout(app: &App, row_h: f32) -> Vec<Row> {
    let pro = app.pro();
    let p = &app.project;
    let lane_bands = |track: Option<usize>, track_id: Option<&str>| {
        let mut out: Vec<(Band, f32)> = shown_params(lanes_of(p, track_id))
            .into_iter()
            .map(|param| {
                let key = LaneKey::new(track_id, param);
                (Band::Lane { track, key }, LANE_H)
            })
            .collect();
        if pro && !addable(p, track_id).is_empty() {
            out.push((Band::AddLane { track }, ADD_H));
        }
        out
    };
    let mut rows = Vec::new();
    let mut y = 0.0;
    let mut push = |band: Band, h: f32| {
        rows.push(Row { band, top: y, h });
        y += h;
    };
    for (i, t) in p.tracks.iter().enumerate() {
        push(Band::Track(i), row_h);
        if app.lanes_open.contains(&t.id) {
            for (band, h) in lane_bands(Some(i), Some(&t.id)) {
                push(band, h);
            }
        }
    }
    if song_has_lanes(p) || (pro && !p.tracks.is_empty()) {
        push(Band::Master, MASTER_H);
        if app.master_lanes_open {
            for (band, h) in lane_bands(None, None) {
                push(band, h);
            }
        }
    }
    rows
}

/// A short help text for each setting, for the "Automate…" menu.
pub fn hint(param: Param) -> &'static str {
    match param {
        Param::Volume => tr("Fade in and out, or make parts louder and softer"),
        Param::Pan => tr("Move the sound between the left and right speaker"),
        Param::Reverb => tr("More or less echo as the song goes on"),
        Param::Cutoff => tr("The classic filter sweep: dark and muffled to bright and open"),
        Param::Resonance => tr("A whistling peak at the brightness point (acid, wah)"),
        Param::Drive => tr("Warmth at low values, distortion at high ones"),
        Param::Fm => tr("Bell / electric-piano tone (sine and triangle waves)"),
        Param::Vibrato => tr("Wobble in the pitch, in cents"),
        Param::Sub => tr("Add a sine one octave lower: more weight"),
        Param::Noise => tr("Mix in hiss: breath, air, grit"),
        Param::MasterVolume => tr("Volume of the whole song: fade it in or out"),
        Param::ReverbSize => tr("Size of the shared reverb: small room .. big hall"),
        Param::ReverbTone => tr("Dark, soft echoes .. bright, airy ones"),
        Param::Unknown => "",
    }
}

// ---- drawing -----------------------------------------------------------

/// Height of the line for `u` (0 = bottom, 1 = top) in a lane at `rect`.
pub fn y_of(rect: Rect, u: f32) -> f32 {
    rect.bottom() - PAD - u * (rect.height() - 2.0 * PAD)
}

/// The lane height (0..1) at `y`.
pub fn unit_at(rect: Rect, y: f32) -> f32 {
    ((rect.bottom() - PAD - y) / (rect.height() - 2.0 * PAD)).clamp(0.0, 1.0)
}

/// The value a click at height `y` means.
pub fn value_at_y(param: Param, rect: Rect, y: f32) -> f32 {
    let info = param.info();
    let v = info.from_unit(unit_at(rect, y));
    // Pan clicks near the middle land exactly in the middle.
    if param == Param::Pan && v.abs() < 0.04 {
        0.0
    } else {
        v
    }
}

/// The line through a lane's points, from `left` to `right` (screen x):
/// straight in the knob's own scale, eased for smooth points, steps for
/// held ones; flat before the first point and after the last.
fn polyline(
    lane: &Lane,
    rect: Rect,
    x_of: &dyn Fn(f64) -> f32,
    left: f32,
    right: f32,
) -> Vec<Pos2> {
    let info = lane.info();
    let y = |v: f32| y_of(rect, info.to_unit(v));
    let pts = &lane.points;
    let (Some(first), Some(last)) = (pts.first(), pts.last()) else {
        return Vec::new();
    };
    let mut out = vec![pos2(left.min(x_of(first.beat)), y(first.value))];
    for (i, a) in pts.iter().enumerate() {
        let xa = x_of(a.beat);
        out.push(pos2(xa, y(a.value)));
        let Some(b) = pts.get(i + 1) else {
            break;
        };
        let xb = x_of(b.beat);
        if xb < left || xa > right {
            continue;
        }
        match a.shape {
            Shape::Hold => out.push(pos2(xb, y(a.value))),
            Shape::Smooth if xb - xa > 2.0 => {
                let n = ((xb - xa) / 3.0).clamp(2.0, 64.0) as usize;
                let (ua, ub) = (info.to_unit(a.value), info.to_unit(b.value));
                for k in 1..n {
                    let t = k as f32 / n as f32;
                    let u = ua + (ub - ua) * Shape::Smooth.ease(t);
                    out.push(pos2(xa + (xb - xa) * t, y_of(rect, u)));
                }
            }
            _ => {}
        }
    }
    out.push(pos2(right.max(x_of(last.beat)), y(last.value)));
    // Only what is on screen (the line only ever goes right).
    let from = out.partition_point(|p| p.x < left).saturating_sub(1);
    let to = (out.partition_point(|p| p.x <= right) + 1).min(out.len());
    out[from..to].to_vec()
}

/// A soft fill between the line and the bottom of the lane.
fn fill_under(line: &[Pos2], bottom: f32, color: Color32) -> egui::Shape {
    let mut mesh = egui::Mesh::default();
    for w in line.windows(2) {
        let (a, b) = (w[0], w[1]);
        if b.x - a.x < 0.01 {
            continue;
        }
        let i = mesh.vertices.len() as u32;
        mesh.colored_vertex(a, color);
        mesh.colored_vertex(b, color);
        mesh.colored_vertex(pos2(b.x, bottom), color);
        mesh.colored_vertex(pos2(a.x, bottom), color);
        mesh.add_triangle(i, i + 1, i + 2);
        mesh.add_triangle(i, i + 2, i + 3);
    }
    egui::Shape::mesh(mesh)
}

/// What the pointer is doing to a lane while it is being painted.
#[derive(Clone, Copy, Default)]
pub struct Marks {
    pub hovered: Option<usize>,
    pub selected: Option<usize>,
    /// Show the value of this point next to it (it is being dragged).
    pub dragging: Option<usize>,
}

/// Paint the timeline part of a lane (`rect` starts where beat 0 is drawn).
#[allow(clippy::too_many_arguments)]
pub fn paint_lane(
    painter: &egui::Painter,
    project: &Project,
    key: &LaneKey,
    rect: Rect,
    x_of: &dyn Fn(f64) -> f32,
    color: Color32,
    playhead: f64,
    marks: Marks,
) {
    let visible = painter.clip_rect().intersect(rect);
    if visible.width() <= 0.0 || visible.height() <= 0.0 {
        return;
    }
    let usable = key.usable(project);
    let color = if usable {
        color
    } else {
        theme::mix(theme::CANVAS, theme::TEXT_FAINT, 0.6)
    };
    let info = key.param.info();
    // Guides at the bottom, the middle and the top.
    for u in [0.0, 0.5, 1.0] {
        let y = y_of(rect, u);
        painter.line_segment(
            [pos2(visible.left(), y), pos2(visible.right(), y)],
            Stroke::new(
                1.0_f32,
                Color32::from_white_alpha(if u == 0.5 { 6 } else { 4 }),
            ),
        );
    }
    let lane = key.lane(project).filter(|l| !l.points.is_empty());
    let Some(lane) = lane else {
        // No points yet: where the setting sits now, and how to start.
        if let Some(v) = key.fixed_value(project) {
            let y = y_of(rect, info.to_unit(v));
            painter.extend(egui::Shape::dashed_line(
                &[pos2(visible.left(), y), pos2(visible.right(), y)],
                Stroke::new(1.0_f32, theme::alpha(color, 110)),
                6.0,
                5.0,
            ));
        }
        painter.text(
            pos2(rect.left() + 10.0, rect.center().y),
            Align2::LEFT_CENTER,
            tr("Click to add points; the line moves this setting over time"),
            FontId::proportional(11.5),
            theme::TEXT_FAINT,
        );
        return;
    };
    let line = polyline(
        lane,
        rect,
        x_of,
        visible.left() - 2.0,
        visible.right() + 2.0,
    );
    painter.add(fill_under(
        &line,
        rect.bottom() - PAD,
        theme::alpha(color, 26),
    ));
    painter.add(egui::Shape::line(line, Stroke::new(1.6_f32, color)));

    for (i, p) in lane.points.iter().enumerate() {
        let x = x_of(p.beat);
        if x < visible.left() - GRAB || x > visible.right() + GRAB {
            continue;
        }
        let c = pos2(x, y_of(rect, info.to_unit(p.value)));
        let big = marks.hovered == Some(i) || marks.dragging == Some(i);
        painter.circle(
            c,
            if big { 5.0 } else { 3.5 },
            color,
            Stroke::new(1.5_f32, theme::CANVAS),
        );
        if marks.selected == Some(i) {
            painter.circle_stroke(c, 7.0, Stroke::new(1.5_f32, Color32::WHITE));
        }
        if marks.dragging == Some(i) || marks.hovered == Some(i) {
            value_tag(painter, c, &info.format(info.clamp(p.value)), rect);
        }
    }
    // Where the playhead crosses the line.
    if let Some(v) = lane.value_at(playhead) {
        let x = x_of(playhead);
        if x >= visible.left() && x <= visible.right() {
            let c = pos2(x, y_of(rect, info.to_unit(v)));
            painter.circle_filled(c, 3.0, theme::PLAYHEAD);
        }
    }
}

/// A small label with a value, next to a point and inside the lane.
fn value_tag(painter: &egui::Painter, at: Pos2, text: &str, lane: Rect) {
    let font = FontId::proportional(11.0);
    let galley = painter.layout_no_wrap(text.to_string(), font, theme::TEXT);
    let size = galley.size() + vec2(8.0, 2.0);
    let mut min = at + vec2(8.0, -size.y - 2.0);
    if min.y < lane.top() {
        min.y = at.y + 4.0;
    }
    let r = Rect::from_min_size(min, size);
    painter.rect_filled(r, 4.0, theme::alpha(theme::SURFACE_HI, 235));
    painter.galley(r.min + vec2(4.0, 1.0), galley, theme::TEXT);
}

/// Paint a lane's header: its name and its value at the playhead. The
/// remove button is a real button the song view puts on top.
pub fn paint_header(
    painter: &egui::Painter,
    project: &Project,
    key: &LaneKey,
    rect: Rect,
    color: Color32,
    playhead: f64,
) {
    let card = Rect::from_min_max(rect.min + vec2(18.0, 3.0), rect.max - vec2(6.0, 3.0));
    painter.rect_filled(card, 6.0, theme::alpha(theme::SURFACE, 200));
    painter.rect_filled(
        Rect::from_min_size(card.min + vec2(5.0, 7.0), vec2(2.0, card.height() - 14.0)),
        1.0,
        theme::alpha(color, 200),
    );
    let info = key.param.info();
    let text = painter.with_clip_rect(card.shrink(2.0).intersect(painter.clip_rect()));
    text.text(
        card.left_top() + vec2(13.0, 6.0),
        Align2::LEFT_TOP,
        tr(info.label),
        theme::bold_font(11.5),
        theme::TEXT,
    );
    let (detail, tint) = if !key.usable(project) {
        let audio = key
            .track_id
            .as_deref()
            .and_then(|id| project.track(id))
            .is_some_and(|t| t.instrument.is_audio());
        let why = if audio {
            tr("not used by audio tracks")
        } else {
            tr("not used by drums")
        };
        (why.to_string(), theme::WARN)
    } else {
        match key.value_at(project, playhead) {
            Some(v) => (info.format(v), theme::TEXT_DIM),
            None => (String::new(), theme::TEXT_DIM),
        }
    };
    text.text(
        card.left_top() + vec2(13.0, 22.0),
        Align2::LEFT_TOP,
        detail,
        FontId::proportional(10.5),
        tint,
    );
}

/// Find the point of `lane` near `pos` (closest first).
pub fn point_near(lane: &Lane, rect: Rect, x_of: &dyn Fn(f64) -> f32, pos: Pos2) -> Option<usize> {
    let info = lane.info();
    lane.points
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let c = pos2(x_of(p.beat), y_of(rect, info.to_unit(p.value)));
            (i, c.distance(pos))
        })
        .filter(|(_, d)| *d <= GRAB)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i)
}

// ---- editing -----------------------------------------------------------

/// Add a lane (empty) for `key` if it has none.
pub fn add_lane(app: &mut App, key: &LaneKey) {
    if let Some(lanes) = key.lanes_mut(&mut app.project) {
        if !lanes.iter().any(|l| l.param == key.param) {
            lanes.push(Lane::new(key.param));
        }
    }
}

/// Add a point; returns its index. A point added on a smooth or stepped
/// line takes that shape, so the line keeps its character.
pub fn add_point(app: &mut App, key: &LaneKey, beat: f64, value: f32) -> Option<usize> {
    add_lane(app, key);
    let lane = key.lane_mut(&mut app.project)?;
    if lane.points.len() >= automation::MAX_POINTS {
        return None;
    }
    let info = lane.info();
    let i = lane.insert(Point::new(beat.max(0.0), info.clamp(value)));
    if i > 0 {
        lane.points[i].shape = lane.points[i - 1].shape;
    }
    Some(i)
}

/// Move point `index` to `beat` and `value`, staying between its
/// neighbours (it may share their beat: that makes a jump).
pub fn move_point(app: &mut App, key: &LaneKey, index: usize, beat: f64, value: f32) {
    let Some(lane) = key.lane_mut(&mut app.project) else {
        return;
    };
    if index >= lane.points.len() {
        return;
    }
    let lo = index.checked_sub(1).map_or(0.0, |i| lane.points[i].beat);
    let hi = lane.points.get(index + 1).map_or(f64::INFINITY, |p| p.beat);
    let info = lane.info();
    let p = &mut lane.points[index];
    p.beat = beat.clamp(lo, hi.max(lo));
    p.value = info.clamp(value);
}

pub fn delete_point(app: &mut App, key: &LaneKey, index: usize) {
    if let Some(lane) = key.lane_mut(&mut app.project) {
        if index < lane.points.len() {
            lane.points.remove(index);
        }
    }
    if app.selected_point.as_ref().is_some_and(|(k, _)| k == key) {
        app.selected_point = None;
    }
}

pub fn set_shape(app: &mut App, key: &LaneKey, index: usize, shape: Shape) {
    if let Some(p) = key
        .lane_mut(&mut app.project)
        .and_then(|l| l.points.get_mut(index))
    {
        p.shape = shape;
    }
}

/// Take every point off the lane (it stays, empty).
pub fn clear_lane(app: &mut App, key: &LaneKey) {
    if let Some(lane) = key.lane_mut(&mut app.project) {
        lane.points.clear();
    }
    if app.selected_point.as_ref().is_some_and(|(k, _)| k == key) {
        app.selected_point = None;
    }
}

/// Remove the lane (all of the setting's lanes) from the track or song.
pub fn remove_lane(app: &mut App, key: &LaneKey) {
    if let Some(lanes) = key.lanes_mut(&mut app.project) {
        lanes.retain(|l| l.param != key.param);
    }
    if app.selected_point.as_ref().is_some_and(|(k, _)| k == key) {
        app.selected_point = None;
    }
}

/// The right-click menu of a lane.
pub fn menu(ui: &mut egui::Ui, app: &mut App, menu: &LaneMenu) {
    let key = &menu.key;
    let Some(lane) = key.lane(&app.project) else {
        ui.close_menu();
        return;
    };
    ui.label(egui::RichText::new(tr(lane.info().label)).strong());
    let segment = menu.segment.filter(|&i| i + 1 < lane.points.len());
    if let Some(i) = segment {
        let now = lane.points[i].shape;
        ui.label(egui::RichText::new(tr("From this point to the next:")).weak());
        for (shape, label) in [
            (Shape::Linear, tr("Straight line")),
            (Shape::Smooth, tr("Smooth curve")),
            (Shape::Hold, tr("Steps: stay, then jump")),
        ] {
            if ui.radio(now == shape, label).clicked() {
                set_shape(app, key, i, shape);
                ui.close_menu();
            }
        }
        ui.separator();
    }
    if ui
        .button(tr("Clear the points"))
        .on_hover_text(tr("The setting goes back to its slider value"))
        .clicked()
    {
        clear_lane(app, key);
        ui.close_menu();
    }
    if ui.button(tr("Remove this lane")).clicked() {
        remove_lane(app, key);
        ui.close_menu();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lane(points: &[(f64, f32, Shape)]) -> Lane {
        Lane {
            param: Param::Pan,
            points: points
                .iter()
                .map(|&(beat, value, shape)| Point { beat, value, shape })
                .collect(),
        }
    }

    #[test]
    fn the_line_is_straight_smooth_or_stepped_and_flat_outside() {
        let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(1000.0, 46.0));
        let x_of = |b: f64| b as f32 * 10.0;
        let l = lane(&[
            (2.0, -1.0, Shape::Hold),
            (4.0, 1.0, Shape::Linear),
            (6.0, -1.0, Shape::Smooth),
            (8.0, 1.0, Shape::Linear),
        ]);
        let line = polyline(&l, rect, &x_of, 0.0, 200.0);
        // Flat from the left edge to the first point.
        assert_eq!(line[0], pos2(0.0, y_of(rect, 0.0)));
        // Held: along at -1, then straight up at the next point.
        assert!(line.contains(&pos2(40.0, y_of(rect, 0.0))));
        assert!(line.contains(&pos2(40.0, y_of(rect, 1.0))));
        // Smooth: points in between, leaving the start more slowly than a
        // straight line would.
        let between: Vec<&Pos2> = line.iter().filter(|p| p.x > 60.0 && p.x < 80.0).collect();
        assert!(between.len() > 3);
        let straight = (between[0].x - 60.0) / 20.0;
        assert!(between[0].y > y_of(rect, straight), "{between:?}");
        // Flat to the right edge after the last point, and only ever rightwards.
        assert_eq!(*line.last().unwrap(), pos2(200.0, y_of(rect, 1.0)));
        assert!(line.windows(2).all(|w| w[1].x >= w[0].x));
        // Off-screen parts are left out.
        let right_half = polyline(&l, rect, &x_of, 70.0, 200.0);
        assert!(right_half.len() < line.len());
        assert!(right_half[0].x <= 70.0 && right_half[1].x >= 60.0);
    }

    #[test]
    fn heights_and_values_match_the_knob() {
        let rect = Rect::from_min_size(pos2(0.0, 100.0), vec2(500.0, 46.0));
        // The top of a brightness lane is 20 kHz, the middle about 632 Hz.
        assert!((value_at_y(Param::Cutoff, rect, y_of(rect, 1.0)) - 20_000.0).abs() < 1.0);
        let mid = value_at_y(Param::Cutoff, rect, y_of(rect, 0.5));
        assert!((mid - 632.5).abs() < 2.0, "{mid}");
        // Pan clicks near the middle snap to it.
        assert_eq!(value_at_y(Param::Pan, rect, y_of(rect, 0.51)), 0.0);
        assert!((unit_at(rect, rect.top() - 10.0) - 1.0).abs() < 1e-6);
    }
}
