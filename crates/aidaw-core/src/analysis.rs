//! The song as numbers, so an AI can check a mix without ears: loudness in
//! LUFS (ITU-R BS.1770 with EBU R128 gating), peak level, how hard the
//! master limiter works, the balance of lows, mids and highs, the loudness
//! of each stretch of bars, and each track played on its own.

use anyhow::Result;

use crate::model::Project;
use crate::render::{self, Sequencer};

/// Sample rate the analysis renders at.
const SAMPLE_RATE: u32 = 44_100;
/// Frames rendered per block; the limiter is read once per block.
const BLOCK: usize = 512;
/// Below this a sample counts as silence (-60 dBFS).
const SILENCE: f32 = 0.001;
/// Below this the gated loudness counts as silence (EBU R128 absolute gate).
const ABSOLUTE_GATE: f64 = -70.0;

/// Loudness and peak of a mix or of one track.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Levels {
    /// Integrated loudness in LUFS, `None` when silent.
    pub lufs: Option<f64>,
    /// Highest sample in dBFS, `None` when silent.
    pub peak_db: Option<f64>,
}

/// Loudness of a stretch of bars.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Section {
    /// First and last bar (1-based).
    pub bars: (u32, u32),
    /// Average loudness in LUFS (not gated), `None` when silent.
    pub lufs: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Analysis {
    /// Length of the song without the release tail, in seconds.
    pub seconds: f64,
    pub mix: Levels,
    /// The most the master limiter turned the song down, in dB.
    pub limiter_db: f64,
    /// Share of the song (0..1) during which the limiter cut more than 1 dB.
    pub limiter_busy: f64,
    /// Share of the sound's energy below 250 Hz, from 250 Hz to 4 kHz and
    /// above 4 kHz; all zero when silent.
    pub bands: [f64; 3],
    pub sections: Vec<Section>,
    /// When the first sound starts, in seconds; `None` when silent.
    pub first_sound: Option<f64>,
    /// Each track played alone, by name; `None` for tracks that aren't heard
    /// in the mix (muted, or silenced by another track's solo).
    pub tracks: Vec<(String, Option<Levels>)>,
}

/// Bars per section: 4, or more for long songs, so there are at most 16.
pub fn section_bars(total_bars: u32) -> u32 {
    let mut bars = 4;
    while total_bars.div_ceil(bars) > 16 {
        bars *= 2;
    }
    bars
}

/// Render the song (nothing is written to disk) and measure it, and each
/// audible track alone. The renders run in parallel.
pub fn analyze(project: &Project) -> Result<Analysis> {
    render::check_render_args(project, SAMPLE_RATE)?;
    let solos: Vec<(usize, Project)> = project
        .tracks
        .iter()
        .enumerate()
        .filter(|(_, t)| project.track_audible(t))
        .map(|(i, _)| {
            let mut alone = project.clone();
            for (j, t) in alone.tracks.iter_mut().enumerate() {
                t.solo = i == j;
            }
            (i, alone)
        })
        .collect();
    let (mix, alone) = std::thread::scope(|s| {
        let mix = s.spawn(|| measure(project, true));
        let alone: Vec<_> = solos
            .iter()
            .map(|(i, p)| (*i, s.spawn(move || measure(p, false))))
            .collect();
        let alone: Vec<(usize, Measured)> = alone
            .into_iter()
            .map(|(i, h)| (i, h.join().expect("analysis thread")))
            .collect();
        (mix.join().expect("analysis thread"), alone)
    });
    let tracks = project
        .tracks
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let levels = alone.iter().find(|(j, _)| *j == i).map(|(_, m)| m.levels);
            (t.name.clone(), levels)
        })
        .collect();
    Ok(Analysis {
        seconds: mix.seconds,
        mix: mix.levels,
        limiter_db: mix.limiter_db,
        limiter_busy: mix.limiter_busy,
        bands: mix.bands,
        sections: mix.sections,
        first_sound: mix.first_sound,
        tracks,
    })
}

struct Measured {
    seconds: f64,
    levels: Levels,
    limiter_db: f64,
    limiter_busy: f64,
    bands: [f64; 3],
    sections: Vec<Section>,
    first_sound: Option<f64>,
}

/// Render `project` block by block and measure it; `detail` adds the band
/// balance and the sections.
fn measure(project: &Project, detail: bool) -> Measured {
    let sr = SAMPLE_RATE as f64;
    let song_seconds = project.length_beats() * project.seconds_per_beat();
    let total = ((song_seconds + render::tail_seconds(project)) * sr) as usize;
    let song_frames = ((song_seconds * sr) as usize).min(total);
    let frames_per_beat = sr * project.seconds_per_beat();
    let bar_beats = f64::from(project.beats_per_bar.max(1));
    let total_bars = (project.length_beats() / bar_beats - 1e-9).ceil().max(0.0) as u32;
    let per_section = section_bars(total_bars);
    let section_frames = per_section as f64 * bar_beats * frames_per_beat;
    let mut section_sums = vec![(0.0f64, 0usize); total_bars.div_ceil(per_section) as usize];

    let mut seq = Sequencer::new(SAMPLE_RATE);
    let mut block = vec![0.0f32; BLOCK * 2];
    let mut loudness = Loudness::new(sr);
    let mut bands = Bands::new(sr);
    let mut peak = 0.0f32;
    let mut first_sound = None;
    let (mut limiter_db, mut busy_blocks, mut song_blocks) = (0.0f64, 0usize, 0usize);
    let mut done = 0;
    while done < total {
        let n = (total - done).min(BLOCK);
        let out = &mut block[..n * 2];
        seq.process(project, out);
        let cut = seq.take_limiter_reduction_db() as f64;
        limiter_db = limiter_db.max(cut);
        if done < song_frames {
            song_blocks += 1;
            busy_blocks += (cut > 1.0) as usize;
        }
        for (k, &[l, r]) in out.as_chunks::<2>().0.iter().enumerate() {
            let frame = done + k;
            let loud = l.abs().max(r.abs());
            peak = peak.max(loud);
            if first_sound.is_none() && loud > SILENCE {
                first_sound = Some(frame as f64 / sr);
            }
            let energy = loudness.push(l as f64, r as f64);
            if detail {
                bands.push((l as f64 + r as f64) * 0.5);
                if frame < song_frames {
                    let s = (frame as f64 / section_frames) as usize;
                    if let Some(sum) = section_sums.get_mut(s) {
                        sum.0 += energy;
                        sum.1 += 1;
                    }
                }
            }
        }
        done += n;
    }

    let sections = section_sums
        .iter()
        .enumerate()
        .map(|(i, &(energy, frames))| {
            let first = i as u32 * per_section + 1;
            Section {
                bars: (first, (first + per_section - 1).min(total_bars)),
                lufs: lufs(energy / frames.max(1) as f64),
            }
        })
        .collect();
    Measured {
        seconds: song_seconds,
        levels: Levels {
            lufs: loudness.integrated(),
            peak_db: (peak > 0.0).then(|| 20.0 * f64::from(peak).log10()),
        },
        limiter_db,
        limiter_busy: busy_blocks as f64 / song_blocks.max(1) as f64,
        bands: bands.shares(),
        sections,
        first_sound,
    }
}

/// Loudness of a mean square (summed over both channels), or `None` below
/// the absolute gate.
fn lufs(mean_square: f64) -> Option<f64> {
    let l = -0.691 + 10.0 * mean_square.log10();
    (l.is_finite() && l > ABSOLUTE_GATE).then_some(l)
}

/// A second-order filter (transposed direct form II).
#[derive(Clone, Copy)]
struct Biquad {
    b: [f64; 3],
    a: [f64; 2],
    z: [f64; 2],
}

impl Biquad {
    fn new(b: [f64; 3], a: [f64; 3]) -> Self {
        Biquad {
            b: [b[0] / a[0], b[1] / a[0], b[2] / a[0]],
            a: [a[1] / a[0], a[2] / a[0]],
            z: [0.0; 2],
        }
    }

    /// Butterworth low- or high-pass at `hz` (RBJ cookbook, Q = 1/sqrt 2).
    fn pass(hz: f64, sr: f64, high: bool) -> Self {
        let w = std::f64::consts::TAU * hz / sr;
        let alpha = w.sin() / std::f64::consts::SQRT_2;
        let c = w.cos();
        let b = if high {
            [(1.0 + c) / 2.0, -(1.0 + c), (1.0 + c) / 2.0]
        } else {
            [(1.0 - c) / 2.0, 1.0 - c, (1.0 - c) / 2.0]
        };
        Biquad::new(b, [1.0 + alpha, -2.0 * c, 1.0 - alpha])
    }

    fn run(&mut self, x: f64) -> f64 {
        let y = self.b[0] * x + self.z[0];
        self.z[0] = self.b[1] * x - self.a[0] * y + self.z[1];
        self.z[1] = self.b[2] * x - self.a[1] * y;
        y
    }
}

/// The K-weighting of BS.1770: a shelf that lifts the highs by 4 dB, as
/// the head does, then a high-pass that ignores rumble.
fn k_weighting(sr: f64) -> [Biquad; 2] {
    let k = (std::f64::consts::PI * 1681.974450955533 / sr).tan();
    let (q, vh) = (0.7071752369554196, 10f64.powf(3.999843853973347 / 20.0));
    let vb = vh.powf(0.4996667741545416);
    let shelf = Biquad::new(
        [
            vh + vb * k / q + k * k,
            2.0 * (k * k - vh),
            vh - vb * k / q + k * k,
        ],
        [
            1.0 + k / q + k * k,
            2.0 * (k * k - 1.0),
            1.0 - k / q + k * k,
        ],
    );
    let k = (std::f64::consts::PI * 38.13547087602444 / sr).tan();
    let q = 0.5003270373238773;
    let high_pass = Biquad::new(
        [1.0, -2.0, 1.0],
        [
            1.0 + k / q + k * k,
            2.0 * (k * k - 1.0),
            1.0 - k / q + k * k,
        ],
    );
    [shelf, high_pass]
}

/// Gated integrated loudness: 400 ms blocks every 100 ms.
struct Loudness {
    filters: [[Biquad; 2]; 2],
    hop: usize,
    in_hop: usize,
    hop_energy: f64,
    /// Mean square of each finished 100 ms hop.
    hops: Vec<f64>,
}

impl Loudness {
    fn new(sr: f64) -> Self {
        let k = k_weighting(sr);
        Loudness {
            filters: [k, k],
            hop: (sr / 10.0).round() as usize,
            in_hop: 0,
            hop_energy: 0.0,
            hops: Vec::new(),
        }
    }

    /// Add a frame; returns its K-weighted energy (both channels).
    fn push(&mut self, l: f64, r: f64) -> f64 {
        let mut energy = 0.0;
        for (x, [shelf, hp]) in [l, r].into_iter().zip(&mut self.filters) {
            let y = hp.run(shelf.run(x));
            energy += y * y;
        }
        self.hop_energy += energy;
        self.in_hop += 1;
        if self.in_hop == self.hop {
            self.hops.push(self.hop_energy / self.hop as f64);
            self.in_hop = 0;
            self.hop_energy = 0.0;
        }
        energy
    }

    fn integrated(&self) -> Option<f64> {
        let blocks: Vec<f64> = self
            .hops
            .windows(4)
            .map(|w| w.iter().sum::<f64>() / 4.0)
            .collect();
        let mean_above = |gate: f64| {
            let kept: Vec<f64> = blocks
                .iter()
                .copied()
                .filter(|&ms| lufs(ms).is_some_and(|l| l > gate))
                .collect();
            (!kept.is_empty()).then(|| kept.iter().sum::<f64>() / kept.len() as f64)
        };
        let ungated = mean_above(ABSOLUTE_GATE)?;
        lufs(mean_above(lufs(ungated)? - 10.0)?)
    }
}

/// Energy below 250 Hz, between 250 Hz and 4 kHz and above 4 kHz.
struct Bands {
    low: Biquad,
    mid: [Biquad; 2],
    high: Biquad,
    energy: [f64; 3],
}

impl Bands {
    fn new(sr: f64) -> Self {
        Bands {
            low: Biquad::pass(250.0, sr, false),
            mid: [
                Biquad::pass(250.0, sr, true),
                Biquad::pass(4000.0, sr, false),
            ],
            high: Biquad::pass(4000.0, sr, true),
            energy: [0.0; 3],
        }
    }

    fn push(&mut self, x: f64) {
        let low = self.low.run(x);
        let above_lows = self.mid[0].run(x);
        let mid = self.mid[1].run(above_lows);
        let high = self.high.run(x);
        for (e, y) in self.energy.iter_mut().zip([low, mid, high]) {
            *e += y * y;
        }
    }

    fn shares(&self) -> [f64; 3] {
        let total: f64 = self.energy.iter().sum();
        if total > 0.0 {
            self.energy.map(|e| e / total)
        } else {
            [0.0; 3]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Clip, Note, Track};

    fn demo() -> Project {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/demo.aidaw.json");
        crate::io::load(&path).expect("demo project loads")
    }

    /// A 997 Hz sine at -20 dBFS on both channels reads -20 LUFS: the
    /// reference point of BS.1770 (K-weighting is 0 dB around 1 kHz, and
    /// two channels add 3 dB to the -23 of one).
    #[test]
    fn a_sine_reads_its_textbook_loudness() {
        let sr = 48_000.0;
        let mut meter = Loudness::new(sr);
        let amp = 10f64.powf(-20.0 / 20.0);
        for i in 0..(sr as usize * 5) {
            let x = amp * (std::f64::consts::TAU * 997.0 * i as f64 / sr).sin();
            meter.push(x, x);
        }
        let l = meter.integrated().unwrap();
        assert!((l - -20.0).abs() < 0.1, "{l}");
        assert_eq!(Loudness::new(sr).integrated(), None, "silence");
    }

    #[test]
    fn bands_hear_low_and_high_tones() {
        let sr = 44_100.0;
        let tone = |hz: f64| {
            let mut b = Bands::new(sr);
            for i in 0..sr as usize {
                b.push((std::f64::consts::TAU * hz * i as f64 / sr).sin());
            }
            b.shares()
        };
        assert!(tone(60.0)[0] > 0.9);
        assert!(tone(1000.0)[1] > 0.8);
        assert!(tone(10_000.0)[2] > 0.9);
    }

    #[test]
    fn the_demo_measures_sensibly() {
        let p = demo();
        let a = analyze(&p).unwrap();
        let lufs = a.mix.lufs.unwrap();
        assert!((-30.0..-8.0).contains(&lufs), "{lufs}");
        let peak = a.mix.peak_db.unwrap();
        assert!(peak < 0.0 && peak > -20.0, "{peak}");
        assert!((a.bands.iter().sum::<f64>() - 1.0).abs() < 1e-9);
        assert_eq!(a.tracks.len(), p.tracks.len());
        for (name, levels) in &a.tracks {
            let l = levels.unwrap().lufs.unwrap();
            assert!(
                l < lufs + 0.5,
                "{name} alone ({l}) is louder than the whole mix ({lufs})"
            );
        }
        assert_eq!(
            a.sections.len(),
            2,
            "32 beats = 8 bars = two 4-bar sections"
        );
        assert_eq!(a.sections[1].bars, (5, 8));
        assert_eq!(a.first_sound.map(|s| s < 0.05), Some(true));
    }

    #[test]
    fn a_squashed_mix_shows_the_limiter_working() {
        let mut p = demo();
        let calm = analyze(&p).unwrap();
        p.master_gain = 2.0;
        for t in &mut p.tracks {
            t.gain = 2.0;
        }
        let hot = analyze(&p).unwrap();
        assert!(
            hot.limiter_db > calm.limiter_db + 3.0,
            "{} vs {}",
            hot.limiter_db,
            calm.limiter_db
        );
        assert!(hot.limiter_busy > 0.5, "{}", hot.limiter_busy);
        assert!(hot.mix.peak_db.unwrap() < 0.0, "still never clips");
    }

    #[test]
    fn silent_and_muted_parts_are_reported_as_such() {
        let mut p = Project::new("quiet", 120.0);
        p.tracks.push(Track {
            id: "t1".into(),
            name: "Muted".into(),
            instrument: crate::Instrument::default_synth(),
            gain: 0.8,
            pan: 0.0,
            mute: true,
            solo: false,
            reverb: 0.0,
            clips: vec![Clip {
                id: "c1".into(),
                name: String::new(),
                start: 4.0,
                length: 4.0,
                notes: vec![Note {
                    pitch: 60,
                    start: 0.0,
                    duration: 1.0,
                    velocity: 100,
                }],
            }],
        });
        let a = analyze(&p).unwrap();
        assert_eq!(a.mix.lufs, None);
        assert_eq!(a.mix.peak_db, None);
        assert_eq!(a.first_sound, None);
        assert_eq!(a.tracks, [("Muted".to_string(), None)]);
        assert_eq!(a.bands, [0.0; 3]);
        assert_eq!(
            a.sections,
            [Section {
                bars: (1, 2),
                lufs: None
            }]
        );
    }

    #[test]
    fn long_songs_get_longer_sections() {
        assert_eq!(section_bars(8), 4);
        assert_eq!(section_bars(64), 4);
        assert_eq!(section_bars(65), 8);
        assert_eq!(section_bars(400), 32);
    }
}
