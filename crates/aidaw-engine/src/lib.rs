//! Real-time playback. Opens the default output device through cpal (WASAPI
//! on Windows) and runs the core [`Sequencer`] inside the audio callback.
//!
//! The UI thread owns the project and pushes a copy here after each edit.
//! The audio thread only ever `try_lock`s, so a busy UI can cause a short
//! dropout but never blocks the callback.

use std::sync::{Arc, Mutex};

use aidaw_core::render::Sequencer;
use aidaw_core::Project;
use anyhow::{anyhow, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

struct Shared {
    project: Project,
    seq: Sequencer,
    playing: bool,
    position: f64,
}

pub struct Engine {
    _stream: cpal::Stream,
    shared: Arc<Mutex<Shared>>,
    sample_rate: u32,
    device_name: String,
}

impl Engine {
    pub fn new(project: &Project) -> Result<Engine> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or_else(|| anyhow!("no audio output device"))?;
        let device_name = device.name().unwrap_or_else(|_| "unknown".into());
        let config = device
            .default_output_config()
            .context("querying output config")?;
        if config.sample_format() != cpal::SampleFormat::F32 {
            return Err(anyhow!(
                "unsupported sample format {:?} (need f32)",
                config.sample_format()
            ));
        }
        let sample_rate = config.sample_rate().0;
        let channels = config.channels() as usize;
        let shared = Arc::new(Mutex::new(Shared {
            project: project.clone(),
            seq: Sequencer::new(sample_rate),
            playing: false,
            position: 0.0,
        }));

        let cb_shared = shared.clone();
        let mut stereo: Vec<f32> = Vec::with_capacity(8192);
        let stream = device.build_output_stream(
            &config.into(),
            move |data: &mut [f32], _| {
                let frames = data.len() / channels;
                let Ok(mut s) = cb_shared.try_lock() else {
                    data.fill(0.0);
                    return;
                };
                if !s.playing {
                    data.fill(0.0);
                    return;
                }
                stereo.resize(frames * 2, 0.0);
                let Shared {
                    project,
                    seq,
                    position,
                    ..
                } = &mut *s;
                seq.process(project, &mut stereo);
                *position = seq.position();
                for (f, out) in data.chunks_mut(channels).enumerate() {
                    for (c, sample) in out.iter_mut().enumerate() {
                        *sample = match (channels, c) {
                            (1, _) => 0.5 * (stereo[f * 2] + stereo[f * 2 + 1]),
                            (_, 0) => stereo[f * 2],
                            (_, 1) => stereo[f * 2 + 1],
                            _ => 0.0,
                        };
                    }
                }
            },
            |err| eprintln!("audio stream error: {err}"),
            None,
        )?;
        stream.play()?;
        Ok(Engine {
            _stream: stream,
            shared,
            sample_rate,
            device_name,
        })
    }

    pub fn device_name(&self) -> &str {
        &self.device_name
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn set_project(&self, project: &Project) {
        if let Ok(mut s) = self.shared.lock() {
            s.project = project.clone();
        }
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

    pub fn is_playing(&self) -> bool {
        self.shared.lock().map(|s| s.playing).unwrap_or(false)
    }

    pub fn position(&self) -> f64 {
        self.shared.lock().map(|s| s.position).unwrap_or(0.0)
    }
}
