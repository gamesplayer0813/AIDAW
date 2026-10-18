//! Friendly instrument presets so beginners pick "Bass" instead of tuning an ADSR.

use aidaw_core::{Instrument, Waveform};

pub struct Preset {
    pub name: &'static str,
    pub hint: &'static str,
    pub instrument: Instrument,
}

const fn synth(
    waveform: Waveform,
    attack: f32,
    decay: f32,
    sustain: f32,
    release: f32,
    cutoff: f32,
) -> Instrument {
    Instrument::Synth {
        waveform,
        attack,
        decay,
        sustain,
        release,
        cutoff,
    }
}

pub fn all() -> [Preset; 9] {
    [
        Preset {
            name: "🎹 Soft Keys",
            hint: "A gentle piano-like sound. Good for chords and melodies.",
            instrument: synth(Waveform::Triangle, 0.005, 0.8, 0.25, 0.4, 5000.0),
        },
        Preset {
            name: "🎸 Bass",
            hint: "Deep and punchy. Use low notes (around C2).",
            instrument: synth(Waveform::Square, 0.003, 0.15, 0.6, 0.08, 900.0),
        },
        Preset {
            name: "☁ Warm Pad",
            hint: "Slow, soft and wide. Hold long chords with it.",
            instrument: synth(Waveform::Saw, 0.4, 0.5, 0.8, 1.2, 1800.0),
        },
        Preset {
            name: "⚡ Bright Lead",
            hint: "Cuts through the mix. Great for the main melody.",
            instrument: synth(Waveform::Saw, 0.01, 0.2, 0.7, 0.2, 7000.0),
        },
        Preset {
            name: "💧 Pluck",
            hint: "Short and bouncy. Try fast repeating notes.",
            instrument: synth(Waveform::Saw, 0.002, 0.25, 0.0, 0.2, 3000.0),
        },
        Preset {
            name: "🔔 Bell",
            hint: "Clear, ringing tone. Nice for high sparkly notes.",
            instrument: synth(Waveform::Sine, 0.005, 0.9, 0.0, 0.8, 20000.0),
        },
        Preset {
            name: "👾 8-bit",
            hint: "Retro video game sound.",
            instrument: synth(Waveform::Square, 0.002, 0.1, 0.8, 0.05, 12000.0),
        },
        Preset {
            name: "〰 Pure Tone",
            hint: "A plain, smooth sine wave.",
            instrument: synth(Waveform::Sine, 0.01, 0.2, 0.8, 0.2, 20000.0),
        },
        Preset {
            name: "💥 Drum Kit",
            hint: "Kick, snare, hi-hats and more. Each row is a different drum.",
            instrument: Instrument::Drums,
        },
    ]
}

/// Name of the preset that exactly matches `inst`, if any.
pub fn name_of(inst: &Instrument) -> Option<&'static str> {
    all()
        .into_iter()
        .find(|p| &p.instrument == inst)
        .map(|p| p.name)
}

/// The preset name without its emoji, for naming new tracks.
pub fn plain_name(name: &str) -> &str {
    name.split_once(' ').map(|(_, rest)| rest).unwrap_or(name)
}
