//! The project document. This is the single source of truth shared by the
//! GUI, the audio engine and the AI (through the MCP server). It serialises
//! to a human- and LLM-readable JSON file (`*.aidaw.json`).

use serde::{Deserialize, Serialize};

use crate::pitch;

pub const FORMAT_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Project {
    #[serde(default = "default_version")]
    pub version: u32,
    pub name: String,
    /// Tempo in beats per minute (a beat is a quarter note).
    pub bpm: f64,
    #[serde(default = "default_beats_per_bar")]
    pub beats_per_bar: u32,
    #[serde(default = "default_gain")]
    pub master_gain: f32,
    /// The shared reverb that tracks send to (see [`Track::reverb`]).
    #[serde(default, skip_serializing_if = "Reverb::is_default")]
    pub reverb: Reverb,
    #[serde(default)]
    pub tracks: Vec<Track>,
}

/// Settings of the shared reverb: one "room" every track can send to.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Reverb {
    /// Room size 0..1: a small room (short tail) .. a big hall (long tail).
    #[serde(default = "d_half")]
    pub size: f32,
    /// Tone 0..1: dark, soft echoes .. bright, airy ones.
    #[serde(default = "d_half")]
    pub tone: f32,
}

impl Default for Reverb {
    fn default() -> Self {
        Reverb {
            size: 0.5,
            tone: 0.5,
        }
    }
}

impl Reverb {
    fn is_default(&self) -> bool {
        *self == Reverb::default()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Track {
    pub id: String,
    pub name: String,
    pub instrument: Instrument,
    /// Linear gain, 1.0 = unity.
    #[serde(default = "default_gain")]
    pub gain: f32,
    /// -1.0 (left) .. 1.0 (right).
    #[serde(default)]
    pub pan: f32,
    #[serde(default)]
    pub mute: bool,
    #[serde(default)]
    pub solo: bool,
    /// How much of this track goes to the shared reverb, 0..1 (0 = dry).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub reverb: f32,
    #[serde(default)]
    pub clips: Vec<Clip>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Waveform {
    Sine,
    Triangle,
    #[default]
    Saw,
    Square,
    /// Narrow (25%) pulse: thinner and more nasal than a square.
    Pulse,
    /// White noise: wind, breath, hiss and risers.
    Noise,
}

impl Waveform {
    pub const ALL: [Waveform; 6] = [
        Waveform::Sine,
        Waveform::Triangle,
        Waveform::Saw,
        Waveform::Square,
        Waveform::Pulse,
        Waveform::Noise,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Waveform::Sine => "sine",
            Waveform::Triangle => "triangle",
            Waveform::Saw => "saw",
            Waveform::Square => "square",
            Waveform::Pulse => "pulse",
            Waveform::Noise => "noise",
        }
    }
}

/// Settings of the built-in synth. Only `waveform` and the envelope/cutoff
/// are needed; every other knob defaults to "off" and is left out of the
/// JSON when unused, so older project files load and save unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Synth {
    #[serde(default)]
    pub waveform: Waveform,
    /// Envelope times in seconds, sustain level 0..1.
    #[serde(default = "d_attack")]
    pub attack: f32,
    #[serde(default = "d_decay")]
    pub decay: f32,
    #[serde(default = "d_sustain")]
    pub sustain: f32,
    #[serde(default = "d_release")]
    pub release: f32,
    /// Low-pass cutoff in Hz.
    #[serde(default = "d_cutoff")]
    pub cutoff: f32,
    /// Filter resonance 0..1: a whistling peak at the cutoff.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub resonance: f32,
    /// How far the cutoff jumps up at the start of a note, in octaves
    /// (0..8). It falls back over `filter_decay` seconds: the "wow" of
    /// plucks, basses and brass.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub filter_env: f32,
    #[serde(default = "d_filter_decay", skip_serializing_if = "is_filter_decay")]
    pub filter_decay: f32,
    /// Number of stacked, detuned copies of the oscillator (1..5).
    /// Unset: saws use 2, other waves 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unison: Option<u8>,
    /// Total spread of the unison copies in cents (unset: 7 for the classic
    /// two-saw stack, else 14).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detune: Option<f32>,
    /// Level 0..1 of a sine one octave below: weight for basses.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub sub: f32,
    /// Level 0..1 of white noise mixed in: breath, hiss, grit.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub noise: f32,
    /// FM amount (0..10) for the sine and triangle waves: electric pianos,
    /// bells, mallets. Fades over `fm_decay` seconds (0 = never).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub fm: f32,
    /// Modulator frequency as a multiple of the note (1 = warm, 2 = hollow,
    /// 3.5 = metallic bell).
    #[serde(default = "d_one", skip_serializing_if = "is_one")]
    pub fm_ratio: f32,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub fm_decay: f32,
    /// Vibrato depth in cents (fades in over the first half second).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub vibrato: f32,
    #[serde(default = "d_vibrato_rate", skip_serializing_if = "is_vibrato_rate")]
    pub vibrato_rate: f32,
    /// Pitch at the start of a note, in semitones above the played note,
    /// gliding down over `pitch_decay` seconds (808 basses, zaps, toms).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub pitch_env: f32,
    #[serde(default = "d_pitch_decay", skip_serializing_if = "is_pitch_decay")]
    pub pitch_decay: f32,
    /// Saturation 0..1: warmth at low values, distortion at high ones.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub drive: f32,
}

impl Default for Synth {
    fn default() -> Self {
        Synth::BASE
    }
}

impl Synth {
    /// The plain default synth; presets start from this.
    pub const BASE: Synth = Synth {
        waveform: Waveform::Saw,
        attack: 0.005,
        decay: 0.2,
        sustain: 0.7,
        release: 0.25,
        cutoff: 6000.0,
        resonance: 0.0,
        filter_env: 0.0,
        filter_decay: 0.3,
        unison: None,
        detune: None,
        sub: 0.0,
        noise: 0.0,
        fm: 0.0,
        fm_ratio: 1.0,
        fm_decay: 0.0,
        vibrato: 0.0,
        vibrato_rate: 5.0,
        pitch_env: 0.0,
        pitch_decay: 0.05,
        drive: 0.0,
    };
}

/// Which synthesised drum kit a drum track plays.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum DrumKit {
    /// The original all-round electronic kit.
    #[default]
    Standard,
    /// Roland TR-808 style: boomy kick, snappy snare, sizzling hats.
    #[serde(rename = "808")]
    Tr808,
    /// TR-909 style: punchy house/techno kick, bright noisy snare.
    #[serde(rename = "909")]
    Tr909,
    /// Natural sounding drum set: woody kick, ringing toms.
    Acoustic,
    /// Dusty, dark and crunchy, for lo-fi hip-hop.
    Lofi,
    /// Long sub kick, tight snare, crisp hats.
    Trap,
    /// Hard, gritty and distorted.
    Electro,
}

impl DrumKit {
    pub const ALL: [DrumKit; 7] = [
        DrumKit::Standard,
        DrumKit::Tr808,
        DrumKit::Tr909,
        DrumKit::Acoustic,
        DrumKit::Lofi,
        DrumKit::Trap,
        DrumKit::Electro,
    ];

    pub fn label(self) -> &'static str {
        match self {
            DrumKit::Standard => "standard",
            DrumKit::Tr808 => "808",
            DrumKit::Tr909 => "909",
            DrumKit::Acoustic => "acoustic",
            DrumKit::Lofi => "lofi",
            DrumKit::Trap => "trap",
            DrumKit::Electro => "electro",
        }
    }

    fn is_standard(&self) -> bool {
        *self == DrumKit::Standard
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Instrument {
    /// A polyphonic subtractive synth with optional FM, unison and sub.
    Synth(Synth),
    /// Synthesised drum kit using General MIDI note numbers (see
    /// `music::DRUMS`): 36 kick, 38 snare, 39 clap, 42 closed hat, 46 open
    /// hat, 41/45/48 toms, 49 crash, 51 ride, plus percussion such as 56
    /// cowbell, 54 tambourine, 70 shaker and 60-64 bongos/congas.
    Drums {
        #[serde(default, skip_serializing_if = "DrumKit::is_standard")]
        kit: DrumKit,
    },
}

impl Instrument {
    pub fn default_synth() -> Self {
        Instrument::Synth(Synth::BASE)
    }

    pub fn is_drums(&self) -> bool {
        matches!(self, Instrument::Drums { .. })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Clip {
    pub id: String,
    #[serde(default)]
    pub name: String,
    /// Position on the timeline, in beats.
    pub start: f64,
    /// Length in beats. Notes outside [0, length) are not played.
    pub length: f64,
    #[serde(default)]
    pub notes: Vec<Note>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Note {
    /// MIDI note number. Accepts a number (60) or a name ("C4", "F#3", "Bb2").
    #[serde(deserialize_with = "pitch::deserialize")]
    pub pitch: u8,
    /// Start in beats, relative to the clip start.
    pub start: f64,
    /// Duration in beats.
    pub duration: f64,
    #[serde(default = "d_velocity")]
    pub velocity: u8,
}

impl Clip {
    pub fn end(&self) -> f64 {
        self.start + self.length
    }
}

impl Project {
    pub fn new(name: impl Into<String>, bpm: f64) -> Self {
        Project {
            version: FORMAT_VERSION,
            name: name.into(),
            bpm,
            beats_per_bar: 4,
            master_gain: 0.8,
            reverb: Reverb::default(),
            tracks: Vec::new(),
        }
    }

    pub fn seconds_per_beat(&self) -> f64 {
        60.0 / self.bpm.max(1.0)
    }

    /// End of the last clip, in beats.
    pub fn length_beats(&self) -> f64 {
        self.tracks
            .iter()
            .flat_map(|t| t.clips.iter())
            .map(|c| c.end())
            .fold(0.0, f64::max)
    }

    /// Where live playback wraps back to the start: the end of the last
    /// playable note, rounded up to a whole bar so loops stay in time.
    /// `None` when the song has no notes.
    pub fn loop_end_beats(&self) -> Option<f64> {
        let last = self
            .tracks
            .iter()
            .flat_map(|t| t.clips.iter())
            .flat_map(|c| {
                c.notes
                    .iter()
                    .filter(|n| n.start >= 0.0 && n.start < c.length && n.duration > 0.0)
                    .map(|n| c.start + (n.start + n.duration).min(c.length))
            })
            .filter(|b| b.is_finite())
            .fold(0.0, f64::max);
        if last <= 0.0 {
            return None;
        }
        let bar = f64::from(self.beats_per_bar.max(1));
        // Tolerate float noise so a note ending exactly on a bar line doesn't add a bar.
        Some(((last - 1e-6) / bar).ceil().max(1.0) * bar)
    }

    pub fn track(&self, id: &str) -> Option<&Track> {
        self.tracks.iter().find(|t| t.id == id)
    }

    pub fn track_mut(&mut self, id: &str) -> Option<&mut Track> {
        self.tracks.iter_mut().find(|t| t.id == id)
    }

    pub fn clip_mut(&mut self, id: &str) -> Option<&mut Clip> {
        self.tracks
            .iter_mut()
            .flat_map(|t| t.clips.iter_mut())
            .find(|c| c.id == id)
    }

    pub fn clip(&self, id: &str) -> Option<(&Track, &Clip)> {
        self.tracks
            .iter()
            .find_map(|t| t.clips.iter().find(|c| c.id == id).map(|c| (t, c)))
    }

    /// Generate an id with the given prefix that is not used by any track or clip.
    pub fn next_id(&self, prefix: &str) -> String {
        let max = self
            .tracks
            .iter()
            .map(|t| t.id.as_str())
            .chain(
                self.tracks
                    .iter()
                    .flat_map(|t| t.clips.iter().map(|c| c.id.as_str())),
            )
            .filter_map(|id| id.strip_prefix(prefix)?.parse::<u64>().ok())
            .max()
            .unwrap_or(0);
        format!("{prefix}{}", max + 1)
    }

    pub fn any_solo(&self) -> bool {
        self.tracks.iter().any(|t| t.solo)
    }

    pub fn track_audible(&self, track: &Track) -> bool {
        !track.mute && (!self.any_solo() || track.solo)
    }
}

fn default_version() -> u32 {
    FORMAT_VERSION
}
fn default_beats_per_bar() -> u32 {
    4
}
fn default_gain() -> f32 {
    0.8
}
fn d_attack() -> f32 {
    0.005
}
fn d_decay() -> f32 {
    0.2
}
fn d_sustain() -> f32 {
    0.7
}
fn d_release() -> f32 {
    0.25
}
fn d_cutoff() -> f32 {
    6000.0
}
fn d_velocity() -> u8 {
    100
}
fn d_filter_decay() -> f32 {
    0.3
}
fn d_one() -> f32 {
    1.0
}
fn d_half() -> f32 {
    0.5
}
fn d_vibrato_rate() -> f32 {
    5.0
}
fn d_pitch_decay() -> f32 {
    0.05
}
fn is_zero(x: &f32) -> bool {
    *x == 0.0
}
fn is_one(x: &f32) -> bool {
    *x == 1.0
}
fn is_filter_decay(x: &f32) -> bool {
    *x == d_filter_decay()
}
fn is_vibrato_rate(x: &f32) -> bool {
    *x == d_vibrato_rate()
}
fn is_pitch_decay(x: &f32) -> bool {
    *x == d_pitch_decay()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_instrument_json_still_loads_and_saves_the_same() {
        let synth = r#"{"type":"synth","waveform":"square","attack":0.003,"decay":0.15,"sustain":0.6,"release":0.08,"cutoff":900.0}"#;
        let inst: Instrument = serde_json::from_str(synth).unwrap();
        assert_eq!(serde_json::to_string(&inst).unwrap(), synth);
        let drums: Instrument = serde_json::from_str(r#"{"type":"drums"}"#).unwrap();
        assert_eq!(
            drums,
            Instrument::Drums {
                kit: DrumKit::Standard
            }
        );
        assert_eq!(
            serde_json::to_string(&drums).unwrap(),
            r#"{"type":"drums"}"#
        );
    }

    #[test]
    fn projects_without_reverb_load_and_save_unchanged() {
        let json = r#"{"version":1,"name":"x","bpm":120.0,"beats_per_bar":4,"master_gain":0.8,"tracks":[{"id":"t1","name":"A","instrument":{"type":"drums"},"gain":0.8,"pan":0.0,"mute":false,"solo":false,"clips":[]}]}"#;
        let p: Project = serde_json::from_str(json).unwrap();
        assert_eq!(p.reverb, Reverb::default());
        assert_eq!(p.tracks[0].reverb, 0.0);
        assert_eq!(serde_json::to_string(&p).unwrap(), json);

        let mut q = p.clone();
        q.reverb.size = 0.75;
        q.tracks[0].reverb = 0.25;
        let s = serde_json::to_string(&q).unwrap();
        assert!(s.contains(r#""reverb":{"size":0.75,"tone":0.5}"#), "{s}");
        assert!(s.contains(r#""solo":false,"reverb":0.25"#), "{s}");
        assert_eq!(serde_json::from_str::<Project>(&s).unwrap(), q);
    }

    #[test]
    fn new_settings_round_trip() {
        let json = r#"{"type":"synth","waveform":"pulse","fm":2.0,"fm_ratio":3.5,"unison":3,"detune":20.0}"#;
        let inst: Instrument = serde_json::from_str(json).unwrap();
        let Instrument::Synth(s) = &inst else {
            panic!()
        };
        assert_eq!(
            (s.waveform, s.fm, s.unison),
            (Waveform::Pulse, 2.0, Some(3))
        );
        assert_eq!(s.attack, Synth::BASE.attack);
        let back: Instrument =
            serde_json::from_str(&serde_json::to_string(&inst).unwrap()).unwrap();
        assert_eq!(back, inst);
        let kit: Instrument = serde_json::from_str(r#"{"type":"drums","kit":"808"}"#).unwrap();
        assert_eq!(
            kit,
            Instrument::Drums {
                kit: DrumKit::Tr808
            }
        );
    }
}
