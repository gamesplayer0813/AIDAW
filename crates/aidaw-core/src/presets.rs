//! Friendly instrument presets so beginners (and AI tools) pick "Bass"
//! instead of tuning an ADSR. Shared by the GUI and the MCP server.

use crate::model::{DrumKit, Instrument, Synth, Waveform};

/// Groups shown as tabs in the GUI and as headings in `list_presets`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    Keys,
    Bass,
    Lead,
    Pad,
    Pluck,
    Mallets,
    Orchestra,
    Retro,
    Fx,
    Drums,
}

impl Category {
    pub const ALL: [Category; 10] = [
        Category::Keys,
        Category::Bass,
        Category::Lead,
        Category::Pad,
        Category::Pluck,
        Category::Mallets,
        Category::Orchestra,
        Category::Retro,
        Category::Fx,
        Category::Drums,
    ];

    /// Display name with an icon, e.g. "🎹 Keys".
    pub fn label(self) -> &'static str {
        match self {
            Category::Keys => "🎹 Keys",
            Category::Bass => "🎸 Bass",
            Category::Lead => "⚡ Lead",
            Category::Pad => "☁ Pads",
            Category::Pluck => "💧 Plucks",
            Category::Mallets => "🔔 Bells",
            Category::Orchestra => "🎻 Orchestra",
            Category::Retro => "👾 Retro",
            Category::Fx => "🌀 FX",
            Category::Drums => "💥 Drums",
        }
    }

    /// Reverb send a new track with a preset of this group starts with:
    /// pads and bells float in the room, basses stay dry and tight.
    pub fn reverb(self) -> f32 {
        match self {
            Category::Pad | Category::Mallets | Category::Orchestra | Category::Fx => 0.3,
            Category::Pluck => 0.25,
            Category::Keys | Category::Lead => 0.2,
            Category::Retro | Category::Drums => 0.1,
            Category::Bass => 0.0,
        }
    }

    /// Machine name, e.g. `"keys"`.
    pub fn id(self) -> &'static str {
        match self {
            Category::Keys => "keys",
            Category::Bass => "bass",
            Category::Lead => "lead",
            Category::Pad => "pad",
            Category::Pluck => "pluck",
            Category::Mallets => "bells",
            Category::Orchestra => "orchestra",
            Category::Retro => "retro",
            Category::Fx => "fx",
            Category::Drums => "drums",
        }
    }
}

pub struct Preset {
    /// Stable machine name, e.g. `soft_keys`; what AI tools pass around.
    pub id: &'static str,
    pub name: &'static str,
    pub hint: &'static str,
    pub category: Category,
    /// Other words that should find this preset ("rhodes", "808").
    pub aliases: &'static [&'static str],
    pub instrument: Instrument,
}

impl Preset {
    /// Every preset in this category, in display order.
    pub fn in_category(category: Category) -> impl Iterator<Item = &'static Preset> {
        all().iter().filter(move |p| p.category == category)
    }
}

const fn s(
    waveform: Waveform,
    attack: f32,
    decay: f32,
    sustain: f32,
    release: f32,
    cutoff: f32,
) -> Synth {
    Synth {
        waveform,
        attack,
        decay,
        sustain,
        release,
        cutoff,
        ..Synth::BASE
    }
}

const fn synth(p: Synth) -> Instrument {
    Instrument::Synth(p)
}

const fn kit(kit: DrumKit) -> Instrument {
    Instrument::Drums { kit }
}

use Category as C;
use Waveform::{Noise, Pulse, Saw, Sine, Square, Triangle};

static PRESETS: &[Preset] = &[
    // ---- Keys ----
    Preset {
        id: "soft_keys",
        name: "🎹 Soft Keys",
        hint: "A gentle piano-like sound. Good for chords and melodies.",
        category: C::Keys,
        aliases: &["piano", "keys"],
        instrument: synth(s(Triangle, 0.005, 0.8, 0.25, 0.4, 5000.0)),
    },
    Preset {
        id: "bright_piano",
        name: "🎹 Bright Piano",
        hint: "A brighter, punchier piano. Cuts through a busy song.",
        category: C::Keys,
        aliases: &["grand_piano", "acoustic_piano", "pop_piano"],
        instrument: synth(Synth {
            fm: 1.2,
            fm_ratio: 2.0,
            fm_decay: 0.4,
            ..s(Triangle, 0.002, 1.2, 0.2, 0.5, 9000.0)
        }),
    },
    Preset {
        id: "e_piano",
        name: "🎹 Electric Piano",
        hint: "Warm, bell-ish 70s electric piano. Lovely for soul, jazz and lo-fi chords.",
        category: C::Keys,
        aliases: &["rhodes", "ep", "electric_piano", "epiano", "dx7"],
        instrument: synth(Synth {
            fm: 1.8,
            fm_decay: 0.6,
            ..s(Sine, 0.002, 1.6, 0.15, 0.5, 8000.0)
        }),
    },
    Preset {
        id: "wurly",
        name: "🎹 Wurly",
        hint: "Reedy, slightly gritty electric piano for funk and indie.",
        category: C::Keys,
        aliases: &["wurlitzer"],
        instrument: synth(Synth {
            fm: 2.5,
            fm_decay: 0.3,
            drive: 0.3,
            ..s(Triangle, 0.002, 1.0, 0.2, 0.3, 5000.0)
        }),
    },
    Preset {
        id: "organ",
        name: "⛪ Organ",
        hint: "Rock and gospel drawbar organ. Hold chords; it never fades.",
        category: C::Keys,
        aliases: &["hammond", "drawbar", "rock_organ"],
        instrument: synth(Synth {
            fm: 0.9,
            fm_ratio: 2.0,
            sub: 0.5,
            vibrato: 8.0,
            vibrato_rate: 6.5,
            ..s(Sine, 0.01, 0.1, 1.0, 0.08, 12000.0)
        }),
    },
    Preset {
        id: "church_organ",
        name: "⛪ Church Organ",
        hint: "Big, hollow pipe organ. Slow chords sound grand.",
        category: C::Keys,
        aliases: &["pipe_organ", "cathedral"],
        instrument: synth(Synth {
            sub: 0.7,
            ..s(Square, 0.06, 0.2, 1.0, 0.6, 3000.0)
        }),
    },
    Preset {
        id: "clavinet",
        name: "🎹 Clavinet",
        hint: "Snappy, funky keyboard. Play short rhythmic notes.",
        category: C::Keys,
        aliases: &["clav", "funk_keys"],
        instrument: synth(Synth {
            resonance: 0.3,
            filter_env: 2.5,
            filter_decay: 0.12,
            ..s(Pulse, 0.001, 0.35, 0.1, 0.08, 2500.0)
        }),
    },
    Preset {
        id: "harpsichord",
        name: "🎼 Harpsichord",
        hint: "Baroque, plucked-string keyboard.",
        category: C::Keys,
        aliases: &["cembalo", "baroque"],
        instrument: synth(Synth {
            filter_env: 1.5,
            filter_decay: 0.2,
            ..s(Saw, 0.001, 0.9, 0.0, 0.3, 6000.0)
        }),
    },
    Preset {
        id: "lofi_keys",
        name: "📀 Lo-fi Keys",
        hint: "Dusty, wobbly electric piano, like an old tape. Chill beats.",
        category: C::Keys,
        aliases: &["lofi", "chill_keys", "tape_keys"],
        instrument: synth(Synth {
            fm: 1.4,
            fm_decay: 0.5,
            vibrato: 10.0,
            vibrato_rate: 0.8,
            drive: 0.2,
            ..s(Sine, 0.005, 1.2, 0.25, 0.5, 2200.0)
        }),
    },
    // ---- Bass ----
    Preset {
        id: "bass",
        name: "🎸 Bass",
        hint: "Deep and punchy. Use low notes (around C2).",
        category: C::Bass,
        aliases: &[],
        instrument: synth(s(Square, 0.003, 0.15, 0.6, 0.08, 900.0)),
    },
    Preset {
        id: "sub_bass",
        name: "🔈 Sub Bass",
        hint: "Pure, deep low end you feel more than hear. C1-C2.",
        category: C::Bass,
        aliases: &["sub", "deep_bass"],
        instrument: synth(Synth {
            drive: 0.15,
            ..s(Sine, 0.005, 0.1, 1.0, 0.12, 20000.0)
        }),
    },
    Preset {
        id: "bass_808",
        name: "💣 808 Bass",
        hint: "Booming hip-hop/trap bass with a punchy pitch drop. Long notes around C1-C2.",
        category: C::Bass,
        aliases: &["808", "trap_bass", "808_bass"],
        instrument: synth(Synth {
            pitch_env: 7.0,
            pitch_decay: 0.04,
            drive: 0.45,
            ..s(Sine, 0.002, 1.5, 0.3, 0.3, 20000.0)
        }),
    },
    Preset {
        id: "synth_bass",
        name: "🎸 Synth Bass",
        hint: "Fat analog bass with a little 'wow' on every note. Funk, pop, synthwave.",
        category: C::Bass,
        aliases: &["moog", "moog_bass", "analog_bass"],
        instrument: synth(Synth {
            resonance: 0.3,
            filter_env: 2.2,
            filter_decay: 0.15,
            sub: 0.5,
            ..s(Square, 0.002, 0.25, 0.5, 0.1, 600.0)
        }),
    },
    Preset {
        id: "acid_bass",
        name: "🐍 Acid Bass",
        hint: "Squelchy 303 acid line. Fast 16th notes around C2.",
        category: C::Bass,
        aliases: &["acid", "303", "tb303"],
        instrument: synth(Synth {
            resonance: 0.8,
            filter_env: 3.5,
            filter_decay: 0.18,
            drive: 0.4,
            unison: Some(1),
            ..s(Saw, 0.002, 0.2, 0.5, 0.08, 400.0)
        }),
    },
    Preset {
        id: "reese_bass",
        name: "🐉 Reese Bass",
        hint: "Wide, growling detuned bass for drum & bass and dubstep.",
        category: C::Bass,
        aliases: &["reese", "dnb_bass"],
        instrument: synth(Synth {
            unison: Some(3),
            detune: Some(30.0),
            sub: 0.4,
            drive: 0.3,
            ..s(Saw, 0.005, 0.3, 0.9, 0.15, 900.0)
        }),
    },
    Preset {
        id: "pluck_bass",
        name: "🎸 Pluck Bass",
        hint: "Short, bouncy bass for house and pop.",
        category: C::Bass,
        aliases: &["house_bass"],
        instrument: synth(Synth {
            filter_env: 3.0,
            filter_decay: 0.08,
            sub: 0.4,
            ..s(Saw, 0.002, 0.3, 0.0, 0.1, 300.0)
        }),
    },
    Preset {
        id: "finger_bass",
        name: "🎸 Finger Bass",
        hint: "Round electric bass guitar. Rock, funk, pop.",
        category: C::Bass,
        aliases: &["electric_bass", "bass_guitar"],
        instrument: synth(Synth {
            fm: 1.0,
            fm_decay: 0.15,
            sub: 0.3,
            ..s(Triangle, 0.004, 0.6, 0.3, 0.08, 1500.0)
        }),
    },
    Preset {
        id: "growl_bass",
        name: "🔥 Distorted Bass",
        hint: "Aggressive, fuzzy bass for rock, dubstep and metal.",
        category: C::Bass,
        aliases: &["dubstep", "growl", "fuzz_bass"],
        instrument: synth(Synth {
            unison: Some(2),
            detune: Some(12.0),
            sub: 0.5,
            drive: 0.85,
            resonance: 0.4,
            ..s(Saw, 0.003, 0.2, 0.8, 0.1, 1500.0)
        }),
    },
    // ---- Lead ----
    Preset {
        id: "bright_lead",
        name: "⚡ Bright Lead",
        hint: "Cuts through the mix. Great for the main melody.",
        category: C::Lead,
        aliases: &["lead"],
        instrument: synth(s(Saw, 0.01, 0.2, 0.7, 0.2, 7000.0)),
    },
    Preset {
        id: "supersaw",
        name: "🚀 Supersaw",
        hint: "Huge, shiny stack of saws. Trance and EDM anthems.",
        category: C::Lead,
        aliases: &["trance", "edm", "super_saw"],
        instrument: synth(Synth {
            unison: Some(5),
            detune: Some(35.0),
            ..s(Saw, 0.01, 0.3, 0.8, 0.3, 9000.0)
        }),
    },
    Preset {
        id: "analog_lead",
        name: "⭐ Analog Lead",
        hint: "Classic singing mono-synth lead with vibrato.",
        category: C::Lead,
        aliases: &["moog_lead", "mono_lead", "synth_lead"],
        instrument: synth(Synth {
            resonance: 0.45,
            filter_env: 1.5,
            filter_decay: 0.4,
            vibrato: 12.0,
            vibrato_rate: 5.5,
            ..s(Saw, 0.01, 0.3, 0.8, 0.2, 2500.0)
        }),
    },
    Preset {
        id: "square_lead",
        name: "🎮 Square Lead",
        hint: "Hollow, woody lead. Melodies that sound playful.",
        category: C::Lead,
        aliases: &["square"],
        instrument: synth(Synth {
            vibrato: 15.0,
            vibrato_rate: 5.5,
            ..s(Square, 0.005, 0.1, 0.9, 0.1, 5000.0)
        }),
    },
    Preset {
        id: "pulse_lead",
        name: "📻 Pulse Lead",
        hint: "Thin, nasal and bright lead. Synth-pop melodies.",
        category: C::Lead,
        aliases: &["pulse"],
        instrument: synth(Synth {
            vibrato: 10.0,
            ..s(Pulse, 0.005, 0.2, 0.8, 0.15, 6000.0)
        }),
    },
    Preset {
        id: "soft_lead",
        name: "🌙 Soft Lead",
        hint: "Smooth, whistle-like lead. Gentle melodies.",
        category: C::Lead,
        aliases: &["whistle", "smooth_lead"],
        instrument: synth(Synth {
            fm: 0.6,
            vibrato: 18.0,
            ..s(Sine, 0.03, 0.2, 0.9, 0.25, 20000.0)
        }),
    },
    Preset {
        id: "hard_lead",
        name: "🔥 Hard Lead",
        hint: "Screaming, distorted lead. Rock solos and big drops.",
        category: C::Lead,
        aliases: &["rock_lead", "distorted_lead", "scream"],
        instrument: synth(Synth {
            drive: 0.6,
            resonance: 0.5,
            filter_env: 1.0,
            filter_decay: 0.3,
            vibrato: 10.0,
            ..s(Saw, 0.005, 0.3, 0.8, 0.2, 3000.0)
        }),
    },
    Preset {
        id: "pure_tone",
        name: "〰 Pure Tone",
        hint: "A plain, smooth sine wave.",
        category: C::Lead,
        aliases: &["sine"],
        instrument: synth(s(Sine, 0.01, 0.2, 0.8, 0.2, 20000.0)),
    },
    // ---- Pads ----
    Preset {
        id: "warm_pad",
        name: "☁ Warm Pad",
        hint: "Slow, soft and wide. Hold long chords with it.",
        category: C::Pad,
        aliases: &["pad"],
        instrument: synth(s(Saw, 0.4, 0.5, 0.8, 1.2, 1800.0)),
    },
    Preset {
        id: "lush_pad",
        name: "🌌 Lush Pad",
        hint: "Huge, shimmering wall of sound. Big emotional chords.",
        category: C::Pad,
        aliases: &["wide_pad", "big_pad"],
        instrument: synth(Synth {
            unison: Some(5),
            detune: Some(25.0),
            ..s(Saw, 0.8, 1.0, 0.8, 2.0, 2500.0)
        }),
    },
    Preset {
        id: "glass_pad",
        name: "💎 Glass Pad",
        hint: "Clear, crystal-like pad. Dreamy and bright.",
        category: C::Pad,
        aliases: &["crystal_pad", "dream_pad"],
        instrument: synth(Synth {
            fm: 1.2,
            fm_ratio: 3.0,
            vibrato: 6.0,
            vibrato_rate: 4.0,
            ..s(Sine, 0.6, 1.0, 0.8, 1.8, 8000.0)
        }),
    },
    Preset {
        id: "dark_pad",
        name: "🌃 Dark Pad",
        hint: "Low, moody, cinematic. Great under sad or tense music.",
        category: C::Pad,
        aliases: &["ambient", "drone", "moody_pad"],
        instrument: synth(Synth {
            unison: Some(3),
            detune: Some(18.0),
            resonance: 0.2,
            sub: 0.3,
            ..s(Saw, 1.0, 1.0, 0.9, 2.5, 600.0)
        }),
    },
    Preset {
        id: "choir",
        name: "🎤 Choir",
        hint: "Soft 'aah' voices. Hold chords for an angelic sound.",
        category: C::Pad,
        aliases: &["voices", "aah", "vocal", "vox"],
        instrument: synth(Synth {
            unison: Some(3),
            detune: Some(15.0),
            resonance: 0.35,
            noise: 0.05,
            vibrato: 10.0,
            ..s(Pulse, 0.4, 0.5, 0.9, 1.2, 1400.0)
        }),
    },
    Preset {
        id: "airy_pad",
        name: "💨 Airy Pad",
        hint: "Breathy, floating pad with a soft hiss.",
        category: C::Pad,
        aliases: &["breathy", "air_pad"],
        instrument: synth(Synth {
            unison: Some(3),
            detune: Some(12.0),
            noise: 0.25,
            resonance: 0.3,
            ..s(Triangle, 0.7, 0.8, 0.8, 1.6, 4000.0)
        }),
    },
    Preset {
        id: "sweep_pad",
        name: "🌊 Sweep Pad",
        hint: "Each chord slowly opens up like a wave.",
        category: C::Pad,
        aliases: &["sweep", "filter_pad"],
        instrument: synth(Synth {
            unison: Some(3),
            detune: Some(20.0),
            resonance: 0.5,
            filter_env: 3.5,
            filter_decay: 2.5,
            ..s(Saw, 1.0, 2.0, 0.8, 2.0, 300.0)
        }),
    },
    // ---- Plucks & guitars ----
    Preset {
        id: "pluck",
        name: "💧 Pluck",
        hint: "Short and bouncy. Try fast repeating notes.",
        category: C::Pluck,
        aliases: &[],
        instrument: synth(s(Saw, 0.002, 0.25, 0.0, 0.2, 3000.0)),
    },
    Preset {
        id: "synth_pluck",
        name: "✨ Sparkle Pluck",
        hint: "Bright, wide pluck for future bass and pop arpeggios.",
        category: C::Pluck,
        aliases: &["future_pluck", "arp", "arpeggio"],
        instrument: synth(Synth {
            unison: Some(3),
            detune: Some(20.0),
            filter_env: 4.0,
            filter_decay: 0.12,
            ..s(Saw, 0.001, 0.4, 0.0, 0.3, 500.0)
        }),
    },
    Preset {
        id: "nylon_guitar",
        name: "🎸 Nylon Guitar",
        hint: "Soft acoustic guitar. Strum chords or pick melodies.",
        category: C::Pluck,
        aliases: &["guitar", "acoustic_guitar", "classical_guitar", "nylon"],
        instrument: synth(Synth {
            filter_env: 2.0,
            filter_decay: 0.06,
            unison: Some(1),
            ..s(Saw, 0.001, 1.2, 0.0, 0.3, 1200.0)
        }),
    },
    Preset {
        id: "clean_guitar",
        name: "🎸 Clean Guitar",
        hint: "Electric guitar without distortion. Funk, indie, pop.",
        category: C::Pluck,
        aliases: &["electric_guitar", "funk_guitar"],
        instrument: synth(Synth {
            filter_env: 1.5,
            filter_decay: 0.1,
            drive: 0.2,
            unison: Some(1),
            ..s(Saw, 0.001, 2.0, 0.15, 0.3, 2000.0)
        }),
    },
    Preset {
        id: "rock_guitar",
        name: "🔥 Rock Guitar",
        hint: "Crunchy distorted guitar. Power chords (root + fifth) rock!",
        category: C::Pluck,
        aliases: &[
            "distortion",
            "power_chord",
            "metal_guitar",
            "distorted_guitar",
        ],
        instrument: synth(Synth {
            unison: Some(2),
            detune: Some(8.0),
            drive: 1.0,
            resonance: 0.2,
            sub: 0.2,
            ..s(Saw, 0.002, 0.5, 0.7, 0.15, 3000.0)
        }),
    },
    Preset {
        id: "harp",
        name: "🌸 Harp",
        hint: "Graceful harp. Lovely for flowing arpeggios.",
        category: C::Pluck,
        aliases: &["lyre"],
        instrument: synth(Synth {
            filter_env: 1.0,
            filter_decay: 0.05,
            fm: 0.4,
            fm_ratio: 2.0,
            fm_decay: 0.1,
            ..s(Triangle, 0.001, 1.5, 0.0, 1.0, 6000.0)
        }),
    },
    Preset {
        id: "koto",
        name: "🍀 Koto",
        hint: "Twangy Japanese zither. Try the pentatonic scale.",
        category: C::Pluck,
        aliases: &["shamisen", "zither", "guzheng"],
        instrument: synth(Synth {
            filter_env: 1.8,
            filter_decay: 0.08,
            pitch_env: 0.3,
            pitch_decay: 0.03,
            unison: Some(1),
            ..s(Saw, 0.001, 0.9, 0.0, 0.4, 1800.0)
        }),
    },
    // ---- Bells & mallets ----
    Preset {
        id: "bell",
        name: "🔔 Bell",
        hint: "Clear, ringing tone. Nice for high sparkly notes.",
        category: C::Mallets,
        aliases: &[],
        instrument: synth(s(Sine, 0.005, 0.9, 0.0, 0.8, 20000.0)),
    },
    Preset {
        id: "music_box",
        name: "🎐 Music Box",
        hint: "Tiny, tinkly music box. Sweet lullabies, high notes.",
        category: C::Mallets,
        aliases: &["musicbox", "toy_piano", "lullaby"],
        instrument: synth(Synth {
            fm: 1.5,
            fm_ratio: 4.0,
            fm_decay: 0.15,
            ..s(Sine, 0.001, 1.2, 0.0, 1.0, 20000.0)
        }),
    },
    Preset {
        id: "marimba",
        name: "🔨 Marimba",
        hint: "Warm wooden mallets. Tropical and playful.",
        category: C::Mallets,
        aliases: &["mallet", "balafon"],
        instrument: synth(Synth {
            fm: 2.5,
            fm_ratio: 4.0,
            fm_decay: 0.03,
            ..s(Sine, 0.001, 0.5, 0.0, 0.35, 20000.0)
        }),
    },
    Preset {
        id: "xylophone",
        name: "♦ Xylophone",
        hint: "Bright, dry and short mallet sound.",
        category: C::Mallets,
        aliases: &["xylo"],
        instrument: synth(Synth {
            fm: 3.0,
            fm_ratio: 3.0,
            fm_decay: 0.02,
            ..s(Sine, 0.001, 0.25, 0.0, 0.2, 20000.0)
        }),
    },
    Preset {
        id: "vibraphone",
        name: "🎐 Vibraphone",
        hint: "Smooth jazz vibes with a gentle shimmer.",
        category: C::Mallets,
        aliases: &["vibes", "vibraphone"],
        instrument: synth(Synth {
            fm: 1.0,
            fm_ratio: 4.0,
            fm_decay: 0.08,
            vibrato: 10.0,
            ..s(Sine, 0.001, 2.0, 0.0, 1.2, 20000.0)
        }),
    },
    Preset {
        id: "glockenspiel",
        name: "✨ Glockenspiel",
        hint: "Sparkly metal bars. Christmas and fairy-tale melodies.",
        category: C::Mallets,
        aliases: &["glock", "celesta"],
        instrument: synth(Synth {
            fm: 2.0,
            fm_ratio: 5.0,
            fm_decay: 0.1,
            ..s(Sine, 0.001, 1.5, 0.0, 1.0, 20000.0)
        }),
    },
    Preset {
        id: "kalimba",
        name: "💧 Kalimba",
        hint: "Soft thumb piano. Calm, cozy melodies.",
        category: C::Mallets,
        aliases: &["thumb_piano", "mbira"],
        instrument: synth(Synth {
            fm: 2.0,
            fm_ratio: 5.4,
            fm_decay: 0.02,
            ..s(Sine, 0.001, 0.8, 0.0, 0.5, 20000.0)
        }),
    },
    Preset {
        id: "tubular_bells",
        name: "🔔 Church Bells",
        hint: "Big, long, clangy bells. Use sparingly for drama.",
        category: C::Mallets,
        aliases: &["chimes", "tubular", "church_bell"],
        instrument: synth(Synth {
            fm: 4.0,
            fm_ratio: 3.5,
            fm_decay: 1.5,
            ..s(Sine, 0.001, 3.0, 0.0, 2.5, 20000.0)
        }),
    },
    Preset {
        id: "steel_drum",
        name: "🌴 Steel Drum",
        hint: "Sunny Caribbean steel pan.",
        category: C::Mallets,
        aliases: &["steel_pan", "caribbean"],
        instrument: synth(Synth {
            fm: 1.8,
            fm_ratio: 2.0,
            fm_decay: 0.25,
            ..s(Sine, 0.002, 0.6, 0.1, 0.4, 20000.0)
        }),
    },
    // ---- Orchestra ----
    Preset {
        id: "strings",
        name: "🎻 Strings",
        hint: "Lush string section. Film-score chords and long notes.",
        category: C::Orchestra,
        aliases: &[
            "violin",
            "orchestra",
            "ensemble",
            "string_ensemble",
            "violins",
        ],
        instrument: synth(Synth {
            unison: Some(4),
            detune: Some(16.0),
            vibrato: 12.0,
            vibrato_rate: 5.5,
            ..s(Saw, 0.25, 0.5, 0.85, 0.6, 3500.0)
        }),
    },
    Preset {
        id: "cello",
        name: "🎻 Cello",
        hint: "Deep, expressive solo string. Slow melodies around C3.",
        category: C::Orchestra,
        aliases: &["viola", "double_bass", "contrabass"],
        instrument: synth(Synth {
            unison: Some(2),
            detune: Some(6.0),
            vibrato: 15.0,
            ..s(Saw, 0.12, 0.4, 0.9, 0.4, 1500.0)
        }),
    },
    Preset {
        id: "pizzicato",
        name: "🎻 Pizzicato",
        hint: "Plucked strings. Sneaky, playful, cartoonish.",
        category: C::Orchestra,
        aliases: &["pizz", "plucked_strings"],
        instrument: synth(Synth {
            unison: Some(2),
            detune: Some(8.0),
            filter_env: 1.0,
            filter_decay: 0.05,
            ..s(Saw, 0.002, 0.25, 0.0, 0.2, 2000.0)
        }),
    },
    Preset {
        id: "brass",
        name: "🎺 Brass Section",
        hint: "Bold, punchy horns. Stabs and fanfares.",
        category: C::Orchestra,
        aliases: &["horns", "brass_section", "trombone"],
        instrument: synth(Synth {
            unison: Some(3),
            detune: Some(10.0),
            filter_env: 2.0,
            filter_decay: 0.15,
            resonance: 0.1,
            vibrato: 6.0,
            ..s(Saw, 0.05, 0.3, 0.8, 0.2, 900.0)
        }),
    },
    Preset {
        id: "trumpet",
        name: "🎺 Trumpet",
        hint: "Bright solo trumpet. Melodies around C4-C5.",
        category: C::Orchestra,
        aliases: &["cornet", "flugelhorn"],
        instrument: synth(Synth {
            unison: Some(1),
            filter_env: 1.8,
            filter_decay: 0.1,
            vibrato: 14.0,
            vibrato_rate: 5.5,
            drive: 0.2,
            ..s(Saw, 0.03, 0.2, 0.85, 0.15, 1400.0)
        }),
    },
    Preset {
        id: "french_horn",
        name: "📯 French Horn",
        hint: "Mellow, heroic horn. Big film themes.",
        category: C::Orchestra,
        aliases: &["horn"],
        instrument: synth(Synth {
            unison: Some(1),
            filter_env: 1.0,
            filter_decay: 0.2,
            vibrato: 6.0,
            ..s(Saw, 0.08, 0.3, 0.9, 0.3, 600.0)
        }),
    },
    Preset {
        id: "saxophone",
        name: "🎷 Saxophone",
        hint: "Breathy, soulful sax. Jazz and smooth melodies.",
        category: C::Orchestra,
        aliases: &["sax", "alto_sax", "tenor_sax"],
        instrument: synth(Synth {
            noise: 0.08,
            resonance: 0.3,
            filter_env: 1.0,
            filter_decay: 0.15,
            vibrato: 18.0,
            drive: 0.3,
            ..s(Square, 0.03, 0.2, 0.85, 0.15, 1800.0)
        }),
    },
    Preset {
        id: "flute",
        name: "🎵 Flute",
        hint: "Airy, sweet flute. High melodies around C5.",
        category: C::Orchestra,
        aliases: &["piccolo", "recorder"],
        instrument: synth(Synth {
            noise: 0.12,
            vibrato: 14.0,
            fm: 0.3,
            fm_ratio: 2.0,
            ..s(Sine, 0.06, 0.2, 0.9, 0.2, 5000.0)
        }),
    },
    Preset {
        id: "pan_flute",
        name: "🎵 Pan Flute",
        hint: "Breathy wooden pipes. Folk and world music.",
        category: C::Orchestra,
        aliases: &["panpipe", "pan_pipes", "shakuhachi"],
        instrument: synth(Synth {
            noise: 0.25,
            vibrato: 8.0,
            ..s(Sine, 0.04, 0.3, 0.7, 0.3, 3000.0)
        }),
    },
    Preset {
        id: "clarinet",
        name: "🎶 Clarinet",
        hint: "Woody, hollow reed. Warm melodies.",
        category: C::Orchestra,
        aliases: &["woodwind", "bassoon"],
        instrument: synth(Synth {
            vibrato: 6.0,
            ..s(Square, 0.04, 0.2, 0.9, 0.15, 1600.0)
        }),
    },
    Preset {
        id: "oboe",
        name: "🎶 Oboe",
        hint: "Nasal, singing reed. Pastoral, slightly sad melodies.",
        category: C::Orchestra,
        aliases: &["english_horn"],
        instrument: synth(Synth {
            resonance: 0.2,
            vibrato: 12.0,
            ..s(Pulse, 0.04, 0.2, 0.9, 0.15, 2500.0)
        }),
    },
    // ---- Retro ----
    Preset {
        id: "chiptune",
        name: "👾 8-bit",
        hint: "Retro video game sound.",
        category: C::Retro,
        aliases: &["8bit", "chip", "retro"],
        instrument: synth(s(Square, 0.002, 0.1, 0.8, 0.05, 12000.0)),
    },
    Preset {
        id: "chip_pulse",
        name: "👾 NES Pulse",
        hint: "Thin console-style pulse. Fast arpeggios and melodies.",
        category: C::Retro,
        aliases: &["nes", "gameboy", "famicom"],
        instrument: synth(s(Pulse, 0.002, 0.1, 0.7, 0.05, 12000.0)),
    },
    Preset {
        id: "chip_bass",
        name: "🕹 NES Triangle",
        hint: "The soft console triangle, usually the bass line.",
        category: C::Retro,
        aliases: &["nes_bass", "chip_triangle"],
        instrument: synth(s(Triangle, 0.002, 0.05, 1.0, 0.03, 20000.0)),
    },
    Preset {
        id: "game_lead",
        name: "🎮 Game Lead",
        hint: "Wobbly arcade hero melody.",
        category: C::Retro,
        aliases: &["arcade", "game"],
        instrument: synth(Synth {
            vibrato: 25.0,
            vibrato_rate: 7.0,
            ..s(Square, 0.002, 0.1, 0.8, 0.08, 9000.0)
        }),
    },
    Preset {
        id: "retro_synth",
        name: "🌆 Synthwave",
        hint: "80s analog polysynth. Neon nights and car chases.",
        category: C::Retro,
        aliases: &["80s", "retrowave", "outrun", "polysynth"],
        instrument: synth(Synth {
            unison: Some(3),
            detune: Some(12.0),
            resonance: 0.3,
            vibrato: 4.0,
            vibrato_rate: 0.5,
            ..s(Saw, 0.01, 0.4, 0.7, 0.4, 2000.0)
        }),
    },
    // ---- FX ----
    Preset {
        id: "riser",
        name: "🚀 Riser",
        hint: "Swells up into the note. Put one long note before a drop.",
        category: C::Fx,
        aliases: &["uplifter", "buildup", "build_up"],
        instrument: synth(Synth {
            unison: Some(3),
            noise: 0.4,
            pitch_env: -24.0,
            pitch_decay: 1.0,
            ..s(Saw, 1.5, 0.5, 1.0, 0.5, 3000.0)
        }),
    },
    Preset {
        id: "zap",
        name: "💫 Zap",
        hint: "Laser 'pew'. Short notes, any pitch.",
        category: C::Fx,
        aliases: &["laser", "pew"],
        instrument: synth(Synth {
            pitch_env: 36.0,
            pitch_decay: 0.03,
            ..s(Square, 0.001, 0.2, 0.0, 0.1, 8000.0)
        }),
    },
    Preset {
        id: "boom",
        name: "💣 Boom",
        hint: "Cinematic impact. One low note on the big moment.",
        category: C::Fx,
        aliases: &["impact", "hit", "explosion"],
        instrument: synth(Synth {
            pitch_env: 24.0,
            pitch_decay: 0.15,
            noise: 0.3,
            drive: 0.6,
            ..s(Sine, 0.001, 2.0, 0.0, 1.0, 3000.0)
        }),
    },
    Preset {
        id: "wind",
        name: "💨 Wind",
        hint: "Whooshing noise. Pitch doesn't matter, note length does.",
        category: C::Fx,
        aliases: &["noise", "whoosh", "white_noise"],
        instrument: synth(Synth {
            resonance: 0.8,
            drive: 0.7,
            ..s(Noise, 1.0, 0.5, 1.0, 1.5, 1200.0)
        }),
    },
    Preset {
        id: "theremin",
        name: "👻 Theremin",
        hint: "Spooky wobbly sci-fi tone.",
        category: C::Fx,
        aliases: &["spooky", "scifi", "ghost"],
        instrument: synth(Synth {
            vibrato: 30.0,
            vibrato_rate: 6.0,
            ..s(Sine, 0.2, 0.2, 1.0, 0.5, 20000.0)
        }),
    },
    Preset {
        id: "magic",
        name: "🔮 Magic",
        hint: "Glittering fairy-dust sparkle. High notes.",
        category: C::Fx,
        aliases: &["sparkle", "fairy", "twinkle"],
        instrument: synth(Synth {
            fm: 5.0,
            fm_ratio: 7.1,
            fm_decay: 0.4,
            ..s(Sine, 0.001, 2.0, 0.0, 2.0, 20000.0)
        }),
    },
    // ---- Drums ----
    Preset {
        id: "drum_kit",
        name: "💥 Drum Kit",
        hint: "Kick, snare, hi-hats and more. Each row is a different drum.",
        category: C::Drums,
        aliases: &["drums", "drum", "kit", "percussion", "standard_kit"],
        instrument: kit(DrumKit::Standard),
    },
    Preset {
        id: "drums_808",
        name: "💣 808 Drums",
        hint: "The legendary hip-hop drum machine: boomy kick, snappy snare, cowbell.",
        category: C::Drums,
        aliases: &["808_kit", "808_drums", "tr808", "hiphop_drums"],
        instrument: kit(DrumKit::Tr808),
    },
    Preset {
        id: "drums_909",
        name: "🎧 909 Drums",
        hint: "Punchy house and techno drum machine. Four-on-the-floor!",
        category: C::Drums,
        aliases: &["909", "909_kit", "tr909", "house_drums", "techno_drums"],
        instrument: kit(DrumKit::Tr909),
    },
    Preset {
        id: "drums_acoustic",
        name: "🎪 Acoustic Drums",
        hint: "A real-sounding drum set for rock, pop and jazz.",
        category: C::Drums,
        aliases: &["acoustic_kit", "rock_drums", "real_drums", "jazz_drums"],
        instrument: kit(DrumKit::Acoustic),
    },
    Preset {
        id: "drums_lofi",
        name: "📀 Lo-fi Drums",
        hint: "Dusty, crunchy drums for chill beats.",
        category: C::Drums,
        aliases: &["lofi_kit", "lofi_drums", "dusty_drums", "boom_bap"],
        instrument: kit(DrumKit::Lofi),
    },
    Preset {
        id: "drums_trap",
        name: "🔥 Trap Drums",
        hint: "Long sub kick and crisp hats. Try fast hi-hat rolls.",
        category: C::Drums,
        aliases: &["trap_kit", "trap_drums", "trap"],
        instrument: kit(DrumKit::Trap),
    },
    Preset {
        id: "drums_electro",
        name: "⚡ Electro Drums",
        hint: "Hard, gritty, distorted drum machine.",
        category: C::Drums,
        aliases: &["electro_kit", "electro", "industrial"],
        instrument: kit(DrumKit::Electro),
    },
];

/// Every preset, grouped by category in display order.
pub fn all() -> &'static [Preset] {
    PRESETS
}

/// The preset that exactly matches `inst`, if any.
pub fn of(inst: &Instrument) -> Option<&'static Preset> {
    PRESETS.iter().find(|p| &p.instrument == inst)
}

/// Name of the preset that exactly matches `inst`, if any.
pub fn name_of(inst: &Instrument) -> Option<&'static str> {
    of(inst).map(|p| p.name)
}

/// The preset name without its emoji, for naming new tracks.
pub fn plain_name(name: &str) -> &str {
    name.split_once(' ').map(|(_, rest)| rest).unwrap_or(name)
}

fn norm(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

/// Look a preset up by id, display name or alias, forgivingly: `"Soft
/// Keys"`, `"soft-keys"`, `"8-bit"`, `"drums"` and `"rhodes"` all work.
/// Failing an exact match, the longest preset word found inside the query
/// wins, so `"Bass 2"` finds the bass and `"Lead Guitar"` the guitar.
pub fn find(query: &str) -> Option<&'static Preset> {
    let q = norm(query);
    if q.is_empty() {
        return None;
    }
    let keys = |p: &'static Preset| {
        [p.id, plain_name(p.name)]
            .into_iter()
            .chain(p.aliases.iter().copied())
            .map(norm)
    };
    if let Some(p) = PRESETS.iter().find(|p| keys(p).any(|k| k == q)) {
        return Some(p);
    }
    PRESETS
        .iter()
        .flat_map(|p| keys(p).map(move |k| (k, p)))
        .filter(|(k, _)| k.len() >= 3 && q.contains(k.as_str()))
        // Longest match wins; on a tie the earlier preset does.
        .fold(
            None,
            |best: Option<(usize, &'static Preset)>, (k, p)| match best {
                Some((len, _)) if len >= k.len() => best,
                _ => Some((k.len(), p)),
            },
        )
        .map(|(_, p)| p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_and_names_are_unique_and_findable() {
        let mut ids = std::collections::HashSet::new();
        for p in all() {
            assert!(ids.insert(p.id), "duplicate id {}", p.id);
            assert_eq!(find(p.id).map(|f| f.id), Some(p.id));
            assert_eq!(
                find(plain_name(p.name)).map(|f| f.id),
                Some(p.id),
                "{}",
                p.name
            );
            for a in p.aliases {
                assert_eq!(find(a).map(|f| f.id), Some(p.id), "alias {a}");
            }
        }
        for (i, a) in all().iter().enumerate() {
            for b in &all()[i + 1..] {
                assert_ne!(a.instrument, b.instrument, "{} == {}", a.id, b.id);
            }
        }
        assert!(all().len() >= 60);
    }

    #[test]
    fn forgiving_lookup() {
        let id = |q: &str| find(q).map(|p| p.id);
        assert_eq!(id("Rhodes piano"), Some("e_piano"));
        assert_eq!(id("Bass 2"), Some("bass"));
        assert_eq!(id("Lead Guitar"), Some("nylon_guitar"));
        assert_eq!(id("8-bit"), Some("chiptune"));
        assert_eq!(id("808"), Some("bass_808"));
        assert_eq!(id("808 drums"), Some("drums_808"));
        assert_eq!(id("Drums"), Some("drum_kit"));
        assert_eq!(id("kazoo"), None);
    }

    #[test]
    fn categories_are_contiguous_and_complete() {
        for c in Category::ALL {
            assert!(Preset::in_category(c).count() >= 5, "{c:?}");
        }
        let order: Vec<_> = all().iter().map(|p| p.category).collect();
        let mut seen = Vec::new();
        for c in order {
            if seen.last() != Some(&c) {
                assert!(!seen.contains(&c), "{c:?} is split up");
                seen.push(c);
            }
        }
    }
}
