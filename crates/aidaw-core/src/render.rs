//! Sequencer + built-in instruments. The same code drives real-time playback
//! (in `aidaw-engine`) and offline rendering to WAV, so what the AI renders
//! is exactly what the human hears in the GUI.
//!
//! Sound-quality and safety notes:
//! - Saw/square/pulse oscillators are band-limited with polyBLEP; saws use
//!   two slightly detuned oscillators for a fuller tone (up to five with
//!   `unison`), and the synth filter is a 12 dB/oct low-pass (a resonant
//!   state-variable filter once resonance or the filter envelope is on).
//! - Drum kits share one set of drum models; each kit retunes and colours
//!   them (pitch, decay, drive, bit-crush, darkening).
//! - Voices never start or stop abruptly: stolen voices, choked hi-hats and
//!   voices cut by a seek/stop fade out over a few milliseconds, and drum
//!   tails are faded before the voice is freed.
//! - Playback that starts in the middle of a held note (after a seek or a
//!   pause) picks that note up part-way through, fading it in, instead of
//!   staying silent until the next note starts.
//! - Track gain, pan and mute/solo changes are ramped per block (no zipper
//!   noise or clicks).
//! - Tracks send to one shared stereo reverb (Freeverb-style: damped combs
//!   into all-passes) with a high-passed input, so basses don't muddy it.
//!   It sleeps, costing nothing, once its tail has died away.
//! - The master bus runs through a peak limiter and a soft clipper, so
//!   stacking many loud tracks never hard-clips or produces NaNs: the output
//!   is always strictly inside ±1.0.
//! - Tracks can play hosted plugins and run effect plugins (see
//!   [`crate::rack`]): plugin instruments get their notes as sample-accurate
//!   note-on/off events, and a track with plugins renders into its own bus,
//!   through its effects, before its volume, pan and reverb send.
//! - [`Sequencer::process`] does not allocate (state buffers are reserved up
//!   front), so it is safe to call from a real-time audio callback.
//! - Each block looks up the notes that start in it with a binary search
//!   (clips keep their notes sorted by start, see [`playable`]), so a big
//!   song costs no more per block than a small one.

use std::borrow::Cow;
use std::f32::consts::{PI, TAU};
use std::path::Path;

use anyhow::{Context, Result};

use crate::model::{DrumKit, Instrument, Project, Reverb, Synth, Waveform};
use crate::pitch;
use crate::rack::{NoteEvent, Rack, Transport};

/// Voices above this count start stealing (with a short fade).
const MAX_VOICES: usize = 96;
/// Absolute cap including voices that are still fading out after a steal.
const HARD_VOICE_CAP: usize = MAX_VOICES + 32;
/// Track gain state is pre-reserved for this many tracks (more still work,
/// but the first block after adding them allocates once).
const RESERVED_TRACKS: usize = 64;
/// Fade time used when a voice is stolen, choked or cut by a seek.
const FADE_SECONDS: f32 = 0.005;
/// Fade-in of a note picked up part-way through (see [`Sequencer::seek`]).
const CHASE_FADE_SECONDS: f32 = 0.02;
/// Drum voices fade over this long before their length is reached.
const DRUM_TAIL_SECONDS: f32 = 0.03;
/// Master limiter ceiling (about -1 dBFS) and release time.
const LIMIT_THRESHOLD: f32 = 0.89;
const LIMIT_RELEASE_SECONDS: f32 = 0.15;
/// Soft clipper: linear below the knee, approaches (never reaches) 1.0.
const CLIP_KNEE: f32 = 0.8;
/// Frequencies of the six square oscillators of a TR-808 style cymbal.
const METAL_FREQS: [f32; 6] = [205.3, 304.4, 369.6, 522.7, 540.0, 800.0];
/// Detune ratio of the second saw oscillator (about 7 cents).
const SAW_DETUNE: f32 = 1.004;

/// Upper limit of the synth's unison stack.
const MAX_UNISON: usize = 5;

/// Longest stretch rendered in one pass; longer blocks are split, so the
/// reverb send buffer can be allocated once, up front.
const MAX_BLOCK: usize = crate::rack::MAX_BLOCK;
/// Plugin-instrument notes sounding or waiting at once; more are dropped.
const MAX_PLUGIN_NOTES: usize = 1024;
/// Note events handed to one plugin in one block; more wait a block.
const MAX_EVENTS: usize = 512;
/// Freeverb's comb and all-pass delays in samples at 44.1 kHz (scaled to
/// the actual sample rate). The right side is `STEREO_SPREAD` longer.
const COMB_TUNING: [usize; 8] = [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617];
const ALLPASS_TUNING: [usize; 4] = [556, 441, 341, 225];
const STEREO_SPREAD: usize = 23;
/// Gain into the comb bank and out of the reverb.
const REVERB_IN: f32 = 0.03;
const REVERB_OUT: f32 = 1.0;
/// The reverb input is high-passed here so basses and kicks don't boom.
const REVERB_HP_HZ: f32 = 150.0;
/// Below this output level the reverb tail counts as gone.
const REVERB_SILENCE: f32 = 1e-5;

#[derive(Clone, Copy, PartialEq)]
enum Drum {
    Kick,
    Snare,
    Clap,
    ClosedHat,
    OpenHat,
    Tom(f32),
    Crash,
    Splash,
    Ride,
    RideBell,
    Rim,
    Cowbell,
    Tambourine,
    Shaker,
    /// Bongos, congas and timbales: (pitch in Hz, decay in seconds).
    Hand(f32, f32),
    /// Claves and wood blocks: (pitch in Hz, decay in seconds).
    Wood(f32, f32),
    Agogo(f32),
    /// The orchestral triangle: decay in seconds (muted or open).
    Triangle(f32),
}

impl Drum {
    fn from_note(p: u8, kit: &Kit) -> Drum {
        match p {
            35 | 36 => Drum::Kick,
            37 => Drum::Rim,
            38 | 40 => Drum::Snare,
            39 => Drum::Clap,
            42 | 44 => Drum::ClosedHat,
            46 => Drum::OpenHat,
            49 | 52 | 57 => Drum::Crash,
            55 => Drum::Splash,
            51 | 59 => Drum::Ride,
            53 => Drum::RideBell,
            41 | 43 => Drum::Tom(80.0 * kit.tom_mul),
            45 | 47 => Drum::Tom(110.0 * kit.tom_mul),
            48 | 50 => Drum::Tom(150.0 * kit.tom_mul),
            54 => Drum::Tambourine,
            56 => Drum::Cowbell,
            60 => Drum::Hand(420.0, 0.07),
            61 => Drum::Hand(300.0, 0.09),
            62 => Drum::Hand(340.0, 0.04),
            63 => Drum::Hand(330.0, 0.16),
            64 => Drum::Hand(220.0, 0.2),
            65 => Drum::Hand(560.0, 0.13),
            66 => Drum::Hand(390.0, 0.16),
            67 => Drum::Agogo(930.0),
            68 => Drum::Agogo(700.0),
            69 | 70 | 82 => Drum::Shaker,
            75 => Drum::Wood(2500.0, 0.025),
            76 => Drum::Wood(1150.0, 0.035),
            77 => Drum::Wood(850.0, 0.04),
            80 => Drum::Triangle(0.1),
            81 => Drum::Triangle(0.9),
            p => Drum::Tom(pitch::frequency(p as f32).clamp(40.0, 400.0)),
        }
    }

    /// Length in seconds after which the voice is freed.
    fn length(self, kit: &Kit) -> f32 {
        let cymbal = kit.cymbal_decay / 0.6;
        match self {
            Drum::Kick => kit.kick_len,
            Drum::Snare | Drum::Clap => kit.snare_len,
            Drum::ClosedHat => (kit.hat_decay * 5.0).max(0.1),
            Drum::OpenHat => (kit.open_decay * 2.7).max(0.3),
            Drum::Tom(_) => (kit.tom_decay * 3.0).max(0.3),
            Drum::Crash => 1.8 * cymbal,
            Drum::Splash => 0.9 * cymbal,
            Drum::Ride => 2.4 * cymbal,
            Drum::RideBell => 1.6 * cymbal,
            Drum::Rim => 0.12,
            Drum::Cowbell | Drum::Agogo(_) => 0.5,
            Drum::Tambourine => 0.45,
            Drum::Shaker => 0.22,
            Drum::Hand(_, d) => (d * 5.0 + 0.05).min(1.2),
            Drum::Wood(_, d) => d * 6.0 + 0.02,
            Drum::Triangle(d) => d * 5.0 + 0.05,
        }
    }

    /// (high-pass, low-pass) corner frequencies of the noise/metal path.
    fn filter_hz(self, kit: &Kit) -> (f32, f32) {
        match self {
            Drum::Kick => (2_000.0, 8_000.0),
            Drum::Snare => (kit.snare_hp, kit.snare_lp),
            Drum::Clap => (800.0, 4_000.0),
            Drum::ClosedHat | Drum::OpenHat => (kit.hat_hp, kit.hat_lp),
            Drum::Crash | Drum::Splash => (4_000.0, 14_000.0),
            Drum::Ride => (5_000.0, 13_000.0),
            Drum::RideBell => (2_500.0, 12_000.0),
            Drum::Tom(_) | Drum::Hand(..) => (1_000.0, 5_000.0),
            Drum::Rim => (900.0, 6_000.0),
            Drum::Cowbell => (450.0, 4_500.0),
            Drum::Tambourine => (5_500.0, 15_000.0),
            Drum::Shaker => (4_500.0, 13_000.0),
            Drum::Wood(..) | Drum::Agogo(_) | Drum::Triangle(_) => (1_000.0, 10_000.0),
        }
    }
}

/// The character of a drum kit: every kit plays the same drums, built from
/// the same parts, but tuned and coloured differently.
struct Kit {
    /// Kick: final pitch, extra pitch at the hit and how fast it falls.
    kick_base: f32,
    kick_sweep: f32,
    kick_sweep_time: f32,
    kick_decay: f32,
    kick_len: f32,
    /// Saturation of the kick body (higher = harder, squarer).
    kick_drive: f32,
    /// Level of the beater click.
    kick_click: f32,
    /// Snare: drum-head pitch and level, wire decay and filter.
    snare_tone: f32,
    snare_tone_mix: f32,
    snare_decay: f32,
    snare_hp: f32,
    snare_lp: f32,
    snare_len: f32,
    clap_decay: f32,
    /// Hi-hats: closed and open decay, filter, and how much of the metallic
    /// oscillator bank is mixed in (vs. plain noise).
    hat_decay: f32,
    open_decay: f32,
    hat_hp: f32,
    hat_lp: f32,
    metal: f32,
    tom_mul: f32,
    tom_decay: f32,
    cymbal_decay: f32,
    /// Bit-crusher levels (0 = off): grit for lo-fi and electro kits.
    crush: f32,
    /// Final low-pass in Hz (at or above 20 kHz = off): dusty, dark kits.
    dark: f32,
    gain: f32,
}

static KIT_STANDARD: Kit = Kit {
    kick_base: 48.0,
    kick_sweep: 150.0,
    kick_sweep_time: 0.03,
    kick_decay: 0.22,
    kick_len: 0.6,
    kick_drive: 1.6,
    kick_click: 0.4,
    snare_tone: 180.0,
    snare_tone_mix: 0.25,
    snare_decay: 0.12,
    snare_hp: 1_200.0,
    snare_lp: 9_000.0,
    snare_len: 0.4,
    clap_decay: 0.12,
    hat_decay: 0.03,
    open_decay: 0.22,
    hat_hp: 6_500.0,
    hat_lp: 13_000.0,
    metal: 1.0,
    tom_mul: 1.0,
    tom_decay: 0.2,
    cymbal_decay: 0.6,
    crush: 0.0,
    dark: 20_000.0,
    gain: 1.0,
};

static KIT_808: Kit = Kit {
    kick_base: 49.0,
    kick_sweep: 80.0,
    kick_sweep_time: 0.05,
    kick_decay: 0.45,
    kick_len: 1.3,
    kick_drive: 1.2,
    kick_click: 0.12,
    snare_tone: 238.0,
    snare_tone_mix: 0.45,
    snare_decay: 0.09,
    snare_hp: 1_800.0,
    snare_lp: 8_000.0,
    snare_len: 0.35,
    clap_decay: 0.15,
    hat_decay: 0.025,
    open_decay: 0.3,
    hat_hp: 7_000.0,
    hat_lp: 14_000.0,
    metal: 1.5,
    tom_mul: 0.9,
    tom_decay: 0.35,
    cymbal_decay: 0.9,
    crush: 0.0,
    dark: 20_000.0,
    gain: 1.0,
};

static KIT_909: Kit = Kit {
    kick_base: 54.0,
    kick_sweep: 230.0,
    kick_sweep_time: 0.014,
    kick_decay: 0.18,
    kick_len: 0.5,
    kick_drive: 2.4,
    kick_click: 0.8,
    snare_tone: 200.0,
    snare_tone_mix: 0.3,
    snare_decay: 0.16,
    snare_hp: 900.0,
    snare_lp: 11_000.0,
    snare_len: 0.45,
    clap_decay: 0.13,
    hat_decay: 0.04,
    open_decay: 0.26,
    hat_hp: 8_000.0,
    hat_lp: 15_000.0,
    metal: 0.6,
    tom_mul: 1.05,
    tom_decay: 0.22,
    cymbal_decay: 0.8,
    crush: 0.0,
    dark: 20_000.0,
    gain: 1.0,
};

static KIT_ACOUSTIC: Kit = Kit {
    kick_base: 58.0,
    kick_sweep: 60.0,
    kick_sweep_time: 0.02,
    kick_decay: 0.14,
    kick_len: 0.45,
    kick_drive: 1.3,
    kick_click: 0.7,
    snare_tone: 190.0,
    snare_tone_mix: 0.4,
    snare_decay: 0.18,
    snare_hp: 700.0,
    snare_lp: 7_000.0,
    snare_len: 0.5,
    clap_decay: 0.1,
    hat_decay: 0.05,
    open_decay: 0.35,
    hat_hp: 5_000.0,
    hat_lp: 11_000.0,
    metal: 0.8,
    tom_mul: 1.1,
    tom_decay: 0.35,
    cymbal_decay: 1.1,
    crush: 0.0,
    dark: 12_000.0,
    gain: 1.05,
};

static KIT_LOFI: Kit = Kit {
    kick_base: 50.0,
    kick_sweep: 110.0,
    kick_sweep_time: 0.03,
    kick_decay: 0.2,
    kick_len: 0.55,
    kick_drive: 1.5,
    kick_click: 0.3,
    snare_tone: 200.0,
    snare_tone_mix: 0.35,
    snare_decay: 0.13,
    snare_hp: 900.0,
    snare_lp: 6_000.0,
    snare_len: 0.4,
    clap_decay: 0.12,
    hat_decay: 0.035,
    open_decay: 0.2,
    hat_hp: 5_000.0,
    hat_lp: 10_000.0,
    metal: 0.7,
    tom_mul: 1.0,
    tom_decay: 0.25,
    cymbal_decay: 0.6,
    crush: 24.0,
    dark: 3_800.0,
    gain: 1.15,
};

static KIT_TRAP: Kit = Kit {
    kick_base: 42.0,
    kick_sweep: 120.0,
    kick_sweep_time: 0.04,
    kick_decay: 0.8,
    kick_len: 1.8,
    kick_drive: 1.5,
    kick_click: 0.3,
    snare_tone: 220.0,
    snare_tone_mix: 0.3,
    snare_decay: 0.1,
    snare_hp: 1_500.0,
    snare_lp: 10_000.0,
    snare_len: 0.35,
    clap_decay: 0.14,
    hat_decay: 0.018,
    open_decay: 0.25,
    hat_hp: 8_500.0,
    hat_lp: 16_000.0,
    metal: 1.2,
    tom_mul: 0.85,
    tom_decay: 0.4,
    cymbal_decay: 0.9,
    crush: 0.0,
    dark: 20_000.0,
    gain: 1.0,
};

static KIT_ELECTRO: Kit = Kit {
    kick_base: 52.0,
    kick_sweep: 300.0,
    kick_sweep_time: 0.01,
    kick_decay: 0.2,
    kick_len: 0.5,
    kick_drive: 4.0,
    kick_click: 0.7,
    snare_tone: 160.0,
    snare_tone_mix: 0.4,
    snare_decay: 0.14,
    snare_hp: 1_000.0,
    snare_lp: 9_000.0,
    snare_len: 0.4,
    clap_decay: 0.16,
    hat_decay: 0.03,
    open_decay: 0.2,
    hat_hp: 6_000.0,
    hat_lp: 12_000.0,
    metal: 1.0,
    tom_mul: 1.2,
    tom_decay: 0.18,
    cymbal_decay: 0.6,
    crush: 48.0,
    dark: 20_000.0,
    gain: 0.9,
};

fn kit_params(kit: DrumKit) -> &'static Kit {
    match kit {
        DrumKit::Standard => &KIT_STANDARD,
        DrumKit::Tr808 => &KIT_808,
        DrumKit::Tr909 => &KIT_909,
        DrumKit::Acoustic => &KIT_ACOUSTIC,
        DrumKit::Lofi => &KIT_LOFI,
        DrumKit::Trap => &KIT_TRAP,
        DrumKit::Electro => &KIT_ELECTRO,
    }
}

/// A synth patch prepared for playback (clamped, with filter coefficients).
#[derive(Clone, Copy)]
struct SynthVoice {
    waveform: Waveform,
    attack: f32,
    decay: f32,
    sustain: f32,
    release: f32,
    /// The original two-pole filter (no resonance, no envelope).
    lp_coef: f32,
    /// Resonant state-variable filter, used when resonance or the filter
    /// envelope is on. `k` is its damping; `g` is precomputed when the
    /// cutoff doesn't move.
    svf: bool,
    k: f32,
    g: f32,
    cutoff: f32,
    filter_env: f32,
    filter_decay: f32,
    unison: usize,
    ratios: [f32; MAX_UNISON],
    unison_gain: f32,
    sub: f32,
    noise: f32,
    fm: f32,
    fm_ratio: f32,
    fm_decay: f32,
    vibrato: f32,
    vibrato_rate: f32,
    pitch_env: f32,
    pitch_decay: f32,
    drive: f32,
}

impl SynthVoice {
    fn new(p: &Synth, sr: f32) -> Self {
        let cutoff = finite_or(p.cutoff, 20_000.0).clamp(20.0, 20_000.0);
        let resonance = finite_or(p.resonance, 0.0).clamp(0.0, 1.0);
        let filter_env = finite_or(p.filter_env, 0.0).clamp(0.0, 8.0);
        // Two cascaded one-poles are -3 dB at ~0.644x their corner,
        // so shift the corner up to keep the cutoff where users set it.
        let lp_coef = one_pole_coef(cutoff / 0.6436, sr);
        // The SVF at zero resonance is that same two-pole response; as the
        // resonance rises the peak should sit right at the cutoff.
        let svf_cutoff = cutoff * (1.0 / 0.6436f32).powf(1.0 - resonance.min(0.5) * 2.0);
        let k = 2.0 - 1.9 * resonance;
        let auto_unison = p.unison.is_none() && p.detune.is_none() && p.waveform == Waveform::Saw;
        let detune = finite_or(p.detune.unwrap_or(14.0), 0.0).clamp(0.0, 100.0);
        let unison = match p.unison {
            Some(n) => (n as usize).clamp(1, MAX_UNISON),
            None if p.waveform == Waveform::Saw => 2,
            None if detune > 0.0 && p.detune.is_some() => 2,
            None => 1,
        };
        let mut ratios = [1.0; MAX_UNISON];
        if auto_unison {
            ratios[1] = SAW_DETUNE;
        } else if unison > 1 {
            for (i, r) in ratios.iter_mut().take(unison).enumerate() {
                let cents = -detune / 2.0 + detune * i as f32 / (unison - 1) as f32;
                *r = (cents / 1200.0).exp2();
            }
        }
        let unison_gain = match unison {
            1 => 1.0,
            2 => 0.62,
            n => 0.88 / (n as f32).sqrt(),
        };
        SynthVoice {
            waveform: p.waveform,
            attack: finite_or(p.attack, 0.0).max(0.001),
            decay: finite_or(p.decay, 0.0).max(0.001),
            sustain: finite_or(p.sustain, 1.0).clamp(0.0, 1.0),
            release: finite_or(p.release, 0.0).max(0.005),
            lp_coef,
            svf: resonance > 0.0 || filter_env > 0.0,
            k,
            g: svf_g(svf_cutoff, sr),
            cutoff: svf_cutoff,
            filter_env,
            filter_decay: finite_or(p.filter_decay, 0.3).max(0.001),
            unison,
            ratios,
            unison_gain,
            sub: finite_or(p.sub, 0.0).clamp(0.0, 1.0),
            noise: finite_or(p.noise, 0.0).clamp(0.0, 1.0),
            fm: finite_or(p.fm, 0.0).clamp(0.0, 10.0),
            fm_ratio: finite_or(p.fm_ratio, 1.0).clamp(0.0, 16.0),
            fm_decay: finite_or(p.fm_decay, 0.0).max(0.0),
            vibrato: finite_or(p.vibrato, 0.0).clamp(0.0, 200.0),
            vibrato_rate: finite_or(p.vibrato_rate, 5.0).clamp(0.0, 20.0),
            pitch_env: finite_or(p.pitch_env, 0.0).clamp(-48.0, 48.0),
            pitch_decay: finite_or(p.pitch_decay, 0.05).max(0.001),
            drive: finite_or(p.drive, 0.0).clamp(0.0, 1.0),
        }
    }
}

/// `g` coefficient of the TPT state-variable filter.
fn svf_g(hz: f32, sr: f32) -> f32 {
    (PI * hz.clamp(10.0, 0.45 * sr) / sr).tan()
}

#[derive(Clone, Copy)]
enum Kind {
    Synth(SynthVoice),
    Drum {
        drum: Drum,
        kit: &'static Kit,
        hp_coef: f32,
        lp_coef: f32,
        /// One-pole of the kit's final darkening filter (1.0 = off).
        dark_coef: f32,
    },
}

#[derive(Clone, Copy)]
struct Voice {
    track: usize,
    kind: Kind,
    freq: f32,
    phase: f32,
    /// Synths: unison copies (0..4), sub (4) and FM modulator (5).
    /// Drums: extra tone oscillators, or the metallic bank (cymbals).
    phases: [f32; 6],
    vel: f32,
    /// Frames to wait before the voice starts (sample-accurate note starts).
    delay: u32,
    /// Frames the note is held; release starts when this reaches zero.
    gate: u64,
    age: u64,
    /// Envelope level when the release began.
    release_from: f32,
    released_at: Option<u64>,
    env: f32,
    /// Filter states (synths: two low-pass poles or the SVF; drums: band
    /// low-pass, darkening low-pass, two high-pass poles).
    z: [f32; 4],
    done: bool,
    /// A preview note that sounds until [`Sequencer::release_held`].
    held: bool,
    /// Gain of the anti-click fade-out; 1.0 unless `fading`.
    fade: f32,
    fading: bool,
    /// Gain of the fade-in of a note picked up part-way through; else 1.0.
    fade_in: f32,
    /// Start fading this many frames into the current block (hi-hat choke).
    fade_at: Option<u32>,
}

#[derive(Clone, Copy, Default)]
struct TrackGain {
    /// Gains reached at the end of the previous block (left, right, reverb send).
    cur: [f32; 3],
}

/// A note on a plugin instrument. The plugin makes the sound; this only
/// keeps time, to send the note-on and the note-off at the right frames.
#[derive(Clone, Copy)]
struct PluginNote {
    /// The track the note was played on; its plugin is looked up from this
    /// once, then the note stays with that plugin (`slot`).
    track: usize,
    slot: Option<u64>,
    pitch: u8,
    velocity: f32,
    id: i32,
    /// Frames to wait before the note starts.
    delay: u32,
    started: bool,
    /// Frames the note is held once started.
    gate: u64,
    age: u64,
    /// A preview that sounds until [`Sequencer::release_held`].
    held: bool,
    /// End it at the start of the next block (after a seek or a stop).
    cut: bool,
    done: bool,
    /// Its plugin was found this block (else the plugin is gone).
    seen: bool,
}

impl PluginNote {
    /// Advance by one block of `frames`, writing the events it makes.
    fn step(&mut self, frames: u32, events: &mut Vec<NoteEvent>) {
        let room = events.len() < events.capacity();
        let off = |offset| NoteEvent {
            offset,
            pitch: self.pitch,
            velocity: 0.0,
            on: false,
            id: self.id,
        };
        if self.cut {
            if self.started {
                if !room {
                    return; // try again next block
                }
                events.push(off(0));
            }
            self.done = true;
            return;
        }
        let mut at = 0;
        if !self.started {
            if self.delay >= frames {
                self.delay -= frames;
                return;
            }
            if !room {
                self.done = true; // never started, so nothing hangs
                return;
            }
            at = self.delay;
            self.delay = 0;
            self.started = true;
            events.push(NoteEvent {
                offset: at,
                pitch: self.pitch,
                velocity: self.velocity,
                on: true,
                id: self.id,
            });
        }
        let left = u64::from(frames - at);
        let remain = self.gate.saturating_sub(self.age);
        if remain < left {
            if events.len() < events.capacity() {
                events.push(off(at + remain as u32));
                self.done = true;
            } else {
                self.cut = true;
            }
        } else {
            self.age += left;
        }
    }
}

/// Advance `voice` by one frame: its sample (0 while it waits to start), or
/// `None` once it is done.
#[inline]
fn tick(
    voice: &mut Voice,
    sr: f32,
    rng: &mut u32,
    fade_step: f32,
    fade_in_step: f32,
) -> Option<f32> {
    if voice.done {
        return None;
    }
    if let Some(n) = voice.fade_at {
        if n == 0 {
            voice.fading = true;
            voice.fade_at = None;
        } else {
            voice.fade_at = Some(n - 1);
        }
    }
    if voice.delay > 0 {
        voice.delay -= 1;
        return Some(0.0);
    }
    let mut s = voice.next(sr, rng);
    if voice.fade_in < 1.0 {
        voice.fade_in = (voice.fade_in + fade_in_step).min(1.0);
        s *= voice.fade_in;
    }
    if voice.fading {
        voice.fade -= fade_step;
        if voice.fade <= 0.0 {
            voice.fade = 0.0;
            voice.done = true;
        }
        s *= voice.fade;
    }
    Some(s)
}

pub struct Sequencer {
    sample_rate: f32,
    /// Playhead in beats.
    position: f64,
    voices: Vec<Voice>,
    noise: u32,
    /// Smoothed per-track gains, so gain/pan/mute changes don't click.
    gains: Vec<TrackGain>,
    /// Master limiter envelope.
    limit_env: f32,
    /// Highest limiter envelope since [`Self::take_limiter_reduction_db`].
    limit_peak: f32,
    limit_release: f32,
    /// Set by [`Self::seek`]: the next block also starts the notes held
    /// across the new playhead position.
    chase: bool,
    /// What the tracks send to the reverb in the current block (mono).
    send: Box<[f32]>,
    room: Room,
    /// Notes playing on plugin instruments.
    plugin_notes: Vec<PluginNote>,
    next_note_id: i32,
    /// The events of one track in the current block (capacity reserved).
    events: Vec<NoteEvent>,
    /// Frames rendered so far, for [`Transport::frames`].
    clock: u64,
    /// The region [`Self::process_looping`] repeats, for [`Transport::cycle`].
    cycle: Option<(f64, f64)>,
    /// Clip notes started so far (tests check that none is missed).
    #[cfg(test)]
    notes_started: usize,
}

impl Sequencer {
    pub fn new(sample_rate: u32) -> Self {
        let sample_rate = sample_rate.max(1) as f32;
        Sequencer {
            sample_rate,
            position: 0.0,
            voices: Vec::with_capacity(HARD_VOICE_CAP),
            noise: 0x1234_5678,
            gains: Vec::with_capacity(RESERVED_TRACKS),
            limit_env: 0.0,
            limit_peak: 0.0,
            limit_release: (-1.0 / (LIMIT_RELEASE_SECONDS * sample_rate)).exp(),
            chase: false,
            send: vec![0.0; MAX_BLOCK].into_boxed_slice(),
            room: Room::new(sample_rate),
            plugin_notes: Vec::with_capacity(MAX_PLUGIN_NOTES),
            next_note_id: 0,
            events: Vec::with_capacity(MAX_EVENTS),
            clock: 0,
            cycle: None,
            #[cfg(test)]
            notes_started: 0,
        }
    }

    pub fn position(&self) -> f64 {
        self.position
    }

    /// Move the playhead. Voices that are still sounding fade out over a few
    /// milliseconds instead of being cut (which would click). When playback
    /// continues, synth notes that are held across the new position start
    /// sounding right away, part-way through, like in other DAWs.
    pub fn seek(&mut self, beat: f64) {
        self.position = beat.max(0.0);
        self.chase = true;
        for v in &mut self.voices {
            if v.delay > 0 {
                // Not started yet: drop silently.
                v.done = true;
            }
            v.fading = true;
            v.held = false;
        }
        self.voices.retain(|v| !v.done);
        // Plugin notes that are sounding end at once; waiting ones never start.
        for n in &mut self.plugin_notes {
            n.cut = true;
            n.held = false;
        }
        self.plugin_notes.retain(|n| n.started);
    }

    /// How far the master limiter turned the sound down at most since the
    /// last call, in dB (0 when it never had to).
    pub fn take_limiter_reduction_db(&mut self) -> f32 {
        let env = std::mem::take(&mut self.limit_peak);
        if env > LIMIT_THRESHOLD {
            20.0 * (env / LIMIT_THRESHOLD).log10()
        } else {
            0.0
        }
    }

    pub fn active_voices(&self) -> usize {
        self.voices.len()
    }

    /// Notes sounding (or about to start) on plugin instruments.
    pub fn active_plugin_notes(&self) -> usize {
        self.plugin_notes.len()
    }

    /// Whether anything is still audible: a voice, a plugin note, or the
    /// reverb's tail. (Plugins may ring on by themselves; see the engine.)
    pub fn is_sounding(&self) -> bool {
        !self.voices.is_empty() || !self.plugin_notes.is_empty() || !self.room.asleep
    }

    /// Render `out.len() / 2` stereo frames (interleaved L/R) and advance the playhead.
    ///
    /// The notes of every clip must be sorted by start ([`playable`] makes
    /// sure of it): notes out of order may be skipped.
    pub fn process(&mut self, project: &Project, out: &mut [f32]) {
        self.process_with(project, None, out);
    }

    /// [`Self::process`] with the project's plugins in `rack`.
    pub fn process_with(
        &mut self,
        project: &Project,
        rack: Option<&mut (dyn Rack + '_)>,
        out: &mut [f32],
    ) {
        self.cycle = None;
        self.render_block(project, rack, out, true);
    }

    /// Like [`Self::process`], but playback repeats `region` = `(start, end)`
    /// in beats: on reaching `end` the playhead jumps back to `start`, mid-block,
    /// and keeps going (see [`Project::playback_loop`]). A playhead before
    /// `start` plays on into the loop; one at or past `end` jumps back at once.
    ///
    /// Each pass sounds the same: notes still held at the loop end are let
    /// go (they ring out with their own release, as in a real loop), notes
    /// that start right on the end are not begun, and notes held across
    /// `start` are picked up part-way, as after a seek.
    pub fn process_looping(
        &mut self,
        project: &Project,
        out: &mut [f32],
        region: Option<(f64, f64)>,
    ) {
        self.process_looping_with(project, None, out, region);
    }

    /// [`Self::process_looping`] with the project's plugins in `rack`.
    pub fn process_looping_with(
        &mut self,
        project: &Project,
        mut rack: Option<&mut (dyn Rack + '_)>,
        out: &mut [f32],
        region: Option<(f64, f64)>,
    ) {
        let frames_per_beat = self.sample_rate as f64 * project.seconds_per_beat();
        // A loop shorter than a frame could never advance.
        let Some((start, end)) = region.filter(|&(s, e)| {
            s.is_finite() && e.is_finite() && s >= 0.0 && (e - s) * frames_per_beat >= 1.0
        }) else {
            self.process_with(project, rack, out);
            return;
        };
        self.cycle = Some((start, end));
        let mut rest = out;
        while rest.len() >= 2 {
            // Whole frames before the end: the block stops short of it, so it
            // never starts a note that lies on the end itself.
            let left = ((end - self.position) * frames_per_beat).floor();
            if left < 1.0 {
                self.wrap_to(start);
                continue;
            }
            let n = (rest.len() / 2).min(left as usize) * 2;
            let (now, later) = rest.split_at_mut(n);
            self.render_block(project, rack.as_deref_mut(), now, true);
            rest = later;
        }
        rest.fill(0.0);
    }

    /// The jump back at the end of a loop: notes still held are released,
    /// and those held across `beat` start sounding part-way through.
    fn wrap_to(&mut self, beat: f64) {
        for v in &mut self.voices {
            if !v.held && v.gate > v.age {
                v.gate = v.age;
            }
        }
        for n in &mut self.plugin_notes {
            if !n.held && n.gate > n.age {
                n.gate = n.age;
            }
        }
        self.position = beat;
        self.chase = true;
    }

    /// Render only the voices already sounding (e.g. note previews) without
    /// triggering clip notes or moving the playhead. Used while stopped.
    pub fn process_voices(&mut self, project: &Project, out: &mut [f32]) {
        self.process_voices_with(project, None, out);
    }

    /// [`Self::process_voices`] with the project's plugins in `rack`, so
    /// their tails and note previews keep sounding while stopped.
    pub fn process_voices_with(
        &mut self,
        project: &Project,
        rack: Option<&mut (dyn Rack + '_)>,
        out: &mut [f32],
    ) {
        self.render_block(project, rack, out, false);
    }

    /// Start a short one-off note on `track`'s instrument, for auditioning.
    pub fn preview(&mut self, project: &Project, track: usize, pitch_: u8) {
        let Some(t) = project.tracks.get(track) else {
            return;
        };
        let gate = (self.sample_rate * 0.35) as u64;
        self.spawn(track, &t.instrument, pitch_, 100, 0, gate);
    }

    /// Start a preview note that keeps sounding until [`Self::release_held`]
    /// (like holding a piano key). Any previously held note is released.
    pub fn preview_hold(&mut self, project: &Project, track: usize, pitch_: u8) {
        self.release_held();
        let Some(t) = project.tracks.get(track) else {
            return;
        };
        if self.spawn(track, &t.instrument, pitch_, 100, 0, u64::MAX) {
            if let Some(n) = self.plugin_notes.last_mut() {
                n.held = true;
            }
        } else if let Some(v) = self.voices.last_mut() {
            v.held = true;
        }
    }

    /// Let go of the held preview note; it fades out with its release time.
    pub fn release_held(&mut self) {
        for v in self.voices.iter_mut().filter(|v| v.held) {
            v.held = false;
            v.gate = v.age;
        }
        for n in self.plugin_notes.iter_mut().filter(|n| n.held) {
            n.held = false;
            n.gate = n.age;
        }
    }

    fn render_block(
        &mut self,
        project: &Project,
        mut rack: Option<&mut (dyn Rack + '_)>,
        out: &mut [f32],
        advance: bool,
    ) {
        for chunk in out.chunks_mut(MAX_BLOCK * 2) {
            self.render_chunk(project, rack.as_deref_mut(), chunk, advance);
        }
    }

    /// [`Self::render_block`] for at most [`MAX_BLOCK`] frames.
    fn render_chunk(
        &mut self,
        project: &Project,
        mut rack: Option<&mut (dyn Rack + '_)>,
        out: &mut [f32],
        advance: bool,
    ) {
        out.fill(0.0);
        let frames = out.len() / 2;
        if frames == 0 {
            return;
        }
        let frames_per_beat = self.sample_rate as f64 * project.seconds_per_beat();
        let b0 = self.position;
        let b1 = b0 + frames as f64 / frames_per_beat;
        if advance {
            if std::mem::take(&mut self.chase) {
                self.chase_notes(project, b0, frames_per_beat);
            }
            self.spawn_notes(project, b0, b1, frames_per_beat);
        }
        let send = &mut self.send[..frames];
        send.fill(0.0);

        // Per-track gain targets; new tracks start at their target (no fade-in).
        let any_solo = project.any_solo();
        let n_tracks = project.tracks.len();
        let old_len = self.gains.len();
        if old_len < n_tracks {
            self.gains.resize(n_tracks, TrackGain::default());
        }
        let sr = self.sample_rate;
        let inv_frames = 1.0 / frames as f32;
        let fade_step = 1.0 / (FADE_SECONDS * sr);
        let fade_in_step = 1.0 / (CHASE_FADE_SECONDS * sr);
        // Gains of track `i` across this block: where they start, and the
        // step per frame towards where they end.
        let gains = &self.gains;
        let ramp = |i: usize, track: &crate::model::Track| {
            let target = track_gains(track, any_solo);
            let from = if i >= old_len { target } else { gains[i].cur };
            let step = [
                (target[0] - from[0]) * inv_frames,
                (target[1] - from[1]) * inv_frames,
                (target[2] - from[2]) * inv_frames,
            ];
            (from, step, from == [0.0; 3] && target == [0.0; 3])
        };

        // Tracks with plugins start the block with an empty bus.
        if let Some(rack) = rack.as_deref_mut() {
            for i in 0..n_tracks {
                if rack.routes(i) {
                    if let Some([l, r]) = rack.bus(i) {
                        l[..frames].fill(0.0);
                        r[..frames].fill(0.0);
                    }
                }
            }
        }

        for voice in self.voices.iter_mut() {
            let Some(track) = project.tracks.get(voice.track) else {
                voice.done = true;
                continue;
            };
            let noise = &mut self.noise;
            // On a track with plugins the voice goes into the track's bus;
            // its gains are applied when the bus is mixed.
            if let Some([l, r]) = rack
                .as_deref_mut()
                .filter(|r| r.routes(voice.track))
                .and_then(|r| r.bus(voice.track))
            {
                for frame in 0..frames {
                    let Some(s) = tick(voice, sr, noise, fade_step, fade_in_step) else {
                        break;
                    };
                    l[frame] += s;
                    r[frame] += s;
                }
                continue;
            }
            let (from, step, silent) = ramp(voice.track, track);
            for frame in 0..frames {
                let Some(s) = tick(voice, sr, noise, fade_step, fade_in_step) else {
                    break;
                };
                if !silent {
                    let k = frame as f32;
                    out[frame * 2] += s * (from[0] + step[0] * k);
                    out[frame * 2 + 1] += s * (from[1] + step[1] * k);
                    send[frame] += s * (from[2] + step[2] * k);
                }
            }
        }
        self.voices.retain(|v| !v.done);

        match rack {
            Some(rack) => {
                let meter = project.time_signature();
                let transport = Transport {
                    sample_rate: f64::from(sr),
                    bpm: project.bpm,
                    beats_per_bar: project.beats_per_bar,
                    time_signature: (meter.num, meter.den),
                    beat: b0,
                    playing: advance,
                    cycle: self.cycle,
                    frames: self.clock,
                };
                for n in &mut self.plugin_notes {
                    if n.slot.is_none() {
                        n.slot = rack.instrument(n.track);
                    }
                    n.seen = false;
                }
                for (i, track) in project.tracks.iter().enumerate() {
                    if !rack.routes(i) {
                        continue;
                    }
                    let events = &mut self.events;
                    events.clear();
                    if let Some(slot) = rack.instrument(i) {
                        for n in self
                            .plugin_notes
                            .iter_mut()
                            .filter(|n| n.slot == Some(slot))
                        {
                            n.seen = true;
                            n.step(frames as u32, events);
                        }
                        sort_events(events);
                    }
                    rack.process(i, frames, events, &transport);
                    let Some([l, r]) = rack.bus(i) else {
                        continue;
                    };
                    let (from, step, silent) = ramp(i, track);
                    if silent {
                        continue;
                    }
                    for f in 0..frames {
                        let k = f as f32;
                        let (bl, br) = (l[f], r[f]);
                        out[f * 2] += bl * (from[0] + step[0] * k);
                        out[f * 2 + 1] += br * (from[1] + step[1] * k);
                        send[f] += 0.5 * (bl + br) * (from[2] + step[2] * k);
                    }
                }
                // Notes whose plugin is gone (track deleted, plugin swapped)
                // go with it.
                self.plugin_notes.retain(|n| n.seen && !n.done);
            }
            None => self.plugin_notes.clear(),
        }
        self.clock += frames as u64;

        for (g, t) in self.gains.iter_mut().zip(&project.tracks) {
            g.cur = track_gains(t, any_solo);
        }

        self.room.process(&project.reverb, send, out);

        // Master: gain -> peak limiter -> soft clipper.
        let master = if project.master_gain.is_finite() {
            project.master_gain.max(0.0)
        } else {
            0.0
        };
        for f in out.as_chunks_mut::<2>().0 {
            let mut l = f[0] * master;
            let mut r = f[1] * master;
            if !l.is_finite() || !r.is_finite() {
                l = 0.0;
                r = 0.0;
            }
            let peak = l.abs().max(r.abs());
            self.limit_env = if peak > self.limit_env {
                peak
            } else {
                peak + (self.limit_env - peak) * self.limit_release
            };
            self.limit_peak = self.limit_peak.max(self.limit_env);
            let g = if self.limit_env > LIMIT_THRESHOLD {
                LIMIT_THRESHOLD / self.limit_env
            } else {
                1.0
            };
            f[0] = soft_clip(l * g);
            f[1] = soft_clip(r * g);
        }
        if advance {
            self.position = b1;
        }
    }

    /// Start the synth notes that began before `beat` and are still held
    /// there, already `beat - start` into their envelopes, fading them in.
    /// Drum hits are one-shots and are not picked up half-way.
    fn chase_notes(&mut self, project: &Project, beat: f64, frames_per_beat: f64) {
        for (ti, track) in project.tracks.iter().enumerate() {
            if track.instrument.is_drums() {
                continue;
            }
            for clip in &track.clips {
                if clip.start >= beat || clip.end() <= beat {
                    continue;
                }
                for note in &clip.notes {
                    let at = clip.start + note.start;
                    // Notes starting at or after `beat` are spawn_notes' job,
                    // and in start order every later note does too.
                    if at >= beat {
                        break;
                    }
                    if !(0.0..clip.length).contains(&note.start) || note.duration <= 0.0 {
                        continue;
                    }
                    let held = note.duration.min(clip.length - note.start);
                    if at + held <= beat {
                        continue;
                    }
                    let gate = (held * frames_per_beat).max(1.0) as u64;
                    let age = ((beat - at) * frames_per_beat) as u64;
                    let plugin_notes = self.plugin_notes.len();
                    if self.spawn(ti, &track.instrument, note.pitch, note.velocity, 0, gate) {
                        // A plugin starts the note now, for what is left of
                        // it (unless all its note slots are taken).
                        if self.plugin_notes.len() > plugin_notes {
                            if let Some(n) = self.plugin_notes.last_mut() {
                                n.age = age;
                            }
                        }
                    } else if let Some(v) = self.voices.last_mut() {
                        v.age = age;
                        v.fade_in = 0.0;
                    }
                }
            }
        }
    }

    fn spawn_notes(&mut self, project: &Project, b0: f64, b1: f64, frames_per_beat: f64) {
        for (ti, track) in project.tracks.iter().enumerate() {
            for clip in &track.clips {
                if clip.end() <= b0 || clip.start >= b1 {
                    continue;
                }
                debug_assert!(clip.notes_sorted(), "notes of {} are not sorted", clip.id);
                // The notes are in start order: jump to the first one that
                // starts in this block and stop after the last one.
                let first = clip.notes.partition_point(|n| clip.start + n.start < b0);
                for note in &clip.notes[first..] {
                    let at = clip.start + note.start;
                    if at >= b1 {
                        break;
                    }
                    if !(0.0..clip.length).contains(&note.start) || note.duration <= 0.0 {
                        continue;
                    }
                    #[cfg(test)]
                    {
                        self.notes_started += 1;
                    }
                    let held = note.duration.min(clip.length - note.start);
                    let delay = ((at - b0) * frames_per_beat) as u32;
                    let gate = (held * frames_per_beat).max(1.0) as u64;
                    self.spawn(
                        ti,
                        &track.instrument,
                        note.pitch,
                        note.velocity,
                        delay,
                        gate,
                    );
                }
            }
        }
    }

    /// Start a note on `inst`: a voice of a built-in instrument, or (returns
    /// true) a note on a plugin instrument.
    fn spawn(
        &mut self,
        track: usize,
        inst: &Instrument,
        pitch_: u8,
        vel: u8,
        delay: u32,
        gate: u64,
    ) -> bool {
        let sr = self.sample_rate;
        let kind = match inst {
            Instrument::Vst3(_) => {
                if self.plugin_notes.len() < MAX_PLUGIN_NOTES {
                    self.next_note_id = self.next_note_id.wrapping_add(1) & i32::MAX;
                    self.plugin_notes.push(PluginNote {
                        track,
                        slot: None,
                        pitch: pitch_.min(127),
                        velocity: f32::from(vel.clamp(1, 127)) / 127.0,
                        id: self.next_note_id,
                        delay,
                        started: false,
                        gate,
                        age: 0,
                        held: false,
                        cut: false,
                        done: false,
                        seen: false,
                    });
                }
                return true;
            }
            Instrument::Synth(p) => Kind::Synth(SynthVoice::new(p, sr)),
            Instrument::Drums { kit } => {
                let kit = kit_params(*kit);
                let drum = Drum::from_note(pitch_, kit);
                let (hp, lp) = drum.filter_hz(kit);
                if matches!(drum, Drum::ClosedHat | Drum::OpenHat) {
                    // Hi-hat choke: a new hat cuts a ringing open hat.
                    for v in &mut self.voices {
                        if v.track == track
                            && matches!(
                                v.kind,
                                Kind::Drum {
                                    drum: Drum::OpenHat,
                                    ..
                                }
                            )
                            && v.delay <= delay
                            && !v.fading
                        {
                            v.fade_at = Some(delay);
                        }
                    }
                }
                Kind::Drum {
                    drum,
                    kit,
                    hp_coef: one_pole_coef(hp, sr),
                    lp_coef: one_pole_coef(lp, sr),
                    dark_coef: if kit.dark >= 20_000.0 {
                        1.0
                    } else {
                        one_pole_coef(kit.dark, sr)
                    },
                }
            }
        };
        if self.voices.len() >= MAX_VOICES {
            self.steal();
        }
        let mut phases = [0.0; 6];
        // Spread start phases so the cymbal oscillators don't start in sync
        // and the detuned saw doesn't start fully correlated.
        for (i, p) in phases.iter_mut().enumerate() {
            *p = (i as f32 * 0.161 + 0.37) % 1.0;
        }
        self.voices.push(Voice {
            track,
            kind,
            freq: pitch::frequency(pitch_ as f32),
            phase: 0.0,
            phases,
            vel: (vel.clamp(1, 127) as f32 / 127.0).powf(1.5),
            delay,
            gate,
            age: 0,
            release_from: 0.0,
            released_at: None,
            env: 0.0,
            z: [0.0; 4],
            done: false,
            held: false,
            fade: 1.0,
            fading: false,
            fade_in: 1.0,
            fade_at: None,
        });
        false
    }

    /// Make room for a new voice. Prefers voices that are already fading or
    /// releasing, then the oldest; the victim fades out instead of being cut.
    /// Only when the hard cap is reached is a (fading) voice removed outright.
    fn steal(&mut self) {
        if self.voices.len() >= HARD_VOICE_CAP {
            // Drop the quietest fading voice (or the oldest if none fade).
            let i = (0..self.voices.len())
                .max_by(|&a, &b| {
                    let (va, vb) = (&self.voices[a], &self.voices[b]);
                    (va.fading, -va.fade, va.age)
                        .partial_cmp(&(vb.fading, -vb.fade, vb.age))
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .unwrap_or(0);
            self.voices.swap_remove(i);
        }
        let victim = self
            .voices
            .iter_mut()
            .filter(|v| !v.fading)
            .max_by_key(|v| (v.released_at.is_some(), v.age));
        if let Some(v) = victim {
            // A voice that hasn't started yet can simply be dropped.
            v.done |= v.delay > 0;
            v.fading = true;
            v.held = false;
        }
        self.voices.retain(|v| !v.done);
    }
}

/// Order a block's events by frame, note-offs before note-ons on the same
/// frame (so a repeated note ends before it starts again). Insertion sort:
/// the list is short and nearly sorted, and this must not allocate.
fn sort_events(events: &mut [NoteEvent]) {
    for i in 1..events.len() {
        let mut j = i;
        while j > 0 && (events[j].offset, events[j].on) < (events[j - 1].offset, events[j - 1].on) {
            events.swap(j, j - 1);
            j -= 1;
        }
    }
}

/// Target gains of a track: (left, right) with equal-power pan, and its
/// reverb send, which follows the volume but not the pan. All 0 when the
/// track is muted or another one is soloed.
fn track_gains(t: &crate::model::Track, any_solo: bool) -> [f32; 3] {
    let audible = !t.mute && (!any_solo || t.solo);
    if !audible {
        return [0.0; 3];
    }
    let pan = finite_or(t.pan, 0.0).clamp(-1.0, 1.0);
    let angle = (pan + 1.0) * PI / 4.0;
    let gain = finite_or(t.gain, 0.0).max(0.0);
    let g = gain * std::f32::consts::SQRT_2;
    let send = gain * finite_or(t.reverb, 0.0).clamp(0.0, 1.0);
    [angle.cos() * g, angle.sin() * g, send]
}

/// Comb feedback, damping and input level for the reverb settings: size
/// 0..1 rings for about 0.6 s (small room) to 5 s (big hall), tone 0..1
/// goes from dark to bright.
fn room_params(r: &Reverb) -> (f32, f32, f32) {
    let size = finite_or(r.size, 0.5).clamp(0.0, 1.0);
    let tone = finite_or(r.tone, 0.5).clamp(0.0, 1.0);
    let feedback = 0.7 + 0.25 * size;
    // Bigger rooms build up more energy. Take back about half of it, so the
    // size mostly changes how long the tail is, not how loud.
    let level = ((1.0 - feedback * feedback) / (1.0 - 0.825f32 * 0.825)).powf(0.25);
    (feedback, 0.05 + 0.4 * (1.0 - tone), level)
}

/// How long the reverb rings (time to fall by 60 dB), in seconds.
pub fn reverb_tail_seconds(r: &Reverb) -> f64 {
    let (feedback, _, _) = room_params(r);
    let longest = (COMB_TUNING[7] + STEREO_SPREAD) as f64 / 44_100.0;
    3.0 * longest / -f64::from(feedback).log10()
}

/// A feedback comb filter with a damping low-pass in the loop.
struct Comb {
    buf: Box<[f32]>,
    pos: usize,
    lp: f32,
}

impl Comb {
    fn new(len: usize) -> Self {
        Comb {
            buf: vec![0.0; len.max(1)].into_boxed_slice(),
            pos: 0,
            lp: 0.0,
        }
    }

    #[inline]
    fn tick(&mut self, x: f32, feedback: f32, damp: f32) -> f32 {
        let y = self.buf[self.pos];
        self.lp = y + (self.lp - y) * damp;
        self.buf[self.pos] = x + self.lp * feedback;
        self.pos += 1;
        if self.pos == self.buf.len() {
            self.pos = 0;
        }
        y
    }
}

/// Schroeder all-pass: smears the echoes into a smooth tail.
struct Allpass {
    buf: Box<[f32]>,
    pos: usize,
}

impl Allpass {
    fn new(len: usize) -> Self {
        Allpass {
            buf: vec![0.0; len.max(1)].into_boxed_slice(),
            pos: 0,
        }
    }

    #[inline]
    fn tick(&mut self, x: f32) -> f32 {
        let b = self.buf[self.pos];
        self.buf[self.pos] = x + b * 0.5;
        self.pos += 1;
        if self.pos == self.buf.len() {
            self.pos = 0;
        }
        b - x
    }
}

/// The shared stereo reverb (Freeverb): eight damped combs in parallel
/// into four all-passes in series, per side. All buffers are allocated in
/// [`Room::new`].
struct Room {
    combs: [[Comb; 8]; 2],
    allpasses: [[Allpass; 4]; 2],
    /// Input high-pass state and coefficient.
    hp: f32,
    hp_coef: f32,
    /// Frames since the tail fell below [`REVERB_SILENCE`] with no input.
    quiet: usize,
    sleep_after: usize,
    /// Nothing in the buffers and no input: processing is skipped.
    asleep: bool,
}

impl Room {
    fn new(sr: f32) -> Self {
        let len = |n: usize, side: usize| {
            ((n + side * STEREO_SPREAD) as f32 * sr / 44_100.0).round() as usize
        };
        Room {
            combs: std::array::from_fn(|side| {
                std::array::from_fn(|i| Comb::new(len(COMB_TUNING[i], side)))
            }),
            allpasses: std::array::from_fn(|side| {
                std::array::from_fn(|i| Allpass::new(len(ALLPASS_TUNING[i], side)))
            }),
            hp: 0.0,
            hp_coef: one_pole_coef(REVERB_HP_HZ, sr),
            quiet: 0,
            sleep_after: (0.05 * sr) as usize,
            asleep: true,
        }
    }

    /// Add the reverb of `send` (one mono sample per frame) to `out`
    /// (interleaved stereo).
    fn process(&mut self, settings: &Reverb, send: &[f32], out: &mut [f32]) {
        let fed = send.iter().any(|x| *x != 0.0);
        if self.asleep && !fed {
            return;
        }
        self.asleep = false;
        let (feedback, damp, level) = room_params(settings);
        let mut peak = 0.0f32;
        for (x, o) in send.iter().zip(out.as_chunks_mut::<2>().0) {
            // Anything absurd from a track must not get stuck in the loop.
            let x = if x.is_finite() {
                x.clamp(-16.0, 16.0)
            } else {
                0.0
            };
            self.hp += self.hp_coef * (x - self.hp);
            let input = (x - self.hp) * (REVERB_IN * level);
            let [left, right] = &mut self.combs;
            let mut l = 0.0;
            let mut r = 0.0;
            for c in left.iter_mut() {
                l += c.tick(input, feedback, damp);
            }
            for c in right.iter_mut() {
                r += c.tick(input, feedback, damp);
            }
            let [left, right] = &mut self.allpasses;
            for a in left.iter_mut() {
                l = a.tick(l);
            }
            for a in right.iter_mut() {
                r = a.tick(r);
            }
            o[0] += l * REVERB_OUT;
            o[1] += r * REVERB_OUT;
            peak = peak.max(l.abs()).max(r.abs());
        }
        if fed || peak * REVERB_OUT > REVERB_SILENCE {
            self.quiet = 0;
            return;
        }
        self.quiet += send.len();
        if self.quiet >= self.sleep_after {
            // Start from exact silence next time (also keeps denormals out).
            for c in self.combs.iter_mut().flatten() {
                c.buf.fill(0.0);
                c.lp = 0.0;
            }
            for a in self.allpasses.iter_mut().flatten() {
                a.buf.fill(0.0);
            }
            self.hp = 0.0;
            self.quiet = 0;
            self.asleep = true;
        }
    }
}

fn finite_or(x: f32, default: f32) -> f32 {
    if x.is_finite() {
        x
    } else {
        default
    }
}

/// Coefficient of a one-pole low-pass with corner `hz` (clamped below Nyquist).
fn one_pole_coef(hz: f32, sr: f32) -> f32 {
    1.0 - (-TAU * hz.clamp(1.0, 0.45 * sr) / sr).exp()
}

/// Transparent below [`CLIP_KNEE`], smoothly saturating towards ±1 above it.
fn soft_clip(x: f32) -> f32 {
    let a = x.abs();
    if a <= CLIP_KNEE {
        x
    } else {
        let room = 1.0 - CLIP_KNEE;
        // Keep a hair of headroom so 16-bit conversion never wraps.
        let y = CLIP_KNEE + room * ((a - CLIP_KNEE) / room).tanh() * 0.999;
        y.copysign(x)
    }
}

fn noise(state: &mut u32) -> f32 {
    // xorshift32
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 17;
    x ^= x << 5;
    *state = x;
    (x as f32 / u32::MAX as f32) * 2.0 - 1.0
}

/// PolyBLEP residual to band-limit saw/square discontinuities.
fn poly_blep(t: f32, dt: f32) -> f32 {
    if t < dt {
        let t = t / dt;
        t + t - t * t - 1.0
    } else if t > 1.0 - dt {
        let t = (t - 1.0) / dt;
        t * t + t + t + 1.0
    } else {
        0.0
    }
}

fn blep_saw(p: f32, dt: f32) -> f32 {
    2.0 * p - 1.0 - poly_blep(p, dt)
}

/// One sample of `w` at phase `p` (0..1) with phase step `dt`. `pm` is a
/// phase offset (FM) applied to the sine and triangle.
fn wave(w: Waveform, p: f32, dt: f32, pm: f32, rng: &mut u32) -> f32 {
    match w {
        Waveform::Sine => ((p + pm) * TAU).sin(),
        Waveform::Triangle => {
            let q = (p + pm).rem_euclid(1.0);
            1.0 - 4.0 * (q - 0.5).abs()
        }
        Waveform::Saw => blep_saw(p, dt),
        Waveform::Square => {
            let sq = if p < 0.5 { 1.0 } else { -1.0 };
            sq + poly_blep(p, dt) - poly_blep((p + 0.5) % 1.0, dt)
        }
        Waveform::Pulse => {
            const WIDTH: f32 = 0.25;
            let sq = if p < WIDTH { 1.0 } else { -1.0 };
            // +0.5 removes the DC offset of a 25% pulse.
            (sq + poly_blep(p, dt) - poly_blep((p + 1.0 - WIDTH) % 1.0, dt) + 0.5) * 0.85
        }
        Waveform::Noise => noise(rng) * 0.7,
    }
}

impl Voice {
    fn next(&mut self, sr: f32, rng: &mut u32) -> f32 {
        let t = self.age as f32 / sr;
        let out = match self.kind {
            Kind::Synth(sv) => self.synth(&sv, t, sr, rng),
            Kind::Drum {
                drum,
                kit,
                hp_coef,
                lp_coef,
                dark_coef,
            } => {
                let len = drum.length(kit);
                if t >= len {
                    self.done = true;
                }
                // Fade the tail so voices end at exactly zero (no click).
                let tail = ((len - t) / DRUM_TAIL_SECONDS).clamp(0.0, 1.0);
                let mut s = self.drum(drum, kit, t, sr, rng, hp_coef, lp_coef);
                if kit.crush > 0.0 {
                    s = (s * kit.crush).round() / kit.crush;
                }
                if dark_coef < 1.0 {
                    self.z[1] += dark_coef * (s - self.z[1]);
                    s = self.z[1];
                }
                s * kit.gain * tail
            }
        };
        self.age += 1;
        out * self.vel
    }

    fn synth(&mut self, sv: &SynthVoice, t: f32, sr: f32, rng: &mut u32) -> f32 {
        if self.released_at.is_none() && self.age >= self.gate {
            self.released_at = Some(self.age);
            self.release_from = self.env;
        }
        self.env = match self.released_at {
            None if t < sv.attack => t / sv.attack,
            None if t < sv.attack + sv.decay => {
                1.0 - (1.0 - sv.sustain) * (t - sv.attack) / sv.decay
            }
            None => sv.sustain,
            Some(at) => {
                let rt = (self.age - at) as f32 / sr;
                if rt >= sv.release {
                    self.done = true;
                }
                self.release_from * (1.0 - rt / sv.release).max(0.0)
            }
        };

        let mut freq = self.freq;
        if sv.pitch_env != 0.0 {
            freq *= (sv.pitch_env / 12.0 * (-t / sv.pitch_decay).exp()).exp2();
        }
        if sv.vibrato > 0.0 {
            // Vibrato eases in, like a player who starts the note straight.
            let depth = sv.vibrato * (t / 0.5).min(1.0);
            freq *= (depth / 1200.0 * (TAU * sv.vibrato_rate * t).sin()).exp2();
        }
        let dt = (freq / sr).min(0.5);

        let mut pm = 0.0;
        if sv.fm > 0.0 {
            let mdt = dt * sv.fm_ratio;
            if mdt < 0.5 {
                let index = if sv.fm_decay > 0.0 {
                    sv.fm * (-t / sv.fm_decay).exp()
                } else {
                    sv.fm
                };
                let m = self.phases[5];
                self.phases[5] = (m + mdt).fract();
                pm = index * (m * TAU).sin() / TAU;
            }
        }

        let mut osc = 0.0;
        for i in 0..sv.unison {
            let d = (dt * sv.ratios[i]).min(0.5);
            let ph = if i == 0 {
                &mut self.phase
            } else {
                &mut self.phases[i - 1]
            };
            let p = *ph;
            *ph = (p + d) % 1.0;
            osc += wave(sv.waveform, p, d, pm, rng);
        }
        osc *= sv.unison_gain;
        if sv.sub > 0.0 {
            let p = self.phases[4];
            self.phases[4] = (p + dt * 0.5) % 1.0;
            osc += sv.sub * (p * TAU).sin();
        }
        if sv.noise > 0.0 {
            osc += sv.noise * noise(rng);
        }

        let mut y = if sv.svf {
            let g = if sv.filter_env > 0.0 {
                let oct = sv.filter_env * (-t / sv.filter_decay).exp();
                svf_g(sv.cutoff * oct.exp2(), sr)
            } else {
                sv.g
            };
            // Topology-preserving state-variable low-pass (Zavalishin/Simper).
            let a1 = 1.0 / (1.0 + g * (g + sv.k));
            let a2 = g * a1;
            let a3 = g * a2;
            let v3 = osc - self.z[1];
            let v1 = a1 * self.z[0] + a2 * v3;
            let v2 = self.z[1] + a2 * self.z[0] + a3 * v3;
            self.z[0] = 2.0 * v1 - self.z[0];
            self.z[1] = 2.0 * v2 - self.z[1];
            v2
        } else {
            // 12 dB/oct low-pass: smoother, less buzzy than a single pole.
            self.z[0] += sv.lp_coef * (osc - self.z[0]);
            self.z[1] += sv.lp_coef * (self.z[0] - self.z[1]);
            self.z[1]
        };
        if sv.drive > 0.0 {
            let g = 1.0 + 5.0 * sv.drive;
            y = (y * g).tanh() / g.tanh();
        }
        y * self.env * 0.3
    }

    #[allow(clippy::too_many_arguments)]
    fn drum(
        &mut self,
        drum: Drum,
        kit: &Kit,
        t: f32,
        sr: f32,
        rng: &mut u32,
        hp_coef: f32,
        lp_coef: f32,
    ) -> f32 {
        let sine = |p: &mut f32, f: f32| {
            let out = (*p * TAU).sin();
            *p = (*p + f / sr) % 1.0;
            out
        };
        match drum {
            Drum::Kick => {
                // Fast pitch drop to the kit's base pitch, saturated for
                // punch, with a short noise click.
                let f = kit.kick_base
                    + kit.kick_sweep * (-t / kit.kick_sweep_time).exp()
                    + 40.0 * (-t / 0.006).exp();
                self.phase = (self.phase + f / sr) % 1.0;
                let amp = (-t / kit.kick_decay).exp();
                let body = (kit.kick_drive * (self.phase * TAU).sin() * amp).tanh() * 0.8;
                let click = self.band(noise(rng), hp_coef, lp_coef) * (-t / 0.004).exp();
                body + click * kit.kick_click
            }
            Drum::Tom(base) => {
                let f = base * (1.0 + 0.5 * (-t / 0.05).exp());
                self.phase = (self.phase + f / sr) % 1.0;
                let attack = self.band(noise(rng), hp_coef, lp_coef) * (-t / 0.01).exp();
                (self.phase * TAU).sin() * (-t / kit.tom_decay).exp() * 0.8 + attack * 0.25
            }
            Drum::Snare => {
                // Two drum-head modes plus band-passed noise (wires).
                let f = kit.snare_tone;
                self.phase = (self.phase + f / sr) % 1.0;
                self.phases[0] = (self.phases[0] + f * 1.833 / sr) % 1.0;
                let tone = ((self.phase * TAU).sin() + 0.6 * (self.phases[0] * TAU).sin())
                    * (-t / 0.05).exp()
                    * kit.snare_tone_mix;
                let wires = self.band(noise(rng), hp_coef, lp_coef) * (-t / kit.snare_decay).exp();
                tone + wires * 0.8
            }
            Drum::Clap => {
                let mut burst = (-t / kit.clap_decay).exp() * 0.5;
                for o in [0.0f32, 0.01, 0.02] {
                    if t >= o {
                        burst += (-(t - o) / 0.012).exp();
                    }
                }
                self.band(noise(rng), hp_coef, lp_coef) * burst * 0.45
            }
            Drum::ClosedHat => {
                self.metal(sr, rng, hp_coef, lp_coef, kit.metal) * (-t / kit.hat_decay).exp()
            }
            Drum::OpenHat => {
                self.metal(sr, rng, hp_coef, lp_coef, kit.metal)
                    * (-t / kit.open_decay).exp()
                    * 0.85
            }
            Drum::Crash => {
                self.metal(sr, rng, hp_coef, lp_coef, kit.metal)
                    * (-t / kit.cymbal_decay).exp()
                    * 0.6
            }
            Drum::Splash => {
                self.metal(sr, rng, hp_coef, lp_coef, kit.metal)
                    * (-t / (kit.cymbal_decay * 0.4)).exp()
                    * 0.6
            }
            Drum::Ride => {
                let ping = sine(&mut self.phase, 3_150.0) * (-t / 0.7).exp() * 0.07;
                let wash = self.metal(sr, rng, hp_coef, lp_coef, kit.metal);
                ping + wash
                    * ((-t / (kit.cymbal_decay * 1.6)).exp() * 0.22 + (-t / 0.01).exp() * 0.3)
            }
            Drum::RideBell => {
                let bell = sine(&mut self.phase, 2_350.0) * (-t / 0.5).exp() * 0.35;
                let wash = self.metal(sr, rng, hp_coef, lp_coef, kit.metal);
                bell + wash * (-t / (kit.cymbal_decay * 0.8)).exp() * 0.2
            }
            Drum::Rim => {
                let hi = sine(&mut self.phase, 1_700.0) * (-t / 0.012).exp();
                let lo = sine(&mut self.phases[0], 450.0) * (-t / 0.03).exp() * 0.3;
                let tick = self.band(noise(rng), hp_coef, lp_coef) * (-t / 0.008).exp() * 0.5;
                (hi + lo + tick) * 0.7
            }
            Drum::Cowbell => {
                // TR-808 style: two square oscillators, band-passed.
                let mut sq = 0.0;
                for (i, f) in [540.0f32, 800.0].into_iter().enumerate() {
                    let p = &mut self.phases[i];
                    *p = (*p + f / sr) % 1.0;
                    sq += if *p < 0.5 { 0.5 } else { -0.5 };
                }
                let env = (-t / 0.05).exp() * 0.6 + (-t / 0.22).exp() * 0.4;
                self.band(sq, hp_coef, lp_coef) * env * 1.4
            }
            Drum::Tambourine => {
                let jingles = self.metal(sr, rng, hp_coef, lp_coef, 0.6);
                let env = (-t / 0.005).exp() + (-t / 0.12).exp() * 0.6;
                jingles * env * 0.55
            }
            Drum::Shaker => {
                let env = (t / 0.012).min(1.0) * (-(t - 0.012).max(0.0) / 0.05).exp();
                self.band(noise(rng), hp_coef, lp_coef) * env * 0.7
            }
            Drum::Hand(f, d) => {
                let f = f * (1.0 + 0.15 * (-t / 0.02).exp());
                let body = sine(&mut self.phase, f) * (-t / d).exp() * 0.8;
                let over = sine(&mut self.phases[0], f * 1.52) * (-t / (d * 0.5)).exp() * 0.25;
                let slap = self.band(noise(rng), hp_coef, lp_coef) * (-t / 0.006).exp() * 0.4;
                body + over + slap
            }
            Drum::Wood(f, d) => {
                let body = sine(&mut self.phase, f) * (-t / d).exp();
                let over = sine(&mut self.phases[0], f * 2.7) * (-t / (d * 0.5)).exp() * 0.3;
                (body + over) * 0.6
            }
            Drum::Agogo(f) => {
                let body = sine(&mut self.phase, f) * (-t / 0.1).exp();
                let over = sine(&mut self.phases[0], f * 2.76) * (-t / 0.05).exp() * 0.4;
                (body + over) * 0.5
            }
            Drum::Triangle(d) => {
                let a = sine(&mut self.phase, 4_200.0);
                let b = sine(&mut self.phases[0], 6_850.0) * 0.5;
                let c = sine(&mut self.phases[1], 9_430.0) * 0.3;
                (a + b + c) * (-t / d).exp() * 0.3
            }
        }
    }

    /// Band-pass (two high-pass poles, one low-pass pole) of `x`.
    fn band(&mut self, x: f32, hp_coef: f32, lp_coef: f32) -> f32 {
        self.z[2] += hp_coef * (x - self.z[2]);
        let h1 = x - self.z[2];
        self.z[3] += hp_coef * (h1 - self.z[3]);
        let h2 = h1 - self.z[3];
        self.z[0] += lp_coef * (h2 - self.z[0]);
        self.z[0]
    }

    /// TR-808 style metallic source: six square oscillators plus some noise,
    /// band-passed. Less harsh than plain white noise. `metal` sets how much
    /// of the square bank is heard (1.0 = the classic mix).
    fn metal(&mut self, sr: f32, rng: &mut u32, hp_coef: f32, lp_coef: f32, metal: f32) -> f32 {
        let mut m = 0.0;
        for (p, f) in self.phases.iter_mut().zip(METAL_FREQS) {
            *p = (*p + f * 1.7 / sr) % 1.0;
            m += if *p < 0.5 { 1.0 } else { -1.0 };
        }
        let x = m * (metal / 6.0) + noise(rng) * 0.5;
        self.band(x, hp_coef, lp_coef) * 2.2
    }
}

/// Seconds rendered after the last clip: [`RENDER_TAIL_SECONDS`] for note
/// releases, plus the reverb's ring-out when an audible track uses it.
pub fn tail_seconds(project: &Project) -> f64 {
    let reverb = project
        .tracks
        .iter()
        .any(|t| t.reverb > 0.0 && project.track_audible(t));
    if reverb {
        RENDER_TAIL_SECONDS + reverb_tail_seconds(&project.reverb)
    } else {
        RENDER_TAIL_SECONDS
    }
}

/// `project` ready for the [`Sequencer`]: every clip's notes sorted by
/// start. Borrowed as it is when they already are (the usual case: loading
/// and editing keep them sorted), sorted in a copy otherwise.
pub fn playable(project: &Project) -> Cow<'_, Project> {
    if project.notes_sorted() {
        Cow::Borrowed(project)
    } else {
        let mut p = project.clone();
        p.sort_notes();
        Cow::Owned(p)
    }
}

/// [`tail_seconds`] plus whatever the rack's plugins need to ring out.
pub fn tail_seconds_with(project: &Project, rack: Option<&(dyn Rack + '_)>) -> f64 {
    let plugins = rack.map_or(0.0, |r| r.tail_seconds());
    tail_seconds(project)
        + if plugins.is_finite() {
            plugins.clamp(0.0, 30.0)
        } else {
            0.0
        }
}

/// Render the whole project (plus a tail for releases) to interleaved stereo.
pub fn render(project: &Project, sample_rate: u32) -> Vec<f32> {
    render_with(project, None, sample_rate)
}

/// [`render`] with the project's plugins in `rack`.
pub fn render_with(
    project: &Project,
    mut rack: Option<&mut (dyn Rack + '_)>,
    sample_rate: u32,
) -> Vec<f32> {
    let project = &*playable(project);
    let tail = tail_seconds_with(project, rack.as_deref());
    let total_seconds = project.length_beats() * project.seconds_per_beat() + tail;
    let total_frames = (total_seconds * sample_rate as f64) as usize;
    let mut seq = Sequencer::new(sample_rate);
    let mut out = vec![0.0f32; total_frames * 2];
    for chunk in out.chunks_mut(512 * 2) {
        seq.process_with(project, rack.as_deref_mut(), chunk);
    }
    out
}

pub fn write_wav(path: &Path, samples: &[f32], sample_rate: u32) -> Result<()> {
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(path, spec)?;
    for s in samples {
        let s = if s.is_finite() { *s } else { 0.0 };
        w.write_sample((s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)?;
    }
    w.finalize()?;
    Ok(())
}

/// Longest song [`render_to_wav`] will write. Rendering streams to disk in
/// small blocks, so this only bounds the size of the WAV file (about 635 MB
/// per hour at 44.1 kHz), not memory.
pub const MAX_RENDER_SECONDS: f64 = 3600.0;

/// Seconds appended after the last clip so releases can ring out (more
/// when the reverb is in use, see [`tail_seconds`]).
pub const RENDER_TAIL_SECONDS: f64 = 2.0;

pub(crate) fn check_render_args(project: &Project, sample_rate: u32) -> Result<()> {
    if !(8_000..=192_000).contains(&sample_rate) {
        anyhow::bail!("sample rate must be between 8000 and 192000 Hz (got {sample_rate})");
    }
    let seconds = project.length_beats() * 60.0 / project.bpm.max(1.0);
    if seconds.is_nan() || seconds > MAX_RENDER_SECONDS {
        anyhow::bail!(
            "the song is {:.0} minutes long; the limit is {:.0} minutes. Shorten or remove very long clips first.",
            seconds / 60.0,
            MAX_RENDER_SECONDS / 60.0
        );
    }
    Ok(())
}

/// Render the song straight into a 16-bit stereo WAV file, block by block,
/// so even an hour-long song needs only a few kilobytes of memory. Returns
/// the duration written in seconds.
pub fn render_to_wav(project: &Project, path: &Path, sample_rate: u32) -> Result<f64> {
    render_to_wav_with_progress(project, path, sample_rate, |_| {})
}

/// Like [`render_to_wav`], calling `progress` with a value from 0.0 to 1.0
/// as the render advances (handy for a progress bar on another thread).
pub fn render_to_wav_with_progress(
    project: &Project,
    path: &Path,
    sample_rate: u32,
    progress: impl FnMut(f64),
) -> Result<f64> {
    render_to_wav_with(project, None, path, sample_rate, progress)
}

/// [`render_to_wav_with_progress`] with the project's plugins in `rack`
/// (prepared for `sample_rate`, ideally for offline processing).
pub fn render_to_wav_with(
    project: &Project,
    mut rack: Option<&mut (dyn Rack + '_)>,
    path: &Path,
    sample_rate: u32,
    mut progress: impl FnMut(f64),
) -> Result<f64> {
    check_render_args(project, sample_rate)?;
    let project = &*playable(project);
    let tail = tail_seconds_with(project, rack.as_deref());
    let total_seconds = project.length_beats() * project.seconds_per_beat() + tail;
    let total_frames = (total_seconds * sample_rate as f64) as usize;
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(path, spec)
        .with_context(|| format!("creating {}", path.display()))?;
    let mut seq = Sequencer::new(sample_rate);
    let mut block = vec![0.0f32; 512 * 2];
    let mut done = 0usize;
    let mut next_report = 0usize;
    while done < total_frames {
        let n = (total_frames - done).min(512);
        let out = &mut block[..n * 2];
        seq.process_with(project, rack.as_deref_mut(), out);
        for s in out.iter() {
            let s = if s.is_finite() { *s } else { 0.0 };
            w.write_sample((s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)?;
        }
        done += n;
        if done >= next_report {
            progress(done as f64 / total_frames.max(1) as f64);
            next_report = done + sample_rate as usize / 4;
        }
    }
    w.finalize()?;
    progress(1.0);
    Ok(total_frames as f64 / sample_rate as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn demo_project() -> Project {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/demo.aidaw.json");
        crate::io::load(&path).expect("demo project loads")
    }

    fn peak(buf: &[f32]) -> f32 {
        buf.iter().fold(0.0f32, |m, s| m.max(s.abs()))
    }

    /// One drum track whose single clip holds `count` hits a sixteenth apart.
    fn busy_song(count: usize) -> Project {
        let mut p = Project::new("busy", 133.0);
        p.tracks = demo_project().tracks;
        p.tracks.truncate(1);
        p.tracks[0].instrument = Instrument::Drums {
            kit: DrumKit::Standard,
        };
        p.tracks[0].clips = vec![crate::Clip {
            id: "c1".into(),
            name: String::new(),
            start: 1.0,
            length: count as f64 * 0.25,
            notes: (0..count)
                .map(|i| crate::Note {
                    pitch: [36, 38, 42, 46, 49][i % 5],
                    start: i as f64 * 0.25,
                    duration: 0.1,
                    velocity: 100,
                })
                .collect(),
        }];
        p
    }

    #[test]
    fn every_note_starts_exactly_once_whatever_the_block_size() {
        let p = busy_song(400);
        let end = p.length_beats();
        // Odd sizes put note starts right next to block edges; 1500 is more
        // than one chunk.
        for frames in [7, 37, 512, 1500] {
            let mut seq = Sequencer::new(22_050);
            let mut buf = vec![0.0; frames * 2];
            while seq.position() < end {
                seq.process(&p, &mut buf);
            }
            assert_eq!(seq.notes_started, 400, "blocks of {frames} frames");
        }
    }

    #[test]
    fn notes_out_of_order_play_the_same_once_made_playable() {
        let sorted = busy_song(64);
        let mut shuffled = sorted.clone();
        let notes = &mut shuffled.tracks[0].clips[0].notes;
        notes.reverse();
        notes.swap(3, 40);
        assert!(!shuffled.notes_sorted());
        assert!(matches!(playable(&sorted), Cow::Borrowed(_)));
        assert_eq!(*playable(&shuffled), sorted);
        assert_eq!(render(&shuffled, 22_050), render(&sorted, 22_050));
    }

    #[test]
    fn demo_render_is_finite_and_never_clips() {
        let p = demo_project();
        let audio = render(&p, 44_100);
        assert!(audio.iter().all(|s| s.is_finite()));
        let pk = peak(&audio);
        assert!(pk > 0.05 && pk < 1.0, "peak {pk}");
    }

    #[test]
    fn many_loud_tracks_stay_inside_full_scale() {
        let mut p = demo_project();
        p.master_gain = 4.0;
        p.reverb.size = 1.0;
        let base = p.tracks.clone();
        for _ in 0..6 {
            for t in &base {
                let mut t = t.clone();
                t.gain = 3.0;
                t.reverb = 1.0;
                p.tracks.push(t);
            }
        }
        let audio = render(&p, 22_050);
        assert!(audio.iter().all(|s| s.is_finite() && s.abs() < 1.0));
    }

    #[test]
    fn nan_parameters_do_not_poison_output() {
        let mut p = demo_project();
        p.master_gain = f32::NAN;
        p.tracks[0].gain = f32::INFINITY;
        p.tracks[0].pan = f32::NAN;
        let audio = render(&p, 22_050);
        assert!(audio.iter().all(|s| s.is_finite() && s.abs() < 1.0));

        let mut p = demo_project();
        p.tracks[0].reverb = f32::NAN;
        p.tracks[1].reverb = f32::INFINITY;
        p.reverb.size = f32::NAN;
        p.reverb.tone = f32::NEG_INFINITY;
        let audio = render(&p, 22_050);
        assert!(audio.iter().all(|s| s.is_finite() && s.abs() < 1.0));
        assert!(peak(&audio) > 0.05);
    }

    #[test]
    fn silence_after_release_tail() {
        let mut p = demo_project();
        // Only the notes' releases here; the reverb's tail has its own test.
        for t in &mut p.tracks {
            t.reverb = 0.0;
        }
        let sr = 22_050;
        let audio = render(&p, sr);
        // The demo's longest release is 1.2 s; the last 0.5 s of the 2 s tail
        // must be completely silent and no voices may linger.
        let tail = &audio[audio.len() - sr as usize..];
        assert!(peak(tail) == 0.0, "tail peak {}", peak(tail));
    }

    #[test]
    fn loop_end_rounds_last_note_up_to_bar() {
        let mut p = Project::new("t", 120.0);
        assert_eq!(p.loop_end_beats(), None);
        p.tracks = demo_project().tracks;
        for t in &mut p.tracks {
            t.clips.clear();
        }
        p.tracks[0].clips.push(crate::Clip {
            id: "c".into(),
            name: String::new(),
            start: 4.0,
            length: 16.0,
            notes: vec![crate::Note {
                pitch: 60,
                start: 1.0,
                duration: 1.5,
                velocity: 100,
            }],
        });
        // Note ends at beat 6.5 -> wraps at the end of bar 2 (beat 8),
        // even though the clip itself runs on to beat 20.
        assert_eq!(p.loop_end_beats(), Some(8.0));
        p.tracks[0].clips[0].notes[0].duration = 3.0; // ends exactly on beat 8
        assert_eq!(p.loop_end_beats(), Some(8.0));
    }

    #[test]
    fn playback_wraps_to_start_after_last_note() {
        let p = demo_project();
        let end = p.loop_end_beats().expect("demo has notes");
        let sr = 22_050;
        let mut seq = Sequencer::new(sr);
        seq.seek(end - 0.25);
        let fpb = sr as f64 * p.seconds_per_beat();
        let frames = (fpb * 0.5) as usize; // half a beat: crosses the loop point
        let mut buf = vec![0.0f32; frames * 2];
        seq.process_looping(&p, &mut buf, Some((0.0, end)));
        assert!(
            (seq.position() - 0.25).abs() < 1e-3,
            "pos {}",
            seq.position()
        );
        // The first note(s) of the song were triggered after the wrap.
        assert!(peak(&buf[frames..]) > 0.0);
        assert!(buf.iter().all(|s| s.is_finite()));
    }

    #[test]
    fn streamed_wav_matches_in_memory_render() {
        let p = demo_project();
        let sr = 22_050;
        let path = std::env::temp_dir().join(format!("aidaw-render-{}.wav", std::process::id()));
        let mut reports = Vec::new();
        let secs = render_to_wav_with_progress(&p, &path, sr, |x| reports.push(x)).unwrap();
        let expected = render(&p, sr);
        assert!((secs - expected.len() as f64 / 2.0 / sr as f64).abs() < 1e-9);
        let got: Vec<i16> = hound::WavReader::open(&path)
            .unwrap()
            .samples::<i16>()
            .map(Result::unwrap)
            .collect();
        assert_eq!(got.len(), expected.len());
        for (a, b) in got.iter().zip(&expected) {
            assert_eq!(*a, (b.clamp(-1.0, 1.0) * i16::MAX as f32) as i16);
        }
        assert_eq!(reports.last().copied(), Some(1.0));
        assert!(reports.windows(2).all(|w| w[0] <= w[1]));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn seek_fades_instead_of_cutting() {
        let p = demo_project();
        let mut seq = Sequencer::new(22_050);
        seq.preview_hold(&p, 0, 60);
        let mut buf = vec![0.0f32; 2 * 4096];
        seq.process_voices(&p, &mut buf);
        let last = buf[buf.len() - 2].abs();
        seq.seek(0.0);
        let mut next = vec![0.0f32; 2 * 512];
        seq.process_voices(&p, &mut next);
        // First sample continues smoothly from where we were...
        assert!((next[0].abs() - last).abs() < 0.05);
        // ...and the voice is gone after the short fade.
        assert_eq!(seq.active_voices(), 0);
    }

    /// One track holding a single note from beat 0 to beat 8.
    fn held_note(instrument: Instrument) -> Project {
        let mut p = Project::new("t", 120.0);
        p.tracks = demo_project().tracks;
        p.tracks.truncate(1);
        p.tracks[0].instrument = instrument;
        p.tracks[0].clips = vec![crate::Clip {
            id: "c".into(),
            name: String::new(),
            start: 0.0,
            length: 8.0,
            notes: vec![crate::Note {
                pitch: 48,
                start: 0.0,
                duration: 8.0,
                velocity: 100,
            }],
        }];
        p
    }

    #[test]
    fn playing_from_the_middle_of_a_held_note_picks_it_up() {
        let p = held_note(Instrument::Synth(Synth::BASE));
        let sr = 22_050;
        let mut seq = Sequencer::new(sr);
        seq.seek(4.0);
        let mut buf = vec![0.0f32; 2 * 2048];
        seq.process(&p, &mut buf);
        assert!(peak(&buf) > 0.02, "silent: {}", peak(&buf));
        // Faded in, not switched on at full level (which would click).
        assert!(peak(&buf[..20]) < 0.01, "{}", peak(&buf[..20]));
        // Already in its sustain: no second attack.
        let fade_in = (CHASE_FADE_SECONDS * sr as f32) as usize * 2;
        let after = peak(&buf[fade_in..fade_in + 400]);
        assert!(after > 0.02, "{after}");

        // Played on from the start, the same note sounds the same once settled.
        let mut from_start = Sequencer::new(sr);
        let mut long = vec![0.0f32; 2 * (sr as usize * 2 + 2048)];
        from_start.process(&p, &mut long);
        let settled = peak(&long[long.len() - 1000..]);
        let chased = peak(&buf[buf.len() - 1000..]);
        assert!((settled - chased).abs() < 0.02, "{settled} vs {chased}");

        // A seek past the note's end, or before it starts, hears nothing new.
        let mut seq = Sequencer::new(sr);
        seq.seek(9.0);
        let mut quiet = vec![0.0f32; 2 * 2048];
        seq.process(&p, &mut quiet);
        assert_eq!(peak(&quiet), 0.0);
    }

    /// Play `frames` frames looping `region`, in blocks like a sound card asks for.
    fn play_loop(seq: &mut Sequencer, p: &Project, region: (f64, f64), frames: usize) -> Vec<f32> {
        let mut out = vec![0.0f32; frames * 2];
        for block in out.chunks_mut(2 * 441) {
            seq.process_looping(p, block, Some(region));
        }
        out
    }

    #[test]
    fn a_loop_region_repeats_and_keeps_its_place() {
        let p = held_note(Instrument::Synth(Synth::BASE));
        let sr = 22_050;
        let fpb = sr as f64 * p.seconds_per_beat();
        let mut seq = Sequencer::new(sr);
        // Beats 4-6 of an 8-beat note: the note is held across the loop start.
        seq.seek(4.0);
        let audio = play_loop(&mut seq, &p, (4.0, 6.0), (fpb * 5.0) as usize);
        // 5 beats in a 2-beat loop: a beat into the third pass.
        assert!((seq.position() - 5.0).abs() < 1e-3, "{}", seq.position());
        // Every pass picks the note up again, so the loop never goes quiet.
        for pass in 0..2 {
            let after_wrap = (fpb * (2.0 * pass as f64 + 2.2)) as usize * 2;
            let s = peak(&audio[after_wrap..after_wrap + 2000]);
            assert!(s > 0.02, "pass {pass} silent after the wrap: {s}");
        }
        // ...and the voice that was playing at the end was let go: only the
        // newly picked-up one is left once the release (0.25 s) is over.
        assert_eq!(seq.active_voices(), 1);
        assert!(audio.iter().all(|s| s.is_finite()));
    }

    #[test]
    fn a_note_right_on_the_loop_end_is_not_started() {
        // A kick on beat 4 only, looping beats 0-4: the kick is never heard.
        let mut p = held_note(Instrument::Drums {
            kit: DrumKit::Standard,
        });
        p.tracks[0].clips[0].notes[0].start = 4.0;
        p.tracks[0].clips[0].notes[0].duration = 0.25;
        let sr = 22_050;
        let fpb = sr as f64 * p.seconds_per_beat();
        let mut seq = Sequencer::new(sr);
        let audio = play_loop(&mut seq, &p, (0.0, 4.0), (fpb * 13.0) as usize);
        assert_eq!(peak(&audio), 0.0);
        // Played through without the loop, it is.
        let mut seq = Sequencer::new(sr);
        let mut once = vec![0.0f32; (fpb * 5.0) as usize * 2];
        seq.process(&p, &mut once);
        assert!(peak(&once) > 0.05);
    }

    #[test]
    fn playing_before_a_loop_runs_into_it_and_past_it_jumps_back() {
        let p = blip(0.0);
        let sr = 22_050;
        let fpb = sr as f64 * p.seconds_per_beat();
        let mut seq = Sequencer::new(sr);
        seq.seek(1.0);
        play_loop(&mut seq, &p, (4.0, 8.0), (fpb * 2.0) as usize);
        assert!((seq.position() - 3.0).abs() < 1e-3, "{}", seq.position());
        play_loop(&mut seq, &p, (4.0, 8.0), (fpb * 2.0) as usize);
        assert!((seq.position() - 5.0).abs() < 1e-3, "{}", seq.position());
        seq.seek(12.0);
        play_loop(&mut seq, &p, (4.0, 8.0), (fpb * 0.5) as usize);
        assert!((seq.position() - 4.5).abs() < 1e-3, "{}", seq.position());
        // A loop too short to play just plays on.
        seq.seek(0.0);
        play_loop(&mut seq, &p, (2.0, 2.0), (fpb * 3.0) as usize);
        assert!((seq.position() - 3.0).abs() < 1e-3, "{}", seq.position());
    }

    #[test]
    fn drum_hits_are_not_picked_up_half_way() {
        let p = held_note(Instrument::Drums {
            kit: DrumKit::Standard,
        });
        let mut seq = Sequencer::new(22_050);
        seq.seek(0.1);
        let mut buf = vec![0.0f32; 2 * 2048];
        seq.process(&p, &mut buf);
        assert_eq!(peak(&buf), 0.0);
        assert_eq!(seq.active_voices(), 0);
    }

    /// One synth track playing a single one-beat note, with a reverb send.
    fn blip(reverb: f32) -> Project {
        let mut p = held_note(Instrument::Synth(Synth::BASE));
        p.tracks[0].clips[0].notes[0].duration = 1.0;
        p.tracks[0].reverb = reverb;
        p
    }

    #[test]
    fn reverb_rings_on_after_the_note_and_then_sleeps() {
        let sr = 22_050;
        let second = 2 * sr as usize;
        // After one second the note (0.5 s) and its release (0.25 s) are over...
        let mut dry = Sequencer::new(sr);
        let mut buf = vec![0.0f32; second];
        dry.process(&blip(0.0), &mut buf);
        assert_eq!(peak(&buf[second - 2000..]), 0.0);
        assert!(!dry.is_sounding());
        // ...but with a reverb send the room is still ringing.
        let mut wet = Sequencer::new(sr);
        wet.process(&blip(0.5), &mut buf);
        assert_eq!(wet.active_voices(), 0);
        assert!(wet.is_sounding());
        let tail = &buf[second - 2000..];
        assert!(peak(tail) > 1e-3, "audible tail: {}", peak(tail));
        // A wide tail: left and right differ.
        let side = tail
            .chunks(2)
            .map(|f| (f[0] - f[1]).abs())
            .fold(0.0, f32::max);
        assert!(side > 1e-4, "{side}");
        // A few seconds later it has died away and costs nothing.
        for _ in 0..5 {
            wet.process(&blip(0.5), &mut buf);
        }
        assert!(!wet.is_sounding());
        assert_eq!(peak(&buf), 0.0);
    }

    #[test]
    fn muted_tracks_send_nothing_to_the_reverb() {
        let mut p = blip(1.0);
        p.tracks[0].mute = true;
        let mut seq = Sequencer::new(22_050);
        let mut buf = vec![0.0f32; 2 * 22_050];
        seq.process(&p, &mut buf);
        assert_eq!(peak(&buf), 0.0);
        assert!(!seq.is_sounding());
    }

    #[test]
    fn a_bigger_room_rings_longer_and_the_render_waits_for_it() {
        let small = Reverb {
            size: 0.0,
            tone: 0.5,
        };
        let big = Reverb {
            size: 1.0,
            tone: 0.5,
        };
        let (s, b) = (reverb_tail_seconds(&small), reverb_tail_seconds(&big));
        assert!(s < 1.0 && b > 3.0, "{s} {b}");
        let mut p = blip(0.0);
        p.reverb = big;
        assert_eq!(tail_seconds(&p), RENDER_TAIL_SECONDS);
        p.tracks[0].reverb = 0.3;
        assert_eq!(tail_seconds(&p), RENDER_TAIL_SECONDS + b);
        // By the end of the render the tail has faded to nothing audible.
        let audio = render(&p, 22_050);
        let end = peak(&audio[audio.len() - 2000..]);
        assert!(end < 2e-3, "{end}");
    }

    #[test]
    fn blocks_longer_than_the_send_buffer_are_split() {
        let p = blip(0.5);
        let mut seq = Sequencer::new(22_050);
        let mut buf = vec![0.0f32; 2 * (3 * MAX_BLOCK + 7)];
        seq.process(&p, &mut buf);
        assert!(peak(&buf) > 0.01);
        let beats = buf.len() as f64 / 2.0 / 22_050.0 * 2.0;
        assert!((seq.position() - beats).abs() < 1e-9);
    }

    #[test]
    fn voice_stealing_respects_hard_cap() {
        let p = demo_project();
        let mut seq = Sequencer::new(22_050);
        for i in 0..400 {
            seq.preview_hold(&p, 0, 40 + (i % 40) as u8);
            if let Some(v) = seq.voices.last_mut() {
                v.held = false; // keep them all ringing
            }
        }
        assert!(seq.active_voices() <= HARD_VOICE_CAP);
        let mut buf = vec![0.0f32; 2 * 1024];
        seq.process_voices(&p, &mut buf);
        assert!(buf.iter().all(|s| s.is_finite() && s.abs() < 1.0));
    }

    #[test]
    fn drums_end_at_zero() {
        let mut p = demo_project();
        p.tracks.truncate(1);
        p.tracks[0].instrument = Instrument::Drums {
            kit: DrumKit::Standard,
        };
        let mut seq = Sequencer::new(22_050);
        for n in [36u8, 38, 39, 42, 46, 49, 45] {
            seq.preview(&p, 0, n);
        }
        let mut buf = vec![0.0f32; 2 * 22_050 * 2];
        seq.process_voices(&p, &mut buf);
        assert_eq!(seq.active_voices(), 0);
        assert!(buf.iter().all(|s| s.is_finite()));
        assert!(peak(&buf) > 0.05);
    }

    #[test]
    fn every_preset_sounds_and_stays_clean() {
        let mut p = demo_project();
        p.tracks.truncate(1);
        let sr = 22_050;
        for preset in crate::presets::all() {
            p.tracks[0].instrument = preset.instrument.clone();
            let mut seq = Sequencer::new(sr);
            let notes: &[u8] = if preset.instrument.is_drums() {
                &[
                    35, 36, 37, 38, 39, 42, 46, 49, 51, 53, 54, 55, 56, 60, 63, 64, 67, 70, 75, 76,
                    81,
                ]
            } else {
                &[36, 60, 84]
            };
            for &n in notes {
                seq.preview(&p, 0, n);
            }
            // Long enough for the slowest release (3 s decay + 2.5 s release).
            let mut buf = vec![0.0f32; 2 * sr as usize * 7];
            seq.process_voices(&p, &mut buf);
            assert!(
                buf.iter().all(|s| s.is_finite() && s.abs() < 1.0),
                "{} misbehaves",
                preset.id
            );
            assert!(
                peak(&buf) > 0.02,
                "{} is silent ({})",
                preset.id,
                peak(&buf)
            );
            assert_eq!(seq.active_voices(), 0, "{} never ends", preset.id);
        }
    }

    #[test]
    fn kits_sound_different() {
        let mut p = demo_project();
        p.tracks.truncate(1);
        let render_kit = |p: &mut Project, kit: DrumKit| {
            p.tracks[0].instrument = Instrument::Drums { kit };
            let mut seq = Sequencer::new(22_050);
            seq.preview(p, 0, 36);
            let mut buf = vec![0.0f32; 2 * 4096];
            seq.process_voices(p, &mut buf);
            buf
        };
        let base = render_kit(&mut p, DrumKit::Standard);
        for kit in &DrumKit::ALL[1..] {
            assert_ne!(render_kit(&mut p, *kit), base, "{kit:?}");
        }
    }
}
