//! Sequencer + built-in instruments. The same code drives real-time playback
//! (in `aidaw-engine`) and offline rendering to WAV, so what the AI renders
//! is exactly what the human hears in the GUI.

use std::f32::consts::{PI, TAU};
use std::path::Path;

use anyhow::Result;

use crate::model::{Instrument, Project, Waveform};
use crate::pitch;

const MAX_VOICES: usize = 96;

#[derive(Clone, Copy)]
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
    Drum(Drum),
}

#[derive(Clone, Copy)]
struct Voice {
    track: usize,
    kind: Kind,
    freq: f32,
    phase: f32,
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
    lp: f32,
    hp: f32,
    done: bool,
}

pub struct Sequencer {
    sample_rate: f32,
    /// Playhead in beats.
    position: f64,
    voices: Vec<Voice>,
    noise: u32,
}

impl Sequencer {
    pub fn new(sample_rate: u32) -> Self {
        Sequencer {
            sample_rate: sample_rate as f32,
            position: 0.0,
            voices: Vec::with_capacity(MAX_VOICES),
            noise: 0x1234_5678,
        }
    }

    pub fn position(&self) -> f64 {
        self.position
    }

    pub fn seek(&mut self, beat: f64) {
        self.position = beat.max(0.0);
        self.voices.clear();
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

        let sr = self.sample_rate;
        let audible: Vec<(f32, f32, bool)> = project
            .tracks
            .iter()
            .map(|t| {
                let angle = (t.pan.clamp(-1.0, 1.0) + 1.0) * PI / 4.0;
                let g = t.gain.max(0.0) * std::f32::consts::SQRT_2;
                (angle.cos() * g, angle.sin() * g, project.track_audible(t))
            })
            .collect();

        for voice in self.voices.iter_mut() {
            let Some(&(gl, gr, on)) = audible.get(voice.track) else {
                voice.done = true;
                continue;
            };
            for frame in 0..frames {
                if voice.done {
                    break;
                }
                if voice.delay > 0 {
                    voice.delay -= 1;
                    continue;
                }
                let s = voice.next(sr, &mut self.noise);
                if on {
                    out[frame * 2] += s * gl;
                    out[frame * 2 + 1] += s * gr;
                }
            }
        }
        self.voices.retain(|v| !v.done);

        let master = project.master_gain.max(0.0);
        for s in out.iter_mut() {
            *s = (*s * master).tanh();
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
        let kind = match *inst {
            Instrument::Synth {
                waveform,
                attack,
                decay,
                sustain,
                release,
                cutoff,
            } => Kind::Synth {
                waveform,
                attack: attack.max(0.001),
                decay: decay.max(0.001),
                sustain: sustain.clamp(0.0, 1.0),
                release: release.max(0.005),
                lp_coef: 1.0 - (-TAU * cutoff.clamp(20.0, 20_000.0) / self.sample_rate).exp(),
            },
            Instrument::Drums => Kind::Drum(Drum::from_note(pitch_)),
        };
        if self.voices.len() >= MAX_VOICES {
            // Steal the oldest voice.
            if let Some(i) = (0..self.voices.len()).max_by_key(|&i| self.voices[i].age) {
                self.voices.swap_remove(i);
            }
        }
        self.voices.push(Voice {
            track,
            kind,
            freq: pitch::frequency(pitch_ as f32),
            phase: 0.0,
            vel: (vel.clamp(1, 127) as f32 / 127.0).powf(1.5),
            delay,
            gate,
            age: 0,
            release_from: 0.0,
            released_at: None,
            env: 0.0,
            lp: 0.0,
            hp: 0.0,
            done: false,
        });
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
                let dt = self.freq / sr;
                let p = self.phase;
                let osc = match waveform {
                    Waveform::Sine => (p * TAU).sin(),
                    Waveform::Triangle => 1.0 - 4.0 * (p - 0.5).abs(),
                    Waveform::Saw => 2.0 * p - 1.0 - poly_blep(p, dt),
                    Waveform::Square => {
                        let sq = if p < 0.5 { 1.0 } else { -1.0 };
                        sq + poly_blep(p, dt) - poly_blep((p + 0.5) % 1.0, dt)
                    }
                };
                self.phase = (p + dt) % 1.0;
                self.lp += lp_coef * (osc - self.lp);
                self.lp * self.env * 0.3
            }
            Kind::Drum(drum) => {
                if t >= drum.length() {
                    self.done = true;
                }
                match drum {
                    Drum::Kick => {
                        let f = 45.0 + 105.0 * (-t / 0.04).exp();
                        self.phase = (self.phase + f / sr) % 1.0;
                        (self.phase * TAU).sin() * (-t / 0.18).exp() * 1.1
                    }
                    Drum::Tom(base) => {
                        let f = base * (1.0 + 0.5 * (-t / 0.05).exp());
                        self.phase = (self.phase + f / sr) % 1.0;
                        (self.phase * TAU).sin() * (-t / 0.2).exp() * 0.8
                    }
                    Drum::Snare => {
                        self.phase = (self.phase + 185.0 / sr) % 1.0;
                        let n = self.highpass(noise(rng), 0.25);
                        n * (-t / 0.09).exp() * 0.7
                            + (self.phase * TAU).sin() * (-t / 0.05).exp() * 0.5
                    }
                    Drum::Clap => {
                        let burst = [0.0, 0.01, 0.02]
                            .iter()
                            .map(|o| {
                                if t >= *o {
                                    (-(t - o) / 0.012).exp()
                                } else {
                                    0.0
                                }
                            })
                            .sum::<f32>()
                            + (-t / 0.12).exp() * 0.5;
                        self.highpass(noise(rng), 0.3) * burst * 0.5
                    }
                    Drum::ClosedHat => self.highpass(noise(rng), 0.7) * (-t / 0.025).exp() * 0.45,
                    Drum::OpenHat => self.highpass(noise(rng), 0.7) * (-t / 0.2).exp() * 0.4,
                    Drum::Crash => self.highpass(noise(rng), 0.6) * (-t / 0.6).exp() * 0.35,
                }
            }
        };
        self.age += 1;
        out * self.vel
    }

    /// Crude one-pole high-pass: subtract a low-passed copy.
    fn highpass(&mut self, x: f32, amount: f32) -> f32 {
        self.hp += (1.0 - amount) * (x - self.hp);
        x - self.hp
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
