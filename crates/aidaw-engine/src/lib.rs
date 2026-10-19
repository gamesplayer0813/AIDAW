//! Real-time playback. Opens an output device through cpal (WASAPI on
//! Windows) and runs the core [`Sequencer`] inside the audio callback.
//!
//! The audio callback never waits for the UI, and the UI never waits for
//! the audio callback:
//! - The UI sends commands (play, seek, note previews, the latest project)
//!   through a bounded queue. Neither side ever blocks on it (only
//!   `try_send` / `try_recv`), so it never takes a lock either.
//! - The playhead comes back through atomics.
//! - Projects travel as `Arc` snapshots. The callback hands every snapshot
//!   it replaces back to the UI thread, which frees it, so the callback
//!   never frees (or allocates) memory.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::f32::consts::TAU;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};

use aidaw_core::render::Sequencer;
use aidaw_core::Project;
use anyhow::{anyhow, bail, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};

/// Commands that fit between two audio callbacks. Far more than a person
/// can trigger in 10 ms; the rest wait in the UI-side backlog.
const QUEUE_LEN: usize = 256;
/// Replaced project snapshots on their way back to the UI thread.
const RETIRED_LEN: usize = 64;
/// Snapshots the callback can hold on to when the UI is slow to collect them.
const GRAVEYARD_LEN: usize = 64;
/// Commands kept while the device has stalled (beyond the queue).
const MAX_BACKLOG: usize = 4096;

enum Command {
    SetProject {
        project: Arc<Project>,
        loop_end: Option<f64>,
    },
    Play,
    Pause,
    Seek {
        beat: f64,
        id: u64,
    },
    Preview {
        track: usize,
        pitch: u8,
    },
    Hold {
        track: usize,
        pitch: u8,
    },
    Release,
}

/// What the audio thread reports back, lock-free.
#[derive(Default)]
struct Transport {
    /// f64 bits of the playhead, in beats.
    position: AtomicU64,
    /// Id of the last seek the audio thread has carried out.
    seek_done: AtomicU64,
}

/// The audio callback's side: owns the sequencer and the current project.
/// Kept apart from cpal so tests can drive it the way a sound card would.
struct Player {
    commands: Receiver<Command>,
    retired: SyncSender<Arc<Project>>,
    /// Snapshots that did not fit in `retired` yet (capacity reserved).
    graveyard: Vec<Arc<Project>>,
    transport: Arc<Transport>,
    project: Arc<Project>,
    /// Cached [`Project::loop_end_beats`]: playback wraps to the start here.
    loop_end: Option<f64>,
    seq: Sequencer,
    playing: bool,
}

impl Player {
    /// Apply the queued commands, then render `out` (interleaved stereo).
    fn render(&mut self, out: &mut [f32]) {
        self.drain_commands();
        if self.playing {
            self.seq.process_looping(&self.project, out, self.loop_end);
            self.transport
                .position
                .store(self.seq.position().to_bits(), Ordering::Release);
        } else if self.seq.active_voices() > 0 {
            // Stopped, but note previews are still ringing.
            self.seq.process_voices(&self.project, out);
        } else {
            out.fill(0.0);
        }
    }

    fn drain_commands(&mut self) {
        // Hand back snapshots that did not fit last time.
        while let Some(old) = self.graveyard.pop() {
            if let Err(TrySendError::Full(old)) = self.retired.try_send(old) {
                self.graveyard.push(old);
                break;
            }
        }
        while let Ok(cmd) = self.commands.try_recv() {
            match cmd {
                Command::SetProject { project, loop_end } => {
                    let old = std::mem::replace(&mut self.project, project);
                    self.loop_end = loop_end;
                    self.retire(old);
                }
                Command::Play => self.playing = true,
                Command::Pause => {
                    self.playing = false;
                    // Fade out whatever is sounding; playing again picks up
                    // the notes held across this point.
                    let pos = self.seq.position();
                    self.seq.seek(pos);
                }
                Command::Seek { beat, id } => {
                    self.seq.seek(beat);
                    self.transport
                        .position
                        .store(self.seq.position().to_bits(), Ordering::Release);
                    self.transport.seek_done.store(id, Ordering::Release);
                }
                Command::Preview { track, pitch } => self.seq.preview(&self.project, track, pitch),
                Command::Hold { track, pitch } => {
                    self.seq.preview_hold(&self.project, track, pitch)
                }
                Command::Release => self.seq.release_held(),
            }
        }
    }

    /// Send a replaced snapshot to the UI thread to be freed there.
    fn retire(&mut self, old: Arc<Project>) {
        if let Err(TrySendError::Full(old)) = self.retired.try_send(old) {
            if self.graveyard.len() < self.graveyard.capacity() {
                self.graveyard.push(old);
            }
            // Otherwise it is dropped here. That only happens when the UI has
            // not collected anything for a long time.
        }
    }
}

/// The UI thread's side of the connection to the audio callback.
struct Remote {
    commands: SyncSender<Command>,
    retired: Receiver<Arc<Project>>,
    /// Commands the queue had no room for (only while the device stalls).
    backlog: RefCell<VecDeque<Command>>,
    transport: Arc<Transport>,
    playing: Cell<bool>,
    /// Id of the last seek sent, and where it went.
    seek_sent: Cell<u64>,
    seek_to: Cell<f64>,
}

/// Create both ends for `project`: the UI's [`Remote`] and the callback's [`Player`].
fn connect(project: &Project, sample_rate: u32) -> (Remote, Player) {
    let (commands, commands_rx) = mpsc::sync_channel(QUEUE_LEN);
    let (retired_tx, retired) = mpsc::sync_channel(RETIRED_LEN);
    let transport = Arc::new(Transport::default());
    let remote = Remote {
        commands,
        retired,
        backlog: RefCell::new(VecDeque::new()),
        transport: transport.clone(),
        playing: Cell::new(false),
        seek_sent: Cell::new(0),
        seek_to: Cell::new(0.0),
    };
    let player = Player {
        commands: commands_rx,
        retired: retired_tx,
        graveyard: Vec::with_capacity(GRAVEYARD_LEN),
        transport,
        project: Arc::new(project.clone()),
        loop_end: project.loop_end_beats(),
        seq: Sequencer::new(sample_rate),
        playing: false,
    };
    (remote, player)
}

impl Remote {
    fn send(&self, cmd: Command) {
        {
            let mut backlog = self.backlog.borrow_mut();
            if matches!(cmd, Command::SetProject { .. }) {
                // Only the newest snapshot is worth delivering.
                backlog.retain(|c| !matches!(c, Command::SetProject { .. }));
            }
            backlog.push_back(cmd);
            while backlog.len() > MAX_BACKLOG {
                // The device has been stuck for thousands of commands: forget
                // the oldest transport/preview command, never the project.
                match backlog
                    .iter()
                    .position(|c| !matches!(c, Command::SetProject { .. }))
                {
                    Some(i) => drop(backlog.remove(i)),
                    None => break,
                }
            }
        }
        self.maintain();
    }

    /// Deliver backlogged commands and free retired snapshots.
    fn maintain(&self) {
        let mut backlog = self.backlog.borrow_mut();
        while let Some(cmd) = backlog.pop_front() {
            match self.commands.try_send(cmd) {
                Ok(()) => {}
                Err(TrySendError::Full(cmd)) => {
                    backlog.push_front(cmd);
                    break;
                }
                Err(TrySendError::Disconnected(_)) => {
                    backlog.clear();
                    break;
                }
            }
        }
        drop(backlog);
        while self.retired.try_recv().is_ok() {}
    }

    fn set_project(&self, project: &Project) {
        self.send(Command::SetProject {
            project: Arc::new(project.clone()),
            loop_end: project.loop_end_beats(),
        });
    }

    fn play(&self) {
        self.playing.set(true);
        self.send(Command::Play);
    }

    fn pause(&self) {
        self.playing.set(false);
        self.send(Command::Pause);
    }

    fn seek(&self, beat: f64) {
        let beat = if beat.is_finite() { beat.max(0.0) } else { 0.0 };
        let id = self.seek_sent.get() + 1;
        self.seek_sent.set(id);
        self.seek_to.set(beat);
        self.send(Command::Seek { beat, id });
    }

    fn preview(&self, track: usize, pitch: u8) {
        self.send(Command::Preview { track, pitch });
    }

    fn hold(&self, track: usize, pitch: u8) {
        self.send(Command::Hold { track, pitch });
    }

    fn release(&self) {
        self.send(Command::Release);
    }

    /// The playhead in beats. Right after a seek this is the seek target,
    /// until the audio thread has caught up with it.
    fn position(&self) -> f64 {
        if self.transport.seek_done.load(Ordering::Acquire) == self.seek_sent.get() {
            f64::from_bits(self.transport.position.load(Ordering::Acquire))
        } else {
            self.seek_to.get()
        }
    }
}

/// Health counters the UI can show, written lock-free by the audio thread.
#[derive(Default)]
struct Stats {
    callbacks: AtomicU64,
    /// f32 bits of the recent output peak, for a level meter.
    peak: AtomicU32,
    /// Remaining frames of the 440 Hz test tone.
    tone_frames: AtomicU32,
    /// Set by the stream's error callback (not the audio callback).
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
    remote: Remote,
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
        let (remote, player) = connect(project, sample_rate);
        let stats = Arc::new(Stats::default());
        let sc: cpal::StreamConfig = config.config();
        let stream = match config.sample_format() {
            SampleFormat::F32 => build::<f32>(&device, &sc, player, &stats),
            SampleFormat::I16 => build::<i16>(&device, &sc, player, &stats),
            SampleFormat::I32 => build::<i32>(&device, &sc, player, &stats),
            SampleFormat::U16 => build::<u16>(&device, &sc, player, &stats),
            SampleFormat::F64 => build::<f64>(&device, &sc, player, &stats),
            other => bail!("unsupported sample format {other:?}"),
        }
        .with_context(|| format!("opening stream ({format})"))?;
        stream.play().context("starting stream")?;
        Ok(Engine {
            _stream: stream,
            remote,
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

    /// Call once per UI frame: delivers commands that were waiting for room
    /// in the queue and frees project snapshots the audio thread is done with.
    pub fn maintain(&self) {
        self.remote.maintain();
    }

    /// Hand the audio thread a copy of `project` (made here, on the UI thread).
    pub fn set_project(&self, project: &Project) {
        self.remote.set_project(project);
    }

    pub fn play(&self) {
        self.remote.play();
    }

    /// Stop where we are; notes fade out over a few milliseconds.
    pub fn pause(&self) {
        self.remote.pause();
    }

    /// Move the playhead. Playing on from the middle of a held note
    /// sounds that note too.
    pub fn seek(&self, beat: f64) {
        self.remote.seek(beat);
    }

    /// Audition a note on a track's instrument (works while stopped).
    pub fn preview(&self, track: usize, pitch: u8) {
        self.remote.preview(track, pitch);
    }

    /// Start a note that sounds until [`Self::release`] (holding a piano key).
    pub fn hold(&self, track: usize, pitch: u8) {
        self.remote.hold(track, pitch);
    }

    pub fn release(&self) {
        self.remote.release();
    }

    pub fn is_playing(&self) -> bool {
        self.remote.playing.get()
    }

    pub fn position(&self) -> f64 {
        self.remote.position()
    }
}

fn build<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    mut player: Player,
    stats: &Arc<Stats>,
) -> Result<cpal::Stream, cpal::BuildStreamError>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = config.channels as usize;
    let sample_rate = config.sample_rate.0 as f32;
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
            player.render(&mut stereo);
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

#[cfg(test)]
mod tests {
    use super::*;
    use aidaw_core::edit::{apply_all, Op};
    use aidaw_core::{presets, Note};

    const SR: u32 = 22_050;

    /// Counts heap allocations and frees made by the current thread while armed.
    mod counting {
        use std::alloc::{GlobalAlloc, Layout, System};
        use std::cell::Cell;

        thread_local! {
            static ARMED: Cell<bool> = const { Cell::new(false) };
            static COUNT: Cell<usize> = const { Cell::new(0) };
        }

        fn note() {
            if ARMED.try_with(Cell::get).unwrap_or(false) {
                let _ = COUNT.try_with(|c| c.set(c.get() + 1));
            }
        }

        pub struct Counting;

        // SAFETY: forwards every call unchanged to the system allocator.
        unsafe impl GlobalAlloc for Counting {
            unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
                note();
                unsafe { System.alloc(layout) }
            }
            unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
                note();
                unsafe { System.alloc_zeroed(layout) }
            }
            unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
                note();
                unsafe { System.realloc(ptr, layout, size) }
            }
            unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
                note();
                unsafe { System.dealloc(ptr, layout) }
            }
        }

        #[global_allocator]
        static GLOBAL: Counting = Counting;

        /// How many times `f` allocated or freed memory.
        pub fn heap_calls(f: impl FnOnce()) -> usize {
            COUNT.with(|c| c.set(0));
            ARMED.with(|a| a.set(true));
            f();
            ARMED.with(|a| a.set(false));
            COUNT.with(Cell::get)
        }
    }

    /// Piano chords held for a whole bar, plus a drum beat.
    fn song() -> Project {
        let mut p = Project::new("song", 120.0);
        let keys = presets::find("warm_pad").unwrap().instrument.clone();
        let drums = presets::find("drum_kit").unwrap().instrument.clone();
        let note = |pitch, start, duration| Note {
            pitch,
            start,
            duration,
            velocity: 100,
        };
        apply_all(
            &mut p,
            vec![
                Op::AddTrack {
                    name: "Pad".into(),
                    instrument: Some(keys),
                    gain: None,
                    pan: None,
                },
                Op::AddTrack {
                    name: "Drums".into(),
                    instrument: Some(drums),
                    gain: None,
                    pan: None,
                },
                Op::AddClip {
                    track_id: "t1".into(),
                    start: 0.0,
                    length: 8.0,
                    name: None,
                    notes: vec![note(57, 0.0, 4.0), note(60, 0.0, 4.0), note(53, 4.0, 4.0)],
                },
                Op::AddClip {
                    track_id: "t2".into(),
                    start: 0.0,
                    length: 8.0,
                    name: None,
                    notes: (0..8).map(|b| note(36, b as f64, 0.25)).collect(),
                },
            ],
        )
        .unwrap();
        p
    }

    fn block() -> Vec<f32> {
        vec![0.0; 2 * 512]
    }

    fn peak(buf: &[f32]) -> f32 {
        buf.iter().fold(0.0f32, |m, s| m.max(s.abs()))
    }

    #[test]
    fn transport_commands_reach_the_audio_thread_in_order() {
        let p = song();
        let (remote, mut player) = connect(&p, SR);
        let mut buf = block();
        player.render(&mut buf);
        assert_eq!(remote.position(), 0.0);
        assert_eq!(peak(&buf), 0.0, "silent until Play");

        remote.play();
        assert!(remote.playing.get(), "the UI sees its own Play at once");
        for _ in 0..10 {
            player.render(&mut buf);
        }
        let beats = 10.0 * 512.0 / SR as f64 * 2.0; // 120 BPM = 2 beats per second
        assert!(
            (remote.position() - beats).abs() < 1e-9,
            "{}",
            remote.position()
        );

        remote.pause();
        player.render(&mut buf);
        let paused_at = remote.position();
        player.render(&mut buf);
        assert_eq!(remote.position(), paused_at);
    }

    #[test]
    fn a_seek_shows_its_target_until_the_audio_thread_catches_up() {
        let p = song();
        let (remote, mut player) = connect(&p, SR);
        let mut buf = block();
        remote.play();
        player.render(&mut buf);
        remote.seek(6.0);
        // The callback has not run yet, but the playhead is already there.
        assert_eq!(remote.position(), 6.0);
        player.render(&mut buf);
        let pos = remote.position();
        assert!(pos > 6.0 && pos < 6.1, "{pos}");
        // Seeking while stopped moves the playhead too.
        remote.pause();
        remote.seek(2.5);
        player.render(&mut buf);
        assert_eq!(remote.position(), 2.5);
    }

    #[test]
    fn playing_from_the_middle_of_a_chord_is_not_silent() {
        let p = song();
        let (remote, mut player) = connect(&p, SR);
        let mut buf = block();
        // Beat 2 is half-way through the first chord, between two kicks.
        remote.seek(2.1);
        remote.play();
        player.render(&mut buf);
        assert!(peak(&buf) > 0.01, "chord is heard: {}", peak(&buf));
        // It fades in rather than jumping straight to full level.
        assert!(buf[0].abs() < 1e-3, "{}", buf[0]);
    }

    #[test]
    fn replaced_projects_are_freed_on_the_ui_thread() {
        let p = song();
        let (remote, mut player) = connect(&p, SR);
        let mut buf = block();
        let mut renamed = p.clone();
        renamed.name = "renamed".into();
        remote.set_project(&renamed);
        player.render(&mut buf);
        assert_eq!(player.project.name, "renamed");
        // The old snapshot came back to the UI side...
        let old = remote.retired.try_recv().expect("old snapshot returned");
        assert_eq!(old.name, "song");
        // ...and the audio side kept no reference to it.
        assert_eq!(Arc::strong_count(&old), 1);
    }

    #[test]
    fn a_stalled_device_keeps_only_the_newest_project() {
        let p = song();
        let (remote, mut player) = connect(&p, SR);
        let mut buf = block();
        // The callback stops running while the UI keeps editing.
        for i in 0..2 * QUEUE_LEN {
            let mut q = p.clone();
            q.name = format!("v{i}");
            remote.set_project(&q);
            remote.preview(0, 60);
        }
        let backlog = remote.backlog.borrow();
        let projects = backlog
            .iter()
            .filter(|c| matches!(c, Command::SetProject { .. }))
            .count();
        assert_eq!(projects, 1, "older snapshots are dropped, not queued");
        drop(backlog);
        // Once the device runs again everything arrives, newest project last.
        for _ in 0..8 {
            player.render(&mut buf);
            remote.maintain();
        }
        assert!(remote.backlog.borrow().is_empty());
        assert_eq!(player.project.name, format!("v{}", 2 * QUEUE_LEN - 1));
    }

    #[test]
    fn the_audio_callback_never_touches_the_heap() {
        use counting::heap_calls;
        let p = song();
        let (remote, mut player) = connect(&p, SR);
        let mut buf = block();
        let mut render = |player: &mut Player| heap_calls(|| player.render(&mut buf));
        // The counter does see allocations.
        assert!(heap_calls(|| drop(std::hint::black_box(vec![0u8; 64]))) >= 2);

        // Previews while stopped.
        remote.preview(0, 60);
        remote.hold(0, 64);
        assert_eq!(render(&mut player), 0);
        remote.release();
        assert_eq!(render(&mut player), 0);

        // Playing, seeking into held notes, looping around the end.
        remote.play();
        remote.seek(2.1);
        for _ in 0..200 {
            assert_eq!(render(&mut player), 0);
        }

        // A new project arrives while playing; the old one goes back.
        let mut q = p.clone();
        q.tracks[0].gain = 0.5;
        remote.set_project(&q);
        assert_eq!(render(&mut player), 0);
        assert!(remote.retired.try_recv().is_ok());

        // Many snapshots without the UI collecting: they wait in the graveyard.
        for _ in 0..RETIRED_LEN + 8 {
            remote
                .commands
                .try_send(Command::SetProject {
                    project: Arc::new(q.clone()),
                    loop_end: q.loop_end_beats(),
                })
                .ok();
            assert_eq!(render(&mut player), 0);
        }
        assert!(!player.graveyard.is_empty());
        while remote.retired.try_recv().is_ok() {}
        assert_eq!(render(&mut player), 0);
        assert!(player.graveyard.is_empty());

        remote.pause();
        for _ in 0..20 {
            assert_eq!(render(&mut player), 0);
        }
    }
}
