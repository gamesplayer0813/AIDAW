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
    #[serde(default)]
    pub tracks: Vec<Track>,
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
}

impl Waveform {
    pub const ALL: [Waveform; 4] = [
        Waveform::Sine,
        Waveform::Triangle,
        Waveform::Saw,
        Waveform::Square,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Waveform::Sine => "sine",
            Waveform::Triangle => "triangle",
            Waveform::Saw => "saw",
            Waveform::Square => "square",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Instrument {
    /// A simple polyphonic subtractive-style synth.
    Synth {
        #[serde(default)]
        waveform: Waveform,
        /// Envelope times in seconds, sustain level 0..1.
        #[serde(default = "d_attack")]
        attack: f32,
        #[serde(default = "d_decay")]
        decay: f32,
        #[serde(default = "d_sustain")]
        sustain: f32,
        #[serde(default = "d_release")]
        release: f32,
        /// One-pole low-pass cutoff in Hz.
        #[serde(default = "d_cutoff")]
        cutoff: f32,
    },
    /// Synthesised drum kit using General MIDI note numbers:
    /// 36 kick, 38 snare, 39 clap, 42 closed hat, 46 open hat,
    /// 41/45/48 toms, 49 crash.
    Drums,
}

impl Instrument {
    pub fn default_synth() -> Self {
        Instrument::Synth {
            waveform: Waveform::Saw,
            attack: d_attack(),
            decay: d_decay(),
            sustain: d_sustain(),
            release: d_release(),
            cutoff: d_cutoff(),
        }
    }

    pub fn is_drums(&self) -> bool {
        matches!(self, Instrument::Drums)
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
