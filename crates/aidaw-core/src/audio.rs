//! Sound files behind audio clips: reading WAVs into memory, the pool the
//! renderer plays them from, and file names relative to the project.
//!
//! The project file only names the WAVs (see [`crate::model::AudioRef`]);
//! their samples live in an [`AudioPool`], loaded by whoever has the
//! project's folder (the app, the MCP server) and handed to the
//! [`crate::render::Sequencer`]. A clip whose file is not in the pool is
//! silent, so a missing file never stops the rest of the song.

use std::collections::{BTreeSet, HashMap};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use anyhow::{bail, Context, Result};

use crate::model::Project;

/// Frames summarised by one entry of [`AudioData::peaks`].
pub const PEAK_BLOCK: usize = 256;

/// Longest file loaded into memory (about 1.4 GB of samples for stereo at
/// 48 kHz), so a stray multi-hour file can't exhaust memory.
pub const MAX_SECONDS: f64 = 2.0 * 3600.0;

/// A sound file decoded to 32-bit float, mono or stereo.
#[derive(Debug)]
pub struct AudioData {
    sample_rate: u32,
    channels: usize,
    /// Interleaved samples, `channels` per frame.
    samples: Box<[f32]>,
    /// Loudest sample (any channel) of each [`PEAK_BLOCK`] frames, for
    /// drawing waveforms.
    peaks: Box<[f32]>,
}

impl AudioData {
    /// Wrap interleaved samples. More than two channels keep the first two.
    pub fn new(sample_rate: u32, channels: usize, samples: Vec<f32>) -> AudioData {
        let channels = channels.max(1);
        let samples: Box<[f32]> = if channels > 2 {
            samples
                .chunks_exact(channels)
                .flat_map(|f| [f[0], f[1]])
                .collect()
        } else {
            let whole = samples.len() - samples.len() % channels;
            let mut samples = samples;
            samples.truncate(whole);
            samples.into_boxed_slice()
        };
        let channels = channels.min(2);
        let peaks = samples
            .chunks(PEAK_BLOCK * channels)
            .map(|block| {
                block.iter().fold(
                    0.0f32,
                    |m, s| if s.is_finite() { m.max(s.abs()) } else { m },
                )
            })
            .collect();
        AudioData {
            sample_rate: sample_rate.max(1),
            channels,
            samples,
            peaks,
        }
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// 1 (mono) or 2 (stereo).
    pub fn channels(&self) -> usize {
        self.channels
    }

    pub fn frames(&self) -> usize {
        self.samples.len() / self.channels
    }

    pub fn seconds(&self) -> f64 {
        self.frames() as f64 / self.sample_rate as f64
    }

    pub fn samples(&self) -> &[f32] {
        &self.samples
    }

    pub fn peaks(&self) -> &[f32] {
        &self.peaks
    }

    /// Loudest sample between two times in seconds (0 outside the file).
    pub fn peak_between(&self, from: f64, to: f64) -> f32 {
        let sr = self.sample_rate as f64;
        let block = |t: f64| (t * sr / PEAK_BLOCK as f64).max(0.0) as usize;
        let (a, b) = (block(from), block(to).max(block(from) + 1));
        self.peaks
            .get(a.min(self.peaks.len())..b.min(self.peaks.len()))
            .unwrap_or(&[])
            .iter()
            .fold(0.0, |m: f32, p| m.max(*p))
    }

    /// The (left, right) sample of frame `i`; silence outside the file.
    #[inline]
    pub fn frame(&self, i: isize) -> (f32, f32) {
        if i < 0 {
            return (0.0, 0.0);
        }
        let i = i as usize;
        if self.channels == 1 {
            let s = self.samples.get(i).copied().unwrap_or(0.0);
            (s, s)
        } else {
            match self.samples.get(2 * i..2 * i + 2) {
                Some(f) => (f[0], f[1]),
                None => (0.0, 0.0),
            }
        }
    }

    /// The stereo frame at a fractional position, with 4-point Hermite
    /// interpolation (used when the file's sample rate differs from the
    /// output's, or the position falls between frames).
    #[inline]
    pub fn frame_at(&self, pos: f64) -> (f32, f32) {
        let i = pos.floor();
        let t = (pos - i) as f32;
        let i = i as isize;
        if t == 0.0 {
            return self.frame(i);
        }
        let (a0, b0) = self.frame(i - 1);
        let (a1, b1) = self.frame(i);
        let (a2, b2) = self.frame(i + 1);
        let (a3, b3) = self.frame(i + 2);
        (hermite(a0, a1, a2, a3, t), hermite(b0, b1, b2, b3, t))
    }
}

#[inline]
fn hermite(y0: f32, y1: f32, y2: f32, y3: f32, t: f32) -> f32 {
    let c1 = 0.5 * (y2 - y0);
    let c2 = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
    let c3 = 0.5 * (y3 - y0) + 1.5 * (y1 - y2);
    ((c3 * t + c2) * t + c1) * t + y1
}

/// Read a WAV file (8/16/24/32-bit integer or 32-bit float, any number of
/// channels; only the first two are kept).
pub fn load_wav(path: &Path) -> Result<AudioData> {
    let reader =
        hound::WavReader::open(path).with_context(|| format!("opening {}", path.display()))?;
    let spec = reader.spec();
    let channels = spec.channels as usize;
    if channels == 0 {
        bail!("{} has no audio channels", path.display());
    }
    let seconds = reader.duration() as f64 / spec.sample_rate.max(1) as f64;
    if seconds > MAX_SECONDS {
        bail!(
            "{} is {:.0} minutes long; audio clips can use files of up to {:.0} minutes",
            path.display(),
            seconds / 60.0,
            MAX_SECONDS / 60.0
        );
    }
    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader
            .into_samples::<f32>()
            .collect::<Result<_, _>>()
            .with_context(|| format!("reading {}", path.display()))?,
        hound::SampleFormat::Int => {
            let scale = 1.0 / (1u64 << (spec.bits_per_sample.clamp(1, 32) - 1)) as f32;
            reader
                .into_samples::<i32>()
                .map(|s| s.map(|s| s as f32 * scale))
                .collect::<Result<_, _>>()
                .with_context(|| format!("reading {}", path.display()))?
        }
    };
    Ok(AudioData::new(spec.sample_rate, channels, samples))
}

/// Length of a WAV file in seconds, read from its header only.
pub fn wav_seconds(path: &Path) -> Result<f64> {
    let reader =
        hound::WavReader::open(path).with_context(|| format!("opening {}", path.display()))?;
    let spec = reader.spec();
    Ok(reader.duration() as f64 / spec.sample_rate.max(1) as f64)
}

/// Where a clip's `file` is on disk: relative names are taken from the
/// project's folder (`base`).
pub fn resolve(base: Option<&Path>, file: &str) -> PathBuf {
    let path = PathBuf::from(file);
    if path.is_absolute() {
        return path;
    }
    match base {
        Some(dir) => dir.join(path),
        None => path,
    }
}

/// How a clip should name `path`: relative to the project folder `base`
/// (with `/`, so the project opens on any system) when the file is inside
/// it, else the absolute path.
pub fn file_name_for(base: Option<&Path>, path: &Path) -> String {
    if let Some(rel) = base.and_then(|b| path.strip_prefix(b).ok()) {
        let parts: Vec<String> = rel
            .components()
            .filter_map(|c| match c {
                Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
                _ => None,
            })
            .collect();
        if !parts.is_empty() && parts.len() == rel.components().count() {
            return parts.join("/");
        }
    }
    path.to_string_lossy().into_owned()
}

/// The files the project's audio clips play, each once.
pub fn files_used(project: &Project) -> BTreeSet<&str> {
    project
        .tracks
        .iter()
        .flat_map(|t| t.clips.iter())
        .filter_map(|c| c.audio.as_ref().map(|a| a.file.as_str()))
        .collect()
}

/// The decoded files a song plays, by the name its clips use.
#[derive(Debug, Default, Clone)]
pub struct AudioPool {
    files: HashMap<String, Arc<AudioData>>,
}

impl AudioPool {
    pub fn new() -> AudioPool {
        AudioPool::default()
    }

    /// Load every file `project` plays, from the project's folder `base`.
    /// Files that can't be read are left out (their clips stay silent) and
    /// reported.
    pub fn load(project: &Project, base: Option<&Path>) -> (AudioPool, Vec<String>) {
        let mut pool = AudioPool::new();
        let problems = pool.load_missing(project, base);
        (pool, problems)
    }

    /// Load the files `project` plays that the pool doesn't have yet.
    pub fn load_missing(&mut self, project: &Project, base: Option<&Path>) -> Vec<String> {
        let mut problems = Vec::new();
        for file in files_used(project) {
            if self.files.contains_key(file) {
                continue;
            }
            match load_wav(&resolve(base, file)) {
                Ok(data) => {
                    self.files.insert(file.to_string(), Arc::new(data));
                }
                Err(e) => problems.push(format!("{e:#}")),
            }
        }
        problems
    }

    pub fn get(&self, file: &str) -> Option<&Arc<AudioData>> {
        self.files.get(file)
    }

    pub fn contains(&self, file: &str) -> bool {
        self.files.contains_key(file)
    }

    pub fn insert(&mut self, file: impl Into<String>, data: Arc<AudioData>) {
        self.files.insert(file.into(), data);
    }

    /// Forget the files `project` no longer plays.
    pub fn retain_used(&mut self, project: &Project) {
        let used = files_used(project);
        self.files.retain(|f, _| used.contains(f.as_str()));
    }

    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    pub fn files(&self) -> impl Iterator<Item = &str> {
        self.files.keys().map(String::as_str)
    }
}

/// Write interleaved float samples as a 24-bit WAV (tests and tools).
pub fn write_wav_24(path: &Path, sample_rate: u32, channels: u16, samples: &[f32]) -> Result<()> {
    let spec = hound::WavSpec {
        channels,
        sample_rate,
        bits_per_sample: 24,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(path, spec)
        .with_context(|| format!("creating {}", path.display()))?;
    for s in samples {
        w.write_sample(to_i24(*s))?;
    }
    w.finalize()?;
    Ok(())
}

/// A float sample as a 24-bit integer, clipped to full scale.
#[inline]
pub fn to_i24(s: f32) -> i32 {
    let s = if s.is_finite() {
        s.clamp(-1.0, 1.0)
    } else {
        0.0
    };
    (s * 8_388_607.0).round() as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("aidaw-audio-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    #[test]
    fn wav_files_load_in_every_common_format() {
        let ramp: Vec<f32> = (0..1000).map(|i| (i as f32 / 1000.0) - 0.5).collect();
        // 24-bit stereo.
        let p24 = tmp("ramp24.wav");
        write_wav_24(&p24, 48_000, 2, &ramp).unwrap();
        let a = load_wav(&p24).unwrap();
        assert_eq!(
            (a.sample_rate(), a.channels(), a.frames()),
            (48_000, 2, 500)
        );
        assert!((a.frame(10).0 - ramp[20]).abs() < 1e-6);
        assert!((a.frame(10).1 - ramp[21]).abs() < 1e-6);
        // 16-bit mono and 32-bit float mono.
        for (bits, format) in [
            (16, hound::SampleFormat::Int),
            (32, hound::SampleFormat::Float),
        ] {
            let p = tmp(&format!("ramp{bits}.wav"));
            let spec = hound::WavSpec {
                channels: 1,
                sample_rate: 22_050,
                bits_per_sample: bits,
                sample_format: format,
            };
            let mut w = hound::WavWriter::create(&p, spec).unwrap();
            for s in &ramp {
                match format {
                    hound::SampleFormat::Int => w.write_sample((s * 32767.0) as i16).unwrap(),
                    hound::SampleFormat::Float => w.write_sample(*s).unwrap(),
                }
            }
            w.finalize().unwrap();
            let a = load_wav(&p).unwrap();
            assert_eq!((a.channels(), a.frames()), (1, 1000));
            let (l, r) = a.frame(700);
            assert!((l - ramp[700]).abs() < 1e-3 && l == r, "{bits}: {l}");
            assert!((wav_seconds(&p).unwrap() - 1000.0 / 22_050.0).abs() < 1e-9);
        }
        assert!(load_wav(&tmp("missing.wav")).is_err());
    }

    #[test]
    fn peaks_and_interpolation() {
        let mut s = vec![0.0f32; 4 * PEAK_BLOCK];
        s[PEAK_BLOCK + 3] = -0.75;
        let a = AudioData::new(1000, 1, s);
        assert_eq!(a.peaks(), &[0.0, 0.75, 0.0, 0.0]);
        let block = PEAK_BLOCK as f64 / 1000.0;
        assert_eq!(a.peak_between(block, 2.0 * block), 0.75);
        assert_eq!(a.peak_between(2.0 * block, 4.0 * block), 0.0);
        // Interpolation passes through the samples and is smooth between them.
        let line = AudioData::new(1000, 1, (0..10).map(|i| i as f32 * 0.1).collect());
        assert!((line.frame_at(3.0).0 - 0.3).abs() < 1e-6);
        assert!((line.frame_at(3.5).0 - 0.35).abs() < 1e-6);
        assert_eq!(line.frame(-1), (0.0, 0.0));
        assert_eq!(line.frame(10), (0.0, 0.0));
    }

    #[test]
    fn more_than_two_channels_keep_the_first_two() {
        let a = AudioData::new(8000, 4, vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0]);
        assert_eq!((a.channels(), a.frames()), (2, 2));
        assert_eq!(a.frame(1), (5.0, 6.0));
    }

    #[test]
    fn file_names_are_relative_to_the_project_when_inside_it() {
        let base = Path::new("/music/songs");
        assert_eq!(
            file_name_for(
                Some(base),
                Path::new("/music/songs/My Song audio/take 1.wav")
            ),
            "My Song audio/take 1.wav"
        );
        assert_eq!(
            file_name_for(Some(base), Path::new("/samples/kick.wav")),
            "/samples/kick.wav"
        );
        assert_eq!(
            resolve(Some(base), "My Song audio/take 1.wav"),
            Path::new("/music/songs/My Song audio/take 1.wav")
        );
        assert_eq!(
            resolve(Some(base), "/samples/kick.wav"),
            Path::new("/samples/kick.wav")
        );
    }
}
