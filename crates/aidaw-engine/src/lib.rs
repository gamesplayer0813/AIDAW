//! Real-time playback. Opens an output device through cpal (WASAPI on
//! Windows) and runs the core [`Sequencer`] inside the audio callback.
//!
//! The UI thread owns the project and pushes a copy here after each edit.
//! The audio thread only ever `try_lock`s, so a busy UI can cause a short
//! dropout but never blocks the callback.

use std::f32::consts::TAU;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use aidaw_core::render::Sequencer;
use aidaw_core::Project;
use anyhow::{anyhow, bail, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};

struct Shared {
    project: Project,
    seq: Sequencer,
    playing: bool,
    position: f64,
    /// Cached [`Project::loop_end_beats`]: playback wraps to the start here.
    loop_end: Option<f64>,
}

/// Health counters the UI can show, written lock-free by the audio thread.
#[derive(Default)]
struct Stats {
    callbacks: AtomicU64,
    /// f32 bits of the recent output peak, for a level meter.
    peak: AtomicU32,
    /// Remaining frames of the 440 Hz test tone.
    tone_frames: AtomicU32,
    error: Mutex<Option<String>>,
}

pub struct Diagnostics {
    /// Number of times the audio device has asked for samples. If this stays
    /// at 0 the device is not running at all.
    pub callbacks: u64,
    pub peak: f32,
    pub error: Option<String>,
}

pub struct Engine {
    _stream: cpal::Stream,
    shared: Arc<Mutex<Shared>>,
    stats: Arc<Stats>,
    sample_rate: u32,
    device_name: String,
    format: String,
}

/// Names of all output devices, the system default first.
pub fn output_device_names() -> Vec<String> {
    let host = cpal::default_host();
    let default = host.default_output_device().and_then(|d| d.name().ok());
    let mut names: Vec<String> = host
        .output_devices()
        .map(|it| it.filter_map(|d| d.name().ok()).collect())
        .unwrap_or_default();
    if let Some(d) = &default {
        names.retain(|n| n != d);
        names.insert(0, d.clone());
    }
    names
}

impl Engine {
    /// Open the system default output, falling back to any device that works.
    pub fn new(project: &Project) -> Result<Engine> {
        let mut errors = Vec::new();
        let host = cpal::default_host();
        let mut candidates: Vec<cpal::Device> = host.default_output_device().into_iter().collect();
        if let Ok(devs) = host.output_devices() {
            candidates.extend(devs);
        }
        if candidates.is_empty() {
            bail!("no audio output devices found (host: {:?})", host.id());
        }
        for device in candidates {
            let name = device.name().unwrap_or_else(|_| "unknown".into());
            match Self::open_device(project, device) {
                Ok(e) => return Ok(e),
                Err(e) => errors.push(format!("{name}: {e:#}")),
            }
        }
        Err(anyhow!(
            "could not open any output device:\n{}",
            errors.join("\n")
        ))
    }

    /// Open a specific output device by name.
    pub fn with_device(project: &Project, name: &str) -> Result<Engine> {
        let device = cpal::default_host()
            .output_devices()?
            .find(|d| d.name().is_ok_and(|n| n == name))
            .ok_or_else(|| anyhow!("device {name:?} not found"))?;
        Self::open_device(project, device)
    }

    fn open_device(project: &Project, device: cpal::Device) -> Result<Engine> {
        let device_name = device.name().unwrap_or_else(|_| "unknown".into());
        let config = match device.default_output_config() {
            Ok(c) => c,
            Err(e) => device
                .supported_output_configs()
                .context("listing supported configs")?
                .max_by_key(|c| (c.sample_format() == SampleFormat::F32, c.channels() == 2))
                .map(|c| c.with_max_sample_rate())
                .ok_or_else(|| anyhow!("no usable config: {e}"))?,
        };
        let sample_rate = config.sample_rate().0;
        let format = format!(
            "{:?}, {} ch, {} Hz",
            config.sample_format(),
            config.channels(),
            sample_rate
        );
        let shared = Arc::new(Mutex::new(Shared {
            project: project.clone(),
            seq: Sequencer::new(sample_rate),
            playing: false,
            position: 0.0,
            loop_end: project.loop_end_beats(),
        }));
        let stats = Arc::new(Stats::default());
        let sc: cpal::StreamConfig = config.config();
        let stream = match config.sample_format() {
            SampleFormat::F32 => build::<f32>(&device, &sc, &shared, &stats),
            SampleFormat::I16 => build::<i16>(&device, &sc, &shared, &stats),
            SampleFormat::I32 => build::<i32>(&device, &sc, &shared, &stats),
            SampleFormat::U16 => build::<u16>(&device, &sc, &shared, &stats),
            SampleFormat::F64 => build::<f64>(&device, &sc, &shared, &stats),
            other => bail!("unsupported sample format {other:?}"),
        }
        .with_context(|| format!("opening stream ({format})"))?;
        stream.play().context("starting stream")?;
        Ok(Engine {
            _stream: stream,
            shared,
            stats,
            sample_rate,
            device_name,
            format,
        })
    }

    pub fn device_name(&self) -> &str {
        &self.device_name
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn format(&self) -> &str {
        &self.format
    }

    pub fn diagnostics(&self) -> Diagnostics {
        Diagnostics {
            callbacks: self.stats.callbacks.load(Ordering::Relaxed),
            peak: f32::from_bits(self.stats.peak.load(Ordering::Relaxed)),
            error: self.stats.error.lock().ok().and_then(|e| e.clone()),
        }
    }

    /// Play a one-second 440 Hz beep, independent of the song.
    pub fn test_tone(&self) {
        self.stats
            .tone_frames
            .store(self.sample_rate, Ordering::Relaxed);
    }

    pub fn set_project(&self, project: &Project) {
        // Clone before locking so the audio callback's try_lock isn't starved on big songs.
        let mut fresh = project.clone();
        let loop_end = project.loop_end_beats();
        if let Ok(mut s) = self.shared.lock() {
            std::mem::swap(&mut s.project, &mut fresh);
            s.loop_end = loop_end;
        }
        // The old project is dropped here, outside the lock.
    }

    pub fn play(&self) {
        if let Ok(mut s) = self.shared.lock() {
            s.playing = true;
        }
    }

    pub fn pause(&self) {
        if let Ok(mut s) = self.shared.lock() {
            s.playing = false;
            let pos = s.position;
            s.seq.seek(pos);
        }
    }

    pub fn seek(&self, beat: f64) {
        if let Ok(mut s) = self.shared.lock() {
            s.seq.seek(beat);
            s.position = beat.max(0.0);
        }
    }

    /// Audition a note on a track's instrument (works while stopped).
    pub fn preview(&self, track: usize, pitch: u8) {
        if let Ok(mut s) = self.shared.lock() {
            let Shared { project, seq, .. } = &mut *s;
            seq.preview(project, track, pitch);
        }
    }

    /// Start a note that sounds until [`Self::release`] (holding a piano key).
    pub fn hold(&self, track: usize, pitch: u8) {
        if let Ok(mut s) = self.shared.lock() {
            let Shared { project, seq, .. } = &mut *s;
            seq.preview_hold(project, track, pitch);
        }
    }

    pub fn release(&self) {
        if let Ok(mut s) = self.shared.lock() {
            s.seq.release_held();
        }
    }

    pub fn is_playing(&self) -> bool {
        self.shared.lock().map(|s| s.playing).unwrap_or(false)
    }

    pub fn position(&self) -> f64 {
        self.shared.lock().map(|s| s.position).unwrap_or(0.0)
    }
}

fn build<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    shared: &Arc<Mutex<Shared>>,
    stats: &Arc<Stats>,
) -> Result<cpal::Stream, cpal::BuildStreamError>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = config.channels as usize;
    let sample_rate = config.sample_rate.0 as f32;
    let cb_shared = shared.clone();
    let cb_stats = stats.clone();
    let err_stats = stats.clone();
    // Large enough for any common WASAPI/ALSA period so the callback never allocates.
    let mut stereo: Vec<f32> = Vec::with_capacity(1 << 15);
    let mut tone_phase = 0.0f32;
    let tone_fade = (sample_rate * 0.005).max(1.0);
    let mut peak = 0.0f32;
    device.build_output_stream(
        config,
        move |data: &mut [T], _| {
            cb_stats.callbacks.fetch_add(1, Ordering::Relaxed);
            let frames = data.len() / channels.max(1);
            stereo.clear();
            stereo.resize(frames * 2, 0.0);
            if let Ok(mut s) = cb_shared.try_lock() {
                let Shared {
                    project,
                    seq,
                    position,
                    playing,
                    loop_end,
                } = &mut *s;
                if *playing {
                    seq.process_looping(project, &mut stereo, *loop_end);
                    *position = seq.position();
                } else if seq.active_voices() > 0 {
                    // Stopped, but note previews are still ringing.
                    seq.process_voices(project, &mut stereo);
                }
            }
            let tone = cb_stats.tone_frames.load(Ordering::Relaxed);
            if tone > 0 {
                let n = (tone as usize).min(frames);
                for f in 0..n {
                    // 5 ms fade in/out so the beep starts and stops without clicks.
                    let left = (tone as usize - f) as f32;
                    let elapsed = (sample_rate - left).max(0.0);
                    let ramp = (left.min(elapsed + 1.0) / tone_fade).min(1.0);
                    let s = (tone_phase * TAU).sin() * 0.25 * ramp;
                    tone_phase = (tone_phase + 440.0 / sample_rate) % 1.0;
                    stereo[f * 2] += s;
                    stereo[f * 2 + 1] += s;
                }
                cb_stats
                    .tone_frames
                    .store(tone - n as u32, Ordering::Relaxed);
            }
            // Peak with a slow fall-off for the level meter.
            peak *= 0.8;
            for s in &stereo {
                peak = peak.max(s.abs());
            }
            cb_stats.peak.store(peak.to_bits(), Ordering::Relaxed);
            for (f, out) in data.chunks_mut(channels).enumerate() {
                for (c, sample) in out.iter_mut().enumerate() {
                    let v = match (channels, c) {
                        (1, _) => 0.5 * (stereo[f * 2] + stereo[f * 2 + 1]),
                        (_, 0) => stereo[f * 2],
                        (_, 1) => stereo[f * 2 + 1],
                        _ => 0.0,
                    };
                    // The sequencer already limits; this only guards the test
                    // tone mix and integer conversion against overflow.
                    let v = if v.is_finite() {
                        v.clamp(-1.0, 1.0)
                    } else {
                        0.0
                    };
                    *sample = T::from_sample(v);
                }
            }
        },
        move |err| {
            if let Ok(mut e) = err_stats.error.lock() {
                *e = Some(err.to_string());
            }
        },
        None,
    )
}
