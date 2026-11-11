//! Automation: settings that move over time. A lane holds points (a beat
//! and a value) for one setting of a track, or of the whole song (master
//! volume, reverb room). Playback glides from point to point; before the
//! first point and after the last one the value stays put. While a lane has
//! points, the setting's fixed value (a track's `gain`, a synth's `cutoff`,
//! ...) is not used.
//!
//! Values are stored in the setting's own units (gain, -1..1 pan, Hz), so
//! the file reads like the rest of the project. In between points they move
//! the way a knob feels: the cutoff in octaves, the volume along a fader's
//! curve, so sweeps and fades sound even.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Most points one lane may hold.
pub const MAX_POINTS: usize = 4096;

/// What an automation lane changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Param {
    /// A track's volume (its `gain`).
    Volume,
    /// A track's pan.
    Pan,
    /// A track's send to the shared reverb.
    Reverb,
    /// Synth tracks: the low-pass cutoff.
    Cutoff,
    Resonance,
    Drive,
    Fm,
    Vibrato,
    Sub,
    Noise,
    /// The whole song's volume.
    MasterVolume,
    /// The shared reverb room's size and tone.
    ReverbSize,
    ReverbTone,
    /// A setting this version does not know, such as a typo in a file
    /// edited by hand. Playback ignores it, edits refuse it, and it is
    /// saved back as "unknown". Adding a setting therefore bumps
    /// [`crate::model::FORMAT_VERSION`], so older builds refuse the new
    /// files instead of losing the setting.
    Unknown,
}

/// What a parameter belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// Any track.
    Track,
    /// Tracks that play the built-in synth (not drum kits or plugins).
    Synth,
    /// The song as a whole.
    Master,
}

/// How a lane's height maps to the value, and so how values move between
/// two points.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scale {
    Linear,
    /// Even steps in octaves (frequencies).
    Log,
    /// A volume fader's taper: most of the travel near full volume.
    Fader,
}

/// Everything the interface and the AI tools need to know about a parameter.
#[derive(Debug)]
pub struct Info {
    pub param: Param,
    /// The name in the project file and the AI tools, e.g. "cutoff".
    pub id: &'static str,
    /// Short English name for the interface (translated there).
    pub label: &'static str,
    pub min: f32,
    pub max: f32,
    pub scale: Scale,
    pub scope: Scope,
    /// One line for the AI: units and typical values.
    pub hint: &'static str,
}

const fn info(
    param: Param,
    id: &'static str,
    label: &'static str,
    (min, max): (f32, f32),
    scale: Scale,
    scope: Scope,
    hint: &'static str,
) -> Info {
    Info {
        param,
        id,
        label,
        min,
        max,
        scale,
        scope,
        hint,
    }
}

static INFO: [Info; 14] = [
    info(
        Param::Volume,
        "volume",
        "Volume",
        (0.0, 2.0),
        Scale::Fader,
        Scope::Track,
        "track volume 0-2 like update_track's gain (0 silent, 0.8 default, 1 unity)",
    ),
    info(
        Param::Pan,
        "pan",
        "Pan",
        (-1.0, 1.0),
        Scale::Linear,
        Scope::Track,
        "-1 left .. 0 centre .. 1 right",
    ),
    info(
        Param::Reverb,
        "reverb",
        "Reverb",
        (0.0, 1.0),
        Scale::Linear,
        Scope::Track,
        "send to the shared reverb, 0 dry .. 1 very wet",
    ),
    info(
        Param::Cutoff,
        "cutoff",
        "Brightness",
        (20.0, 20_000.0),
        Scale::Log,
        Scope::Synth,
        "synth low-pass cutoff in Hz, 200 dark .. 2000 warm .. 12000 bright (the classic filter sweep)",
    ),
    info(
        Param::Resonance,
        "resonance",
        "Resonance",
        (0.0, 1.0),
        Scale::Linear,
        Scope::Synth,
        "synth filter resonance 0-1 (whistling peak; acid, wah)",
    ),
    info(
        Param::Drive,
        "drive",
        "Drive",
        (0.0, 1.0),
        Scale::Linear,
        Scope::Synth,
        "synth saturation 0-1, warmth to distortion",
    ),
    info(
        Param::Fm,
        "fm",
        "FM",
        (0.0, 10.0),
        Scale::Linear,
        Scope::Synth,
        "FM amount 0-10 for sine/triangle synths: more = brighter, more bell-like",
    ),
    info(
        Param::Vibrato,
        "vibrato",
        "Vibrato",
        (0.0, 100.0),
        Scale::Linear,
        Scope::Synth,
        "pitch wobble depth in cents, e.g. 0 -> 25 for a vibrato that grows",
    ),
    info(
        Param::Sub,
        "sub",
        "Sub",
        (0.0, 1.0),
        Scale::Linear,
        Scope::Synth,
        "level 0-1 of a sine one octave below",
    ),
    info(
        Param::Noise,
        "noise",
        "Noise",
        (0.0, 1.0),
        Scale::Linear,
        Scope::Synth,
        "level 0-1 of noise mixed in (breath, air, risers)",
    ),
    info(
        Param::MasterVolume,
        "master_volume",
        "Master volume",
        (0.0, 2.0),
        Scale::Fader,
        Scope::Master,
        "the whole song's volume 0-2 like update_project's master_gain (fade the song in or out)",
    ),
    info(
        Param::ReverbSize,
        "reverb_size",
        "Reverb room",
        (0.0, 1.0),
        Scale::Linear,
        Scope::Master,
        "shared reverb room 0 small .. 1 big hall",
    ),
    info(
        Param::ReverbTone,
        "reverb_tone",
        "Reverb tone",
        (0.0, 1.0),
        Scale::Linear,
        Scope::Master,
        "shared reverb tone 0 dark .. 1 bright",
    ),
    info(
        Param::Unknown,
        "unknown",
        "Unknown",
        (0.0, 1.0),
        Scale::Linear,
        Scope::Track,
        "a setting this version of AIDAW does not know",
    ),
];

impl Param {
    /// Every parameter that can be automated, track ones first.
    pub const ALL: [Param; 13] = [
        Param::Volume,
        Param::Pan,
        Param::Reverb,
        Param::Cutoff,
        Param::Resonance,
        Param::Drive,
        Param::Fm,
        Param::Vibrato,
        Param::Sub,
        Param::Noise,
        Param::MasterVolume,
        Param::ReverbSize,
        Param::ReverbTone,
    ];

    pub fn info(self) -> &'static Info {
        &INFO[self as usize]
    }

    pub fn id(self) -> &'static str {
        self.info().id
    }

    /// The parameter called `id` in the project file.
    pub fn from_id(id: &str) -> Option<Param> {
        Param::ALL.into_iter().find(|p| p.id() == id)
    }

    /// The parameter a person or an AI most likely means: ids, plus common
    /// words such as "gain", "filter", "brightness" or "send" (case and
    /// punctuation ignored).
    pub fn parse(s: &str) -> Option<Param> {
        let n: String = s
            .chars()
            .filter(|c| c.is_alphanumeric())
            .flat_map(char::to_lowercase)
            .collect();
        if let Some(p) = Param::ALL
            .into_iter()
            .find(|p| p.id().replace('_', "") == n)
        {
            return Some(p);
        }
        Some(match n.as_str() {
            "gain" | "level" | "vol" | "fader" | "loudness" => Param::Volume,
            "panning" | "balance" | "stereo" => Param::Pan,
            "send" | "reverbsend" | "wet" | "space" => Param::Reverb,
            "filter" | "filtercutoff" | "lowpass" | "lpf" | "brightness" | "freq" | "frequency" => {
                Param::Cutoff
            }
            "res" | "q" | "filterresonance" | "peak" => Param::Resonance,
            "distortion" | "saturation" | "overdrive" | "dirt" => Param::Drive,
            "fmamount" | "fmdepth" => Param::Fm,
            "vibratodepth" | "wobble" => Param::Vibrato,
            "sublevel" | "suboscillator" => Param::Sub,
            "noiselevel" | "air" | "breath" => Param::Noise,
            "master" | "mastergain" | "mastervol" | "songvolume" => Param::MasterVolume,
            "room" | "roomsize" | "reverbroom" | "size" => Param::ReverbSize,
            "tone" | "roomtone" | "reverbcolor" | "reverbcolour" => Param::ReverbTone,
            _ => return None,
        })
    }

    pub fn scope(self) -> Scope {
        self.info().scope
    }

    pub fn is_master(self) -> bool {
        self.scope() == Scope::Master
    }

    /// Whether a lane for this setting does anything on a track that plays
    /// `inst` (master settings never do; synth settings only on the built-in
    /// synth, not on drum kits or plugins).
    pub fn suits_track(self, inst: &crate::model::Instrument) -> bool {
        match self.scope() {
            Scope::Track => self != Param::Unknown,
            Scope::Synth => matches!(inst, crate::model::Instrument::Synth(_)),
            Scope::Master => false,
        }
    }
}

impl Serialize for Param {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.id())
    }
}

impl<'de> Deserialize<'de> for Param {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = std::borrow::Cow::<str>::deserialize(d)?;
        Ok(Param::from_id(&s).unwrap_or(Param::Unknown))
    }
}

impl Info {
    pub fn clamp(&self, v: f32) -> f32 {
        if v.is_nan() {
            self.min
        } else {
            v.clamp(self.min, self.max)
        }
    }

    /// Where `v` sits on a lane, from 0 (bottom) to 1 (top).
    pub fn to_unit(&self, v: f32) -> f32 {
        let v = self.clamp(v);
        let u = match self.scale {
            Scale::Linear => (v - self.min) / (self.max - self.min),
            Scale::Log => (v / self.min).ln() / (self.max / self.min).ln(),
            Scale::Fader => (v / self.max).cbrt(),
        };
        if u.is_finite() {
            u.clamp(0.0, 1.0)
        } else {
            0.0
        }
    }

    /// The value at height `u` (0..1) of a lane.
    pub fn from_unit(&self, u: f32) -> f32 {
        let u = if u.is_finite() {
            u.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let v = match self.scale {
            Scale::Linear => self.min + (self.max - self.min) * u,
            Scale::Log => self.min * ((self.max / self.min).ln() * u).exp(),
            Scale::Fader => self.max * u * u * u,
        };
        self.clamp(v)
    }

    /// The value as people read it: "1.2 kHz", "-0.50", "15 ct".
    pub fn format(&self, v: f32) -> String {
        match self.param {
            Param::Cutoff if v >= 1000.0 => format!("{:.1} kHz", v / 1000.0),
            Param::Cutoff => format!("{v:.0} Hz"),
            Param::Vibrato => format!("{v:.0} ct"),
            _ => format!("{v:.2}"),
        }
    }
}

/// How the value travels from a point to the next one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Shape {
    /// A straight line (in the knob's own feel, see [`Scale`]).
    #[default]
    Linear,
    /// Leaves this point slowly and arrives at the next one slowly: an S-curve.
    Smooth,
    /// Stays at this value, then jumps at the next point: steps.
    Hold,
}

impl Shape {
    pub const ALL: [Shape; 3] = [Shape::Linear, Shape::Smooth, Shape::Hold];

    pub fn id(self) -> &'static str {
        match self {
            Shape::Linear => "linear",
            Shape::Smooth => "smooth",
            Shape::Hold => "hold",
        }
    }

    pub fn parse(s: &str) -> Option<Shape> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "linear" | "line" | "straight" | "ramp" | "" => Shape::Linear,
            "smooth" | "curve" | "s" | "scurve" | "s-curve" | "ease" | "eased" | "sine" => {
                Shape::Smooth
            }
            "hold" | "step" | "steps" | "stair" | "stairs" | "square" | "jump" => Shape::Hold,
            _ => return None,
        })
    }

    fn is_linear(&self) -> bool {
        *self == Shape::Linear
    }

    /// Fraction of the way from one point to the next at `t` (0..1).
    pub fn ease(self, t: f32) -> f32 {
        match self {
            Shape::Linear => t,
            Shape::Smooth => t * t * (3.0 - 2.0 * t),
            Shape::Hold => 0.0,
        }
    }
}

/// One point on a lane.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Point {
    /// Position from the start of the song, in beats.
    pub beat: f64,
    /// The setting's value here, in its own units.
    pub value: f32,
    /// How the value moves on to the next point.
    #[serde(default, skip_serializing_if = "Shape::is_linear")]
    pub shape: Shape,
}

impl Point {
    pub fn new(beat: f64, value: f32) -> Self {
        Point {
            beat,
            value,
            shape: Shape::Linear,
        }
    }
}

/// The points of one setting, sorted by beat. Two points on the same beat
/// make the value jump there.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Lane {
    pub param: Param,
    #[serde(default, deserialize_with = "sorted_points")]
    pub points: Vec<Point>,
}

/// Points as written in a file, sorted (a hand-edited file may not be).
fn sorted_points<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<Point>, D::Error> {
    let mut points = Vec::<Point>::deserialize(d)?;
    sort(&mut points);
    Ok(points)
}

/// Sort by beat, keeping the order of points on the same beat.
pub fn sort(points: &mut [Point]) {
    points.sort_by(|a, b| a.beat.total_cmp(&b.beat));
}

impl Lane {
    pub fn new(param: Param) -> Self {
        Lane {
            param,
            points: Vec::new(),
        }
    }

    pub fn info(&self) -> &'static Info {
        self.param.info()
    }

    /// The value at `beat`. Where two points share a beat, the later one
    /// counts from that beat on. `None` when the lane has no points.
    pub fn value_at(&self, beat: f64) -> Option<f32> {
        let i = self.points.partition_point(|p| p.beat <= beat);
        self.between(i, beat)
    }

    /// The value just before `beat` (where a jump is, the value it jumps from).
    pub fn value_before(&self, beat: f64) -> Option<f32> {
        let i = self.points.partition_point(|p| p.beat < beat);
        self.between(i, beat)
    }

    /// The value at `beat`, which lies between point `i - 1` and point `i`.
    fn between(&self, i: usize, beat: f64) -> Option<f32> {
        let info = self.info();
        let first = self.points.first()?;
        if i == 0 {
            return Some(info.clamp(first.value));
        }
        let a = self.points[i - 1];
        let Some(b) = self.points.get(i) else {
            return Some(info.clamp(a.value));
        };
        let span = b.beat - a.beat;
        let t = if span > 0.0 {
            ((beat - a.beat) / span).clamp(0.0, 1.0) as f32
        } else {
            1.0
        };
        if a.shape == Shape::Hold {
            return Some(info.clamp(a.value));
        }
        let (ua, ub) = (info.to_unit(a.value), info.to_unit(b.value));
        Some(info.from_unit(ua + (ub - ua) * a.shape.ease(t)))
    }

    /// Lowest and highest value the lane reaches.
    pub fn range(&self) -> Option<(f32, f32)> {
        let info = self.info();
        let mut it = self.points.iter().map(|p| info.clamp(p.value));
        let first = it.next()?;
        Some(it.fold((first, first), |(lo, hi), v| (lo.min(v), hi.max(v))))
    }

    /// Put `points` on the lane: the points it had from the first to the
    /// last new one (inclusive) make way for them; the rest stay.
    pub fn write(&mut self, mut points: Vec<Point>) {
        sort(&mut points);
        let (Some(first), Some(last)) = (points.first(), points.last()) else {
            return;
        };
        let (from, to) = (first.beat, last.beat);
        self.points.retain(|p| p.beat < from || p.beat > to);
        let at = self.points.partition_point(|p| p.beat < from);
        self.points.splice(at..at, points);
    }

    /// Remove the points in `[from, to)`. Returns how many went.
    pub fn clear_range(&mut self, from: f64, to: f64) -> usize {
        let before = self.points.len();
        self.points.retain(|p| p.beat < from || p.beat >= to);
        before - self.points.len()
    }

    /// Where a new point at `beat` goes so the lane stays sorted (after
    /// any points already on that beat).
    pub fn insert(&mut self, point: Point) -> usize {
        let i = self.points.partition_point(|p| p.beat <= point.beat);
        self.points.insert(i, point);
        i
    }
}

/// The lane for `param` among `lanes`, if it has any points.
pub fn find(lanes: &[Lane], param: Param) -> Option<&Lane> {
    lanes
        .iter()
        .find(|l| l.param == param && !l.points.is_empty())
}

/// The automated value of `param` at `beat`, if it has a lane with points.
pub fn value(lanes: &[Lane], param: Param, beat: f64) -> Option<f32> {
    find(lanes, param)?.value_at(beat)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lane(param: Param, points: &[(f64, f32)]) -> Lane {
        Lane {
            param,
            points: points.iter().map(|&(b, v)| Point::new(b, v)).collect(),
        }
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() <= 1e-3 * b.abs().max(1.0)
    }

    #[test]
    fn values_glide_between_points_and_hold_outside() {
        let l = lane(Param::Pan, &[(4.0, -1.0), (8.0, 1.0)]);
        assert_eq!(l.value_at(0.0), Some(-1.0));
        assert_eq!(l.value_at(4.0), Some(-1.0));
        assert!(close(l.value_at(6.0).unwrap(), 0.0));
        assert_eq!(l.value_at(8.0), Some(1.0));
        assert_eq!(l.value_at(100.0), Some(1.0));
        assert_eq!(Lane::new(Param::Pan).value_at(1.0), None);
    }

    #[test]
    fn cutoff_sweeps_in_octaves_and_volume_fades_like_a_fader() {
        // 100 Hz -> 6400 Hz is six octaves: half way is three, 800 Hz.
        let l = lane(Param::Cutoff, &[(0.0, 100.0), (8.0, 6400.0)]);
        assert!(
            close(l.value_at(4.0).unwrap(), 800.0),
            "{:?}",
            l.value_at(4.0)
        );
        // A fade in from silence spends its first half quiet (about -20 dB).
        let fade = lane(Param::Volume, &[(0.0, 0.0), (4.0, 1.0)]);
        let half = fade.value_at(2.0).unwrap();
        assert!((0.1..0.15).contains(&half), "{half}");
        assert!(fade.value_at(1.0).unwrap() < fade.value_at(3.0).unwrap());
    }

    #[test]
    fn shapes_and_jumps() {
        let mut l = lane(Param::Reverb, &[(0.0, 0.0), (4.0, 1.0)]);
        l.points[0].shape = Shape::Smooth;
        // An S-curve starts slower than a line and meets it half way.
        assert!(l.value_at(1.0).unwrap() < 0.25);
        assert!(close(l.value_at(2.0).unwrap(), 0.5));
        l.points[0].shape = Shape::Hold;
        assert_eq!(l.value_at(3.99), Some(0.0));
        assert_eq!(l.value_at(4.0), Some(1.0));
        // Two points on one beat: a jump.
        let j = lane(
            Param::Reverb,
            &[(0.0, 0.0), (2.0, 0.5), (2.0, 1.0), (4.0, 1.0)],
        );
        assert!(close(j.value_at(1.0).unwrap(), 0.25));
        assert!(close(j.value_before(2.0).unwrap(), 0.5));
        assert_eq!(j.value_at(2.0), Some(1.0));
    }

    #[test]
    fn values_stay_in_range() {
        let l = lane(Param::Resonance, &[(0.0, -3.0), (1.0, 9.0)]);
        assert_eq!(l.value_at(0.0), Some(0.0));
        assert_eq!(l.value_at(1.0), Some(1.0));
        let c = lane(Param::Cutoff, &[(0.0, 0.0)]);
        assert_eq!(c.value_at(0.0), Some(20.0));
        for p in Param::ALL {
            let i = p.info();
            for u in [0.0, 0.3, 1.0] {
                assert!(close(i.to_unit(i.from_unit(u)), u), "{p:?} {u}");
            }
            assert_eq!(i.param, p);
            assert_eq!(Param::from_id(i.id), Some(p));
            assert_eq!(Param::parse(i.id), Some(p));
        }
    }

    #[test]
    fn writing_replaces_only_the_span_it_covers() {
        let mut l = lane(
            Param::Volume,
            &[(0.0, 0.1), (4.0, 0.2), (8.0, 0.3), (12.0, 0.4)],
        );
        l.write(vec![Point::new(6.0, 1.0), Point::new(4.0, 0.9)]);
        let beats: Vec<f64> = l.points.iter().map(|p| p.beat).collect();
        assert_eq!(beats, [0.0, 4.0, 6.0, 8.0, 12.0]);
        assert_eq!(l.points[1].value, 0.9);
        assert_eq!(l.clear_range(4.0, 8.0), 2);
        assert_eq!(l.points.len(), 3);
        let i = l.insert(Point::new(8.0, 0.5));
        assert_eq!((i, l.points[i + 1].beat), (2, 12.0));
    }

    #[test]
    fn files_name_params_plainly_and_survive_unknown_ones() {
        let json = r#"{"param":"cutoff","points":[{"beat":4.0,"value":800.0},{"beat":0.0,"value":200.0,"shape":"smooth"}]}"#;
        let l: Lane = serde_json::from_str(json).unwrap();
        assert_eq!(l.param, Param::Cutoff);
        // Sorted on load.
        assert_eq!((l.points[0].beat, l.points[0].shape), (0.0, Shape::Smooth));
        let back = serde_json::to_string(&l).unwrap();
        assert_eq!(
            back,
            r#"{"param":"cutoff","points":[{"beat":0.0,"value":200.0,"shape":"smooth"},{"beat":4.0,"value":800.0}]}"#
        );
        let future: Lane = serde_json::from_str(r#"{"param":"warp_drive","points":[]}"#).unwrap();
        assert_eq!(future.param, Param::Unknown);
        assert_eq!(Param::parse("Filter Cutoff"), Some(Param::Cutoff));
        assert_eq!(Param::parse("gain"), Some(Param::Volume));
        assert_eq!(Param::parse("room size"), Some(Param::ReverbSize));
        assert_eq!(Param::parse("nope"), None);
        assert_eq!(Shape::parse("Steps"), Some(Shape::Hold));
    }
}
