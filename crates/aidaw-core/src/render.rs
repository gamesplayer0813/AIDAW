//! Sequencer + built-in instruments. The same code drives real-time playback
//! (in `aidaw-engine`) and offline rendering to WAV, so what the AI renders
//! is exactly what the human hears in the GUI.
//!
//! Sound-quality and safety notes:
//! - Saw/square oscillators are band-limited with polyBLEP; saws use two
//!   slightly detuned oscillators for a fuller tone, and the synth filter is
//!   a 12 dB/oct low-pass.
//! - Voices never start or stop abruptly: stolen voices, choked hi-hats and
//!   voices cut by a seek/stop fade out over a few milliseconds, and drum
//!   tails are faded before the voice is freed.
//! - Track gain, pan and mute/solo changes are ramped per block (no zipper
//!   noise or clicks).
//! - The master bus runs through a peak limiter and a soft clipper, so
//!   stacking many loud tracks never hard-clips or produces NaNs: the output
//!   is always strictly inside ±1.0.
//! - [`Sequencer::process`] does not allocate (state buffers are reserved up
//!   front), so it is safe to call from a real-time audio callback.

use std::f32::consts::{PI, TAU};
use std::path::Path;

use anyhow::Result;

use crate::model::{Instrument, Project, Waveform};
use crate::pitch;

/// Voices above this count start stealing (with a short fade).
const MAX_VOICES: usize = 96;
/// Absolute cap including voices that are still fading out after a steal.
const HARD_VOICE_CAP: usize = MAX_VOICES + 32;
/// Track gain state is pre-reserved for this many tracks (more still work,
/// but the first block after adding them allocates once).
const RESERVED_TRACKS: usize = 64;
/// Fade time used when a voice is stolen, choked or cut by a seek.
const FADE_SECONDS: f32 = 0.005;
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

#[derive(Clone, Copy, PartialEq)]
enum Drum {
    Kick,
    Snare,
    Clap,
    ClosedHat,
    OpenHat,
    Tom(f32),
    Crash,
}

impl Drum {
    fn from_note(p: u8) -> Drum {
        match p {
            35 | 36 => Drum::Kick,
            38 | 40 => Drum::Snare,
            39 => Drum::Clap,
            42 | 44 => Drum::ClosedHat,
            46 => Drum::OpenHat,
            49 | 51 | 52 | 55 | 57 | 59 => Drum::Crash,
            41 | 43 => Drum::Tom(80.0),
            45 | 47 => Drum::Tom(110.0),
            48 | 50 => Drum::Tom(150.0),
            p => Drum::Tom(pitch::frequency(p as f32).clamp(40.0, 400.0)),
        }
    }

    /// Length in seconds after which the voice is freed.
    fn length(self) -> f32 {
        match self {
            Drum::Kick => 0.6,
            Drum::Snare | Drum::Clap => 0.4,
            Drum::ClosedHat => 0.15,
            Drum::OpenHat => 0.6,
            Drum::Tom(_) => 0.6,
            Drum::Crash => 1.8,
        }
    }

    /// (high-pass, low-pass) corner frequencies of the noise/metal path.
    fn filter_hz(self) -> (f32, f32) {
        match self {
            Drum::Kick => (2_000.0, 8_000.0),
            Drum::Snare => (1_200.0, 9_000.0),
            Drum::Clap => (800.0, 4_000.0),
            Drum::ClosedHat | Drum::OpenHat => (6_500.0, 13_000.0),
            Drum::Crash => (4_000.0, 14_000.0),
            Drum::Tom(_) => (1_000.0, 5_000.0),
        }
    }
}

#[derive(Clone, Copy)]
enum Kind {
    Synth {
        waveform: Waveform,
        attack: f32,
        decay: f32,
        sustain: f32,
        release: f32,
        lp_coef: f32,
    },
    Drum {
        drum: Drum,
        hp_coef: f32,
        lp_coef: f32,
    },
}

#[derive(Clone, Copy)]
struct Voice {
    track: usize,
    kind: Kind,
    freq: f32,
    phase: f32,
    /// Second oscillator (detuned saw) or metallic oscillator bank (cymbals).
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
    /// Filter states (two low-pass poles, two high-pass poles).
    z: [f32; 4],
    done: bool,
    /// A preview note that sounds until [`Sequencer::release_held`].
    held: bool,
    /// Gain of the anti-click fade-out; 1.0 unless `fading`.
    fade: f32,
    fading: bool,
    /// Start fading this many frames into the current block (hi-hat choke).
    fade_at: Option<u32>,
}

#[derive(Clone, Copy, Default)]
struct TrackGain {
    /// Gains reached at the end of the previous block (left, right).
    cur: [f32; 2],
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
    limit_release: f32,
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
            limit_release: (-1.0 / (LIMIT_RELEASE_SECONDS * sample_rate)).exp(),
        }
    }

    pub fn position(&self) -> f64 {
        self.position
    }

    /// Move the playhead. Voices that are still sounding fade out over a few
    /// milliseconds instead of being cut (which would click).
    pub fn seek(&mut self, beat: f64) {
        self.position = beat.max(0.0);
        for v in &mut self.voices {
            if v.delay > 0 {
                // Not started yet: drop silently.
                v.done = true;
            }
            v.fading = true;
            v.held = false;
        }
        self.voices.retain(|v| !v.done);
    }

    pub fn active_voices(&self) -> usize {
        self.voices.len()
    }

    /// Render `out.len() / 2` stereo frames (interleaved L/R) and advance the playhead.
    pub fn process(&mut self, project: &Project, out: &mut [f32]) {
        self.render_block(project, out, true);
    }

    /// Render only the voices already sounding (e.g. note previews) without
    /// triggering clip notes or moving the playhead. Used while stopped.
    pub fn process_voices(&mut self, project: &Project, out: &mut [f32]) {
        self.render_block(project, out, false);
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
        self.spawn(track, &t.instrument, pitch_, 100, 0, u64::MAX);
        if let Some(v) = self.voices.last_mut() {
            v.held = true;
        }
    }

    /// Let go of the held preview note; it fades out with its release time.
    pub fn release_held(&mut self) {
        for v in self.voices.iter_mut().filter(|v| v.held) {
            v.held = false;
            v.gate = v.age;
        }
    }

    fn render_block(&mut self, project: &Project, out: &mut [f32], advance: bool) {
        out.fill(0.0);
        let frames = out.len() / 2;
        if frames == 0 {
            return;
        }
        let frames_per_beat = self.sample_rate as f64 * project.seconds_per_beat();
        let b0 = self.position;
        let b1 = b0 + frames as f64 / frames_per_beat;
        if advance {
            self.spawn_notes(project, b0, b1, frames_per_beat);
        }

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

        for voice in self.voices.iter_mut() {
            let Some(track) = project.tracks.get(voice.track) else {
                voice.done = true;
                continue;
            };
            let target = track_gains(track, any_solo);
            let from = if voice.track >= old_len {
                target
            } else {
                self.gains[voice.track].cur
            };
            let silent = from == [0.0, 0.0] && target == [0.0, 0.0];
            let step = [
                (target[0] - from[0]) * inv_frames,
                (target[1] - from[1]) * inv_frames,
            ];
            for frame in 0..frames {
                if voice.done {
                    break;
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
                    continue;
                }
                let mut s = voice.next(sr, &mut self.noise);
                if voice.fading {
                    voice.fade -= fade_step;
                    if voice.fade <= 0.0 {
                        voice.fade = 0.0;
                        voice.done = true;
                    }
                    s *= voice.fade;
                }
                if !silent {
                    let k = frame as f32;
                    out[frame * 2] += s * (from[0] + step[0] * k);
                    out[frame * 2 + 1] += s * (from[1] + step[1] * k);
                }
            }
        }
        self.voices.retain(|v| !v.done);
        for (g, t) in self.gains.iter_mut().zip(&project.tracks) {
            g.cur = track_gains(t, any_solo);
        }

        // Master: gain -> peak limiter -> soft clipper.
        let master = if project.master_gain.is_finite() {
            project.master_gain.max(0.0)
        } else {
            0.0
        };
        for f in out.chunks_exact_mut(2) {
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

    fn spawn_notes(&mut self, project: &Project, b0: f64, b1: f64, frames_per_beat: f64) {
        for (ti, track) in project.tracks.iter().enumerate() {
            for clip in &track.clips {
                if clip.end() <= b0 || clip.start >= b1 {
                    continue;
                }
                for note in &clip.notes {
                    if note.start < 0.0 || note.start >= clip.length || note.duration <= 0.0 {
                        continue;
                    }
                    let at = clip.start + note.start;
                    if at < b0 || at >= b1 {
                        continue;
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

    fn spawn(
        &mut self,
        track: usize,
        inst: &Instrument,
        pitch_: u8,
        vel: u8,
        delay: u32,
        gate: u64,
    ) {
        let sr = self.sample_rate;
        let kind = match *inst {
            Instrument::Synth {
                waveform,
                attack,
                decay,
                sustain,
                release,
                cutoff,
            } => {
                // Two cascaded one-poles are -3 dB at ~0.644x their corner,
                // so shift the corner up to keep the cutoff where users set it.
                let cutoff = if cutoff.is_finite() { cutoff } else { 20_000.0 };
                let corner = cutoff.clamp(20.0, 20_000.0) / 0.6436;
                Kind::Synth {
                    waveform,
                    attack: finite_or(attack, 0.0).max(0.001),
                    decay: finite_or(decay, 0.0).max(0.001),
                    sustain: finite_or(sustain, 1.0).clamp(0.0, 1.0),
                    release: finite_or(release, 0.0).max(0.005),
                    lp_coef: one_pole_coef(corner, sr),
                }
            }
            Instrument::Drums => {
                let drum = Drum::from_note(pitch_);
                let (hp, lp) = drum.filter_hz();
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
                    hp_coef: one_pole_coef(hp, sr),
                    lp_coef: one_pole_coef(lp, sr),
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
            fade_at: None,
        });
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

/// Target (left, right) gains of a track: equal-power pan, 0 when inaudible.
fn track_gains(t: &crate::model::Track, any_solo: bool) -> [f32; 2] {
    let audible = !t.mute && (!any_solo || t.solo);
    if !audible {
        return [0.0, 0.0];
    }
    let pan = finite_or(t.pan, 0.0).clamp(-1.0, 1.0);
    let angle = (pan + 1.0) * PI / 4.0;
    let g = finite_or(t.gain, 0.0).max(0.0) * std::f32::consts::SQRT_2;
    [angle.cos() * g, angle.sin() * g]
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

impl Voice {
    fn next(&mut self, sr: f32, rng: &mut u32) -> f32 {
        let t = self.age as f32 / sr;
        let out = match self.kind {
            Kind::Synth {
                waveform,
                attack,
                decay,
                sustain,
                release,
                lp_coef,
            } => {
                if self.released_at.is_none() && self.age >= self.gate {
                    self.released_at = Some(self.age);
                    self.release_from = self.env;
                }
                self.env = match self.released_at {
                    None if t < attack => t / attack,
                    None if t < attack + decay => 1.0 - (1.0 - sustain) * (t - attack) / decay,
                    None => sustain,
                    Some(at) => {
                        let rt = (self.age - at) as f32 / sr;
                        if rt >= release {
                            self.done = true;
                        }
                        self.release_from * (1.0 - rt / release).max(0.0)
                    }
                };
                let dt = (self.freq / sr).min(0.5);
                let p = self.phase;
                let osc = match waveform {
                    Waveform::Sine => (p * TAU).sin(),
                    Waveform::Triangle => 1.0 - 4.0 * (p - 0.5).abs(),
                    Waveform::Saw => {
                        let dt2 = (dt * SAW_DETUNE).min(0.5);
                        let p2 = self.phases[0];
                        self.phases[0] = (p2 + dt2) % 1.0;
                        0.62 * (blep_saw(p, dt) + blep_saw(p2, dt2))
                    }
                    Waveform::Square => {
                        let sq = if p < 0.5 { 1.0 } else { -1.0 };
                        sq + poly_blep(p, dt) - poly_blep((p + 0.5) % 1.0, dt)
                    }
                };
                self.phase = (p + dt) % 1.0;
                // 12 dB/oct low-pass: smoother, less buzzy than a single pole.
                self.z[0] += lp_coef * (osc - self.z[0]);
                self.z[1] += lp_coef * (self.z[0] - self.z[1]);
                self.z[1] * self.env * 0.3
            }
            Kind::Drum {
                drum,
                hp_coef,
                lp_coef,
            } => {
                let len = drum.length();
                if t >= len {
                    self.done = true;
                }
                // Fade the tail so voices end at exactly zero (no click).
                let tail = ((len - t) / DRUM_TAIL_SECONDS).clamp(0.0, 1.0);
                let s = match drum {
                    Drum::Kick => {
                        // Fast pitch drop from ~200 Hz to 48 Hz, gently
                        // saturated for punch, with a short noise click.
                        let f = 48.0 + 150.0 * (-t / 0.03).exp() + 40.0 * (-t / 0.006).exp();
                        self.phase = (self.phase + f / sr) % 1.0;
                        let amp = (-t / 0.22).exp();
                        let body = (1.6 * (self.phase * TAU).sin() * amp).tanh() * 0.8;
                        let click = self.band(noise(rng), hp_coef, lp_coef) * (-t / 0.004).exp();
                        body + click * 0.4
                    }
                    Drum::Tom(base) => {
                        let f = base * (1.0 + 0.5 * (-t / 0.05).exp());
                        self.phase = (self.phase + f / sr) % 1.0;
                        let attack = self.band(noise(rng), hp_coef, lp_coef) * (-t / 0.01).exp();
                        (self.phase * TAU).sin() * (-t / 0.2).exp() * 0.8 + attack * 0.25
                    }
                    Drum::Snare => {
                        // Two drum-head modes plus band-passed noise (wires).
                        self.phase = (self.phase + 180.0 / sr) % 1.0;
                        self.phases[0] = (self.phases[0] + 330.0 / sr) % 1.0;
                        let tone = ((self.phase * TAU).sin() + 0.6 * (self.phases[0] * TAU).sin())
                            * (-t / 0.05).exp()
                            * 0.25;
                        let wires = self.band(noise(rng), hp_coef, lp_coef) * (-t / 0.12).exp();
                        tone + wires * 0.8
                    }
                    Drum::Clap => {
                        let mut burst = (-t / 0.12).exp() * 0.5;
                        for o in [0.0f32, 0.01, 0.02] {
                            if t >= o {
                                burst += (-(t - o) / 0.012).exp();
                            }
                        }
                        self.band(noise(rng), hp_coef, lp_coef) * burst * 0.45
                    }
                    Drum::ClosedHat => self.metal(sr, rng, hp_coef, lp_coef) * (-t / 0.03).exp(),
                    Drum::OpenHat => {
                        self.metal(sr, rng, hp_coef, lp_coef) * (-t / 0.22).exp() * 0.85
                    }
                    Drum::Crash => self.metal(sr, rng, hp_coef, lp_coef) * (-t / 0.6).exp() * 0.6,
                };
                s * tail
            }
        };
        self.age += 1;
        out * self.vel
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
    /// band-passed. Less harsh than plain white noise.
    fn metal(&mut self, sr: f32, rng: &mut u32, hp_coef: f32, lp_coef: f32) -> f32 {
        let mut m = 0.0;
        for (p, f) in self.phases.iter_mut().zip(METAL_FREQS) {
            *p = (*p + f * 1.7 / sr) % 1.0;
            m += if *p < 0.5 { 1.0 } else { -1.0 };
        }
        let x = m * (1.0 / 6.0) + noise(rng) * 0.5;
        self.band(x, hp_coef, lp_coef) * 2.2
    }
}

/// Render the whole project (plus a tail for releases) to interleaved stereo.
pub fn render(project: &Project, sample_rate: u32) -> Vec<f32> {
    let tail_seconds = 2.0;
    let total_seconds = project.length_beats() * project.seconds_per_beat() + tail_seconds;
    let total_frames = (total_seconds * sample_rate as f64) as usize;
    let mut seq = Sequencer::new(sample_rate);
    let mut out = vec![0.0f32; total_frames * 2];
    for chunk in out.chunks_mut(512 * 2) {
        seq.process(project, chunk);
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

pub fn render_to_wav(project: &Project, path: &Path, sample_rate: u32) -> Result<f64> {
    let samples = render(project, sample_rate);
    write_wav(path, &samples, sample_rate)?;
    Ok(samples.len() as f64 / 2.0 / sample_rate as f64)
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
        let base = p.tracks.clone();
        for _ in 0..6 {
            for t in &base {
                let mut t = t.clone();
                t.gain = 3.0;
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
    }

    #[test]
    fn silence_after_release_tail() {
        let p = demo_project();
        let sr = 22_050;
        let audio = render(&p, sr);
        // The demo's longest release is 1.2 s; the last 0.5 s of the 2 s tail
        // must be completely silent and no voices may linger.
        let tail = &audio[audio.len() - sr as usize..];
        assert!(peak(tail) == 0.0, "tail peak {}", peak(tail));
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
        p.tracks[0].instrument = Instrument::Drums;
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
}
