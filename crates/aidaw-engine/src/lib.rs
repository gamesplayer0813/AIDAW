//! Real-time playback and recording. Opens an output device through cpal
//! (WASAPI on Windows, or ASIO in builds with the `asio` feature) and runs
//! the core [`Sequencer`] inside the audio callback; optionally an input
//! device for recording and monitoring, and MIDI keyboards.
//!
//! The audio callback never waits for the UI, and the UI never waits for
//! the audio callback:
//! - The UI sends commands (play, seek, note previews, the latest project)
//!   through a bounded queue. Neither side ever blocks on it (only
//!   `try_send` / `try_recv`), so it never takes a lock either. MIDI notes
//!   come through a second queue of the same kind.
//! - The playhead comes back through atomics, and the song clock (which
//!   beat is heard when, see [`clock`]) through a seqlock.
//! - Projects and sound files travel as `Arc` snapshots. The callback hands
//!   every snapshot it replaces back to the UI thread, which frees it, so
//!   the callback never frees (or allocates) memory.

mod clock;
mod devices;
mod input;
mod midi;

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::f32::consts::TAU;
use std::path::Path;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use aidaw_core::render::{track_gains_at, Sequencer};
use aidaw_core::{AudioPool, Param, Project};
use anyhow::{anyhow, bail, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};
use rtrb::Consumer;

use clock::{Clock, Moment};
pub use devices::{drivers, has_asio, input_devices, output_device_names, output_devices};
pub use input::{Input, Take, MAX_METERED};
pub use midi::{midi_inputs, MidiEvent, MidiProblem};
use midi::{Live, Midi};

/// Commands that fit between two audio callbacks. Far more than a person
/// can trigger in 10 ms; the rest wait in the UI-side backlog.
const QUEUE_LEN: usize = 256;
/// MIDI messages that fit between two audio callbacks.
const LIVE_LEN: usize = 512;
/// Replaced snapshots on their way back to the UI thread.
const RETIRED_LEN: usize = 64;
/// Snapshots the callback can hold on to when the UI is slow to collect them.
const GRAVEYARD_LEN: usize = 64;
/// Commands kept while the device has stalled (beyond the queue).
const MAX_BACKLOG: usize = 4096;
/// Monitoring buffers this much input to start with, and twice as much
/// after each time the input arrived late (up to the most below).
const MONITOR_CUSHION_SECONDS: f64 = 0.002;
const MONITOR_CUSHION_MAX_SECONDS: f64 = 0.05;

enum Command {
    SetProject {
        project: Arc<Project>,
        /// [`Project::playback_loop`], worked out on the UI thread.
        looping: Option<(f64, f64)>,
    },
    /// The sound files the audio clips play.
    SetAudio(Arc<AudioPool>),
    Play,
    /// Count `beats` beats of metronome clicks in, then play.
    CountIn {
        beats: f64,
    },
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
    Metronome(bool),
    /// Play straight on past the end of the song instead of looping
    /// (while recording).
    Linear(bool),
    /// The track MIDI keyboards play.
    LiveTrack(Option<usize>),
    /// Input to hear through the speakers, or none.
    Monitor(Option<Monitor>),
}

impl Command {
    /// Only the newest command of these kinds matters.
    fn replaces(&self, older: &Command) -> bool {
        matches!(
            (self, older),
            (Command::SetProject { .. }, Command::SetProject { .. })
                | (Command::SetAudio(_), Command::SetAudio(_))
        )
    }

    /// Commands a stuck device may forget (the song stays as it should).
    fn expendable(&self) -> bool {
        matches!(
            self,
            Command::Preview { .. }
                | Command::Hold { .. }
                | Command::Release
                | Command::Seek { .. }
        )
    }
}

/// Live input for monitoring: stereo frames from the input callback, heard
/// at a track's volume and pan.
struct Monitor {
    feed: Consumer<f32>,
    track: usize,
    /// Frames buffered before playing (grows when the input runs late).
    cushion: usize,
    max_cushion: usize,
    primed: bool,
}

/// What the callback hands back to the UI thread to be freed there.
enum Retired {
    Project(#[allow(dead_code)] Arc<Project>),
    Audio(#[allow(dead_code)] Arc<AudioPool>),
    Monitor(#[allow(dead_code)] Monitor),
}

/// What the audio thread reports back, lock-free.
#[derive(Default)]
struct Transport {
    /// f64 bits of the playhead, in beats.
    position: AtomicU64,
    /// Id of the last seek the audio thread has carried out.
    seek_done: AtomicU64,
    /// f64 bits of the beats left to count in (0 when not counting in).
    count_in: AtomicU64,
}

/// The audio callback's side: owns the sequencer and the current project.
/// Kept apart from cpal so tests can drive it the way a sound card would.
struct Player {
    commands: Receiver<Command>,
    live: Receiver<Live>,
    retired: SyncSender<Retired>,
    /// Snapshots that did not fit in `retired` yet (capacity reserved).
    graveyard: Vec<Retired>,
    transport: Arc<Transport>,
    clock: Arc<Clock>,
    project: Arc<Project>,
    /// What playback repeats: the loop region, or else the whole song.
    looping: Option<(f64, f64)>,
    seq: Sequencer,
    sample_rate: u32,
    playing: bool,
    /// While counting in: the beat being heard, before the playhead.
    count_in: Option<f64>,
    linear: bool,
    live_track: Option<usize>,
    monitor: Option<Monitor>,
}

impl Player {
    /// Apply the queued commands, then render `out` (interleaved stereo)
    /// whose first frame will be heard at `heard_ns` on the song clock.
    fn render(&mut self, out: &mut [f32], heard_ns: u64) {
        self.drain_commands();
        let moving = self.playing || self.count_in.is_some();
        let beats_per_ns = 1e-9 / self.project.seconds_per_beat();
        self.clock.publish(Moment {
            heard_ns,
            beat: self.count_in.unwrap_or(self.seq.position()),
            beats_per_ns: if moving && beats_per_ns.is_finite() {
                beats_per_ns
            } else {
                0.0
            },
        });
        if let Some(beat) = self.count_in {
            let fpb = self.sample_rate as f64 * self.project.seconds_per_beat();
            let left = ((self.seq.position() - beat) * fpb).round().max(0.0) as usize;
            let n = left.min(out.len() / 2);
            let (counting, rest) = out.split_at_mut(n * 2);
            if n > 0 {
                self.seq.count_in(&self.project, beat, counting);
            }
            if n == left {
                self.count_in = None;
                self.playing = true;
                self.play(rest);
            } else {
                self.count_in = Some(beat + n as f64 / fpb);
            }
        } else if self.playing {
            self.play(out);
        } else if self.seq.is_sounding() {
            // Stopped, but note previews or the reverb tail are still ringing.
            self.seq.process_voices(&self.project, out);
        } else {
            out.fill(0.0);
        }
        self.mix_monitor(out);
        let left = self
            .count_in
            .map_or(0.0, |b| (self.seq.position() - b).max(0.0));
        self.transport
            .count_in
            .store(left.to_bits(), Ordering::Release);
        self.transport
            .position
            .store(self.seq.position().to_bits(), Ordering::Release);
    }

    fn play(&mut self, out: &mut [f32]) {
        if out.is_empty() {
            return;
        }
        let looping = if self.linear { None } else { self.looping };
        self.seq.process_looping(&self.project, out, looping);
    }

    /// Add the monitored input at its track's volume and pan.
    fn mix_monitor(&mut self, out: &mut [f32]) {
        let Some(m) = &mut self.monitor else {
            return;
        };
        let frames = out.len() / 2;
        let mut queued = m.feed.slots() / 2;
        // Never let the delay build up: skip what's beyond one block.
        let most = m.cushion + frames;
        if queued > most + frames {
            if let Ok(skip) = m.feed.read_chunk((queued - most) * 2) {
                skip.commit_all();
            }
            queued = most;
        }
        if !m.primed {
            if queued < m.cushion + frames.min(m.cushion) {
                return;
            }
            m.primed = true;
        }
        // The track's volume and pan (and the master volume) where the
        // playhead is, automation included.
        let beat = self.seq.position();
        let gain = match self.project.tracks.get(m.track) {
            Some(t) => {
                let [l, r, _] = track_gains_at(t, self.project.any_solo(), beat);
                let master = self
                    .project
                    .automated(Param::MasterVolume, beat)
                    .unwrap_or(self.project.master_gain);
                let master = if master.is_finite() {
                    master.clamp(0.0, 4.0)
                } else {
                    0.0
                };
                [l * master, r * master]
            }
            None => [0.0, 0.0],
        };
        let n = queued.min(frames);
        if let Ok(chunk) = m.feed.read_chunk(n * 2) {
            let (a, b) = chunk.as_slices();
            for (i, s) in a.iter().chain(b).enumerate() {
                out[i] += s * gain[i % 2];
            }
            chunk.commit_all();
        }
        if n < frames {
            // The input ran late: wait for a bigger cushion before going on.
            m.primed = false;
            m.cushion = (m.cushion * 2).min(m.max_cushion);
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
                Command::SetProject { project, looping } => {
                    let old = std::mem::replace(&mut self.project, project);
                    self.looping = looping;
                    self.retire(Retired::Project(old));
                }
                Command::SetAudio(pool) => {
                    let old = self.seq.set_audio(pool);
                    self.retire(Retired::Audio(old));
                }
                Command::Play => {
                    // Play starts inside the loop: from its start when the
                    // playhead is before it or past it (not while recording,
                    // which plays straight on from the playhead).
                    if self.count_in.is_none() {
                        if let Some((start, end)) = self.looping.filter(|_| !self.linear) {
                            let pos = self.seq.position();
                            if !self.playing && (pos < start || pos >= end) {
                                self.seq.seek(start);
                                self.transport
                                    .position
                                    .store(self.seq.position().to_bits(), Ordering::Release);
                            }
                        }
                        self.playing = true;
                    }
                }
                Command::CountIn { beats } => {
                    if !self.playing && self.count_in.is_none() {
                        let beats = if beats.is_finite() {
                            beats.max(0.0)
                        } else {
                            0.0
                        };
                        self.count_in = Some(self.seq.position() - beats);
                    }
                }
                Command::Pause => {
                    self.playing = false;
                    self.count_in = None;
                    // Fade out whatever is sounding; playing again picks up
                    // the notes held across this point.
                    let pos = self.seq.position();
                    self.seq.seek(pos);
                }
                Command::Seek { beat, id } => {
                    let left = self.count_in.map(|b| self.seq.position() - b);
                    self.seq.seek(beat);
                    if let Some(left) = left {
                        self.count_in = Some(self.seq.position() - left);
                    }
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
                Command::Metronome(on) => self.seq.set_metronome(on),
                Command::Linear(on) => self.linear = on,
                Command::LiveTrack(track) => {
                    if track != self.live_track {
                        self.seq.all_notes_off();
                        self.live_track = track;
                    }
                }
                Command::Monitor(monitor) => {
                    if let Some(old) = std::mem::replace(&mut self.monitor, monitor) {
                        self.retire(Retired::Monitor(old));
                    }
                }
            }
        }
        while let Ok(event) = self.live.try_recv() {
            match (event, self.live_track) {
                (Live::On { pitch, velocity }, Some(track)) => {
                    self.seq.note_on(&self.project, track, pitch, velocity)
                }
                (Live::Off { pitch }, Some(track)) => self.seq.note_off(track, pitch),
                (Live::Pedal(down), _) => self.seq.set_pedal(down),
                (Live::AllOff, _) => self.seq.all_notes_off(),
                (_, None) => {}
            }
        }
    }

    /// Send a replaced snapshot to the UI thread to be freed there.
    fn retire(&mut self, old: Retired) {
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
    retired: Receiver<Retired>,
    /// Where MIDI keyboards send their notes.
    live: SyncSender<Live>,
    /// Commands the queue had no room for (only while the device stalls).
    backlog: RefCell<VecDeque<Command>>,
    transport: Arc<Transport>,
    playing: Cell<bool>,
    /// Id of the last seek sent, and where it went.
    seek_sent: Cell<u64>,
    seek_to: Cell<f64>,
}

/// The copy of `project` the audio thread plays, with every clip's notes in
/// start order as the sequencer needs (the editor may have them in any
/// order while a note is being dragged).
fn snapshot(project: &Project) -> Arc<Project> {
    let mut copy = project.clone();
    copy.sort_notes();
    Arc::new(copy)
}

/// Create both ends for `project`: the UI's [`Remote`] and the callback's [`Player`].
fn connect(project: &Project, sample_rate: u32, clock: Arc<Clock>) -> (Remote, Player) {
    let (commands, commands_rx) = mpsc::sync_channel(QUEUE_LEN);
    let (live, live_rx) = mpsc::sync_channel(LIVE_LEN);
    let (retired_tx, retired) = mpsc::sync_channel(RETIRED_LEN);
    let transport = Arc::new(Transport::default());
    let remote = Remote {
        commands,
        retired,
        live,
        backlog: RefCell::new(VecDeque::new()),
        transport: transport.clone(),
        playing: Cell::new(false),
        seek_sent: Cell::new(0),
        seek_to: Cell::new(0.0),
    };
    let player = Player {
        commands: commands_rx,
        live: live_rx,
        retired: retired_tx,
        graveyard: Vec::with_capacity(GRAVEYARD_LEN),
        transport,
        clock,
        project: snapshot(project),
        looping: project.playback_loop(),
        seq: Sequencer::new(sample_rate),
        sample_rate,
        playing: false,
        count_in: None,
        linear: false,
        live_track: None,
        monitor: None,
    };
    (remote, player)
}

impl Remote {
    fn send(&self, cmd: Command) {
        {
            let mut backlog = self.backlog.borrow_mut();
            backlog.retain(|c| !cmd.replaces(c));
            backlog.push_back(cmd);
            while backlog.len() > MAX_BACKLOG {
                // The device has been stuck for thousands of commands: forget
                // the oldest preview or seek, never what the song needs.
                match backlog.iter().position(Command::expendable) {
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
            project: snapshot(project),
            looping: project.playback_loop(),
        });
    }

    fn play(&self) {
        self.playing.set(true);
        self.send(Command::Play);
    }

    fn count_in(&self, beats: f64) {
        self.playing.set(true);
        self.send(Command::CountIn { beats });
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

    /// Beats of count-in still to go, while counting in.
    fn counting_in(&self) -> Option<f64> {
        let left = f64::from_bits(self.transport.count_in.load(Ordering::Acquire));
        (left > 0.0 && self.playing.get()).then_some(left)
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
    /// How long until what the callback renders is heard, in µs.
    latency_us: AtomicU32,
    /// Set by the stream's error callback (not the audio callback).
    error: Mutex<Option<String>>,
}

pub struct Diagnostics {
    /// Number of times the audio device has asked for samples. If this stays
    /// at 0 the device is not running at all.
    pub callbacks: u64,
    pub peak: f32,
    pub error: Option<String>,
    /// Output latency as the device reports it.
    pub latency: Duration,
}

/// Device errors that are only news: the stream keeps running.
pub(crate) fn is_harmless(e: &cpal::Error) -> bool {
    use cpal::ErrorKind::*;
    matches!(e.kind(), DeviceChanged | Xrun | RealtimeDenied)
}

pub struct Engine {
    _stream: cpal::Stream,
    device: cpal::Device,
    driver: &'static str,
    remote: Remote,
    stats: Arc<Stats>,
    clock: Arc<Clock>,
    sample_rate: u32,
    device_name: String,
    format: String,
    input: Option<Input>,
    midi: Option<Midi>,
}

impl Engine {
    /// Open the system default output, falling back to any device that works.
    pub fn new(project: &Project) -> Result<Engine> {
        Self::open(project, None, None)
    }

    /// Open a specific output device of the default driver, by name.
    pub fn with_device(project: &Project, name: &str) -> Result<Engine> {
        Self::open(project, None, Some(name))
    }

    /// Open an output on `driver` (a name from [`drivers`]; `None` for the
    /// system's default driver): the device called `output`, or the
    /// default device, falling back to any device that works.
    pub fn open(project: &Project, driver: Option<&str>, output: Option<&str>) -> Result<Engine> {
        let host = devices::host(driver)?;
        let driver = host.id().name();
        let candidates: Vec<cpal::Device> = match output {
            Some(name) => vec![devices::find_output(&host, name)?],
            None => {
                let mut c: Vec<cpal::Device> = host.default_output_device().into_iter().collect();
                if let Ok(devs) = host.output_devices() {
                    c.extend(devs);
                }
                c
            }
        };
        if candidates.is_empty() {
            bail!("no audio output devices found ({driver})");
        }
        let mut errors = Vec::new();
        for device in candidates {
            let name = devices::name_of(&device);
            match Self::open_device(project, device, driver) {
                Ok(e) => return Ok(e),
                Err(e) => errors.push(format!("{name}: {e:#}")),
            }
        }
        Err(anyhow!(
            "could not open any output device:\n{}",
            errors.join("\n")
        ))
    }

    fn open_device(
        project: &Project,
        device: cpal::Device,
        driver: &'static str,
    ) -> Result<Engine> {
        let device_name = devices::name_of(&device);
        let config = match device.default_output_config() {
            Ok(c) => c,
            Err(e) => device
                .supported_output_configs()
                .context("listing supported configs")?
                .max_by_key(|c| (c.sample_format() == SampleFormat::F32, c.channels() == 2))
                .map(|c| c.with_max_sample_rate())
                .ok_or_else(|| anyhow!("no usable config: {e}"))?,
        };
        let sample_rate = config.sample_rate();
        let format = format!(
            "{driver}, {:?}, {} ch, {} Hz",
            config.sample_format(),
            config.channels(),
            sample_rate
        );
        let clock = Arc::new(Clock::new());
        let (remote, player) = connect(project, sample_rate, clock.clone());
        let stats = Arc::new(Stats::default());
        let sc = config.config();
        let stream = match config.sample_format() {
            SampleFormat::F32 => build::<f32>(&device, sc, player, &stats, &clock),
            SampleFormat::I16 => build::<i16>(&device, sc, player, &stats, &clock),
            SampleFormat::I24 => build::<cpal::I24>(&device, sc, player, &stats, &clock),
            SampleFormat::I32 => build::<i32>(&device, sc, player, &stats, &clock),
            SampleFormat::U16 => build::<u16>(&device, sc, player, &stats, &clock),
            SampleFormat::F64 => build::<f64>(&device, sc, player, &stats, &clock),
            SampleFormat::I8 => build::<i8>(&device, sc, player, &stats, &clock),
            SampleFormat::U8 => build::<u8>(&device, sc, player, &stats, &clock),
            other => bail!("unsupported sample format {other:?}"),
        }
        .with_context(|| format!("opening stream ({format})"))?;
        stream.play().context("starting stream")?;
        Ok(Engine {
            _stream: stream,
            device,
            driver,
            remote,
            stats,
            clock,
            sample_rate,
            device_name,
            format,
            input: None,
            midi: None,
        })
    }

    pub fn device_name(&self) -> &str {
        &self.device_name
    }

    /// The driver in use ("WASAPI", "ASIO", ...).
    pub fn driver(&self) -> &str {
        self.driver
    }

    pub fn is_asio(&self) -> bool {
        self.driver.eq_ignore_ascii_case("asio")
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
            latency: Duration::from_micros(self.stats.latency_us.load(Ordering::Relaxed) as u64),
        }
    }

    /// Play a one-second 440 Hz beep, independent of the song.
    pub fn test_tone(&self) {
        self.stats
            .tone_frames
            .store(self.sample_rate, Ordering::Relaxed);
    }

    /// Call once per UI frame: delivers commands that were waiting for room
    /// in the queue and frees snapshots the audio threads are done with.
    pub fn maintain(&mut self) {
        self.remote.maintain();
        if let Some(input) = &mut self.input {
            input.maintain();
        }
    }

    /// Hand the audio thread a copy of `project` (made here, on the UI thread).
    pub fn set_project(&self, project: &Project) {
        self.remote.set_project(project);
    }

    /// Hand the audio thread the sound files the audio clips play.
    pub fn set_audio(&self, audio: Arc<AudioPool>) {
        self.remote.send(Command::SetAudio(audio));
    }

    /// Start playing. With a loop region on, playback starts at the loop's
    /// start unless the playhead is already inside the loop.
    pub fn play(&self) {
        self.remote.play();
    }

    /// Click `beats` beats of metronome first, then play from the playhead
    /// (the usual way to start recording).
    pub fn play_after_count_in(&self, beats: f64) {
        self.remote.count_in(beats);
    }

    /// Beats of count-in still to go, while counting in.
    pub fn counting_in(&self) -> Option<f64> {
        self.remote.counting_in()
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

    /// Click on every beat while playing.
    pub fn set_metronome(&self, on: bool) {
        self.remote.send(Command::Metronome(on));
    }

    /// While recording the song plays straight on past its end instead of
    /// looping back to the start.
    pub fn set_recording(&self, on: bool) {
        self.remote.send(Command::Linear(on));
    }

    /// The track (by index) that MIDI keyboards play, or none.
    pub fn set_live_track(&self, track: Option<usize>) {
        self.remote.send(Command::LiveTrack(track));
    }

    pub fn is_playing(&self) -> bool {
        self.remote.playing.get()
    }

    pub fn position(&self) -> f64 {
        self.remote.position()
    }

    // ---- recording -------------------------------------------------------

    /// Open an input for recording: the device called `name`, or the
    /// default one. With ASIO the input is always the output's own device.
    pub fn open_input(&mut self, name: Option<&str>) -> Result<()> {
        self.close_input();
        let device = if self.is_asio() {
            self.device.clone()
        } else {
            let host = devices::host(Some(self.driver))?;
            match name {
                Some(n) => devices::find_input(&host, n)?,
                None => host
                    .default_input_device()
                    .ok_or_else(|| anyhow!("no microphone or other input found"))?,
            }
        };
        self.input = Some(Input::open(device, self.clock.clone(), self.sample_rate)?);
        Ok(())
    }

    pub fn close_input(&mut self) {
        self.set_monitor(None);
        if let Some(input) = self.input.take() {
            input.pause();
        }
    }

    pub fn input(&self) -> Option<&Input> {
        self.input.as_ref()
    }

    /// Start recording `inputs` (1-based channels of the input device: one
    /// for mono, two for stereo) into a new WAV file at `path`. Start the
    /// song (usually with a count-in) right after.
    pub fn start_take(&mut self, path: &Path, inputs: &[u16]) -> Result<()> {
        let input = self
            .input
            .as_mut()
            .ok_or_else(|| anyhow!("no input is open"))?;
        input.start_take(path, inputs)
    }

    /// Finish the take; [`Self::finished_take`] returns it once it is saved.
    pub fn stop_take(&mut self) {
        if let Some(input) = &mut self.input {
            input.stop_take();
        }
    }

    pub fn is_taking(&self) -> bool {
        self.input.as_ref().is_some_and(Input::is_recording)
    }

    /// Seconds recorded so far in the current take.
    pub fn take_seconds(&self) -> Option<f64> {
        self.input.as_ref()?.take_seconds()
    }

    /// Whether the take can't be saved (e.g. the disk is full).
    pub fn take_failed(&self) -> bool {
        self.input.as_ref().is_some_and(Input::take_failed)
    }

    /// The take, once its file is complete.
    pub fn finished_take(&mut self) -> Option<Result<Take>> {
        self.input.as_mut()?.finished_take()
    }

    /// Hear `inputs` of the input device through the speakers, at the
    /// volume and pan of track `track`; `None` stops. Needs the input to
    /// run at the output's sample rate.
    pub fn set_monitor(&mut self, monitor: Option<(usize, &[u16])>) -> Option<String> {
        let Some(input) = &mut self.input else {
            self.remote.send(Command::Monitor(None));
            return monitor.map(|_| "no input is open".to_string());
        };
        match monitor {
            None => {
                input.untap();
                self.remote.send(Command::Monitor(None));
                None
            }
            Some(_) if input.sample_rate() != self.sample_rate => Some(format!(
                "the input runs at {} Hz and the output at {} Hz",
                input.sample_rate(),
                self.sample_rate
            )),
            Some((track, inputs)) => {
                let feed = input.tap(inputs);
                let rate = self.sample_rate as f64;
                self.remote.send(Command::Monitor(Some(Monitor {
                    feed,
                    track,
                    cushion: (rate * MONITOR_CUSHION_SECONDS) as usize,
                    max_cushion: (rate * MONITOR_CUSHION_MAX_SECONDS) as usize,
                    primed: false,
                })));
                None
            }
        }
    }

    // ---- MIDI --------------------------------------------------------------

    /// Listen to the MIDI input called `port`, or to all of them.
    pub fn open_midi(&mut self, port: Option<&str>) {
        self.midi = None;
        self.midi = Some(Midi::open(
            port,
            self.remote.live.clone(),
            self.clock.clone(),
        ));
    }

    pub fn close_midi(&mut self) {
        self.midi = None;
    }

    /// The MIDI inputs being listened to.
    pub fn midi_ports(&self) -> Vec<String> {
        self.midi.as_ref().map(Midi::ports).unwrap_or_default()
    }

    /// MIDI inputs that could not be opened, and why.
    pub fn midi_problems(&self) -> Vec<MidiProblem> {
        self.midi
            .as_ref()
            .map(|m| m.problems().to_vec())
            .unwrap_or_default()
    }

    /// The keys played on MIDI keyboards since the last call.
    pub fn midi_events(&self) -> Vec<MidiEvent> {
        self.midi.as_ref().map(Midi::drain).unwrap_or_default()
    }
}

fn build<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    mut player: Player,
    stats: &Arc<Stats>,
    clock: &Arc<Clock>,
) -> Result<cpal::Stream, cpal::Error>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = config.channels.max(1) as usize;
    let sample_rate = config.sample_rate as f32;
    let cb_stats = stats.clone();
    let err_stats = stats.clone();
    let clock = clock.clone();
    // Large enough for any common WASAPI/ALSA/ASIO period so the callback never allocates.
    let mut stereo: Vec<f32> = Vec::with_capacity(1 << 15);
    let mut tone_phase = 0.0f32;
    let tone_fade = (sample_rate * 0.005).max(1.0);
    let mut peak = 0.0f32;
    device.build_output_stream::<T, _, _>(
        config,
        move |data: &mut [T], info: &cpal::OutputCallbackInfo| {
            cb_stats.callbacks.fetch_add(1, Ordering::Relaxed);
            let ts = info.timestamp();
            let latency = clock::delay(ts.playback, ts.callback);
            cb_stats
                .latency_us
                .store(latency.as_micros() as u32, Ordering::Relaxed);
            let heard_ns = clock.now_ns() + latency.as_nanos() as u64;
            let frames = data.len() / channels;
            stereo.clear();
            stereo.resize(frames * 2, 0.0);
            player.render(&mut stereo, heard_ns);
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
                    // tone and monitoring mix and integer conversion against
                    // overflow.
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
            if is_harmless(&err) {
                return;
            }
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
    use aidaw_core::{presets, AudioData, Note};

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

    /// Pad chords held for a whole bar (with some reverb), plus a drum beat.
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
                    reverb: Some(0.4),
                },
                Op::AddTrack {
                    name: "Drums".into(),
                    instrument: Some(drums),
                    gain: None,
                    pan: None,
                    reverb: None,
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

    fn pair(p: &Project) -> (Remote, Player) {
        connect(p, SR, Arc::new(Clock::new()))
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
        let (remote, mut player) = pair(&p);
        let mut buf = block();
        player.render(&mut buf, 0);
        assert_eq!(remote.position(), 0.0);
        assert_eq!(peak(&buf), 0.0, "silent until Play");

        remote.play();
        assert!(remote.playing.get(), "the UI sees its own Play at once");
        for _ in 0..10 {
            player.render(&mut buf, 0);
        }
        let beats = 10.0 * 512.0 / SR as f64 * 2.0; // 120 BPM = 2 beats per second
        assert!(
            (remote.position() - beats).abs() < 1e-9,
            "{}",
            remote.position()
        );

        remote.pause();
        player.render(&mut buf, 0);
        let paused_at = remote.position();
        player.render(&mut buf, 0);
        assert_eq!(remote.position(), paused_at);
    }

    #[test]
    fn a_seek_shows_its_target_until_the_audio_thread_catches_up() {
        let p = song();
        let (remote, mut player) = pair(&p);
        let mut buf = block();
        remote.play();
        player.render(&mut buf, 0);
        remote.seek(6.0);
        // The callback has not run yet, but the playhead is already there.
        assert_eq!(remote.position(), 6.0);
        player.render(&mut buf, 0);
        let pos = remote.position();
        assert!(pos > 6.0 && pos < 6.1, "{pos}");
        // Seeking while stopped moves the playhead too.
        remote.pause();
        remote.seek(2.5);
        player.render(&mut buf, 0);
        assert_eq!(remote.position(), 2.5);
    }

    #[test]
    fn playing_from_the_middle_of_a_chord_is_not_silent() {
        let p = song();
        let (remote, mut player) = pair(&p);
        let mut buf = block();
        // Beat 2 is half-way through the first chord, between two kicks.
        remote.seek(2.1);
        remote.play();
        player.render(&mut buf, 0);
        assert!(peak(&buf) > 0.01, "chord is heard: {}", peak(&buf));
        // It fades in rather than jumping straight to full level.
        assert!(buf[0].abs() < 1e-3, "{}", buf[0]);
    }

    #[test]
    fn notes_the_editor_left_out_of_order_still_play() {
        // Dragging a note past its neighbours leaves the clip unsorted.
        let mut p = song();
        p.tracks[1].clips[0].notes.reverse();
        assert!(!p.notes_sorted());
        let (remote, mut player) = pair(&p);
        remote.set_project(&p);
        remote.play();
        let mut buf = block();
        player.render(&mut buf, 0);
        assert!(player.project.notes_sorted());
        assert!(peak(&buf) > 0.01, "the first kick is heard: {}", peak(&buf));
    }

    #[test]
    fn play_starts_inside_the_loop_region_and_repeats_it() {
        let mut p = song();
        p.transport.loop_region = Some(aidaw_core::LoopRegion {
            start: 4.0,
            end: 6.0,
            on: true,
        });
        let (remote, mut player) = pair(&p);
        let mut buf = block();
        // The playhead sits at the start of the song, before the loop.
        remote.play();
        player.render(&mut buf, 0);
        let pos = remote.position();
        assert!(pos > 4.0 && pos < 4.1, "{pos}");
        // Two beats at 120 BPM is one second: the loop comes round again.
        for _ in 0..(SR as usize / 512 + 2) {
            player.render(&mut buf, 0);
        }
        let pos = remote.position();
        assert!((4.0..4.2).contains(&pos), "{pos}");
        // Seeking before the loop while playing plays into it...
        remote.seek(3.0);
        player.render(&mut buf, 0);
        assert!(remote.position() < 4.0);
        // ...but Play after a pause starts inside the loop again.
        remote.pause();
        player.render(&mut buf, 0);
        remote.play();
        player.render(&mut buf, 0);
        assert!(remote.position() >= 4.0, "{}", remote.position());

        // Switched off, the loop no longer holds playback.
        let mut off = p.clone();
        off.transport.loop_region.as_mut().unwrap().on = false;
        remote.set_project(&off);
        remote.seek(6.5);
        player.render(&mut buf, 0);
        assert!(remote.position() > 6.5, "{}", remote.position());
    }

    #[test]
    fn pausing_lets_the_reverb_ring_out() {
        let p = song();
        let (remote, mut player) = pair(&p);
        let mut buf = block();
        remote.play();
        for _ in 0..40 {
            player.render(&mut buf, 0);
        }
        remote.pause();
        // The notes fade out within a few milliseconds, the room keeps ringing.
        for _ in 0..4 {
            player.render(&mut buf, 0);
        }
        assert_eq!(player.seq.active_voices(), 0);
        assert!(peak(&buf) > 1e-4, "tail cut off: {}", peak(&buf));
    }

    #[test]
    fn replaced_projects_are_freed_on_the_ui_thread() {
        let p = song();
        let (remote, mut player) = pair(&p);
        let mut buf = block();
        let mut renamed = p.clone();
        renamed.name = "renamed".into();
        remote.set_project(&renamed);
        player.render(&mut buf, 0);
        assert_eq!(player.project.name, "renamed");
        // The old snapshot came back to the UI side...
        let Ok(Retired::Project(old)) = remote.retired.try_recv() else {
            panic!("old snapshot not returned");
        };
        assert_eq!(old.name, "song");
        // ...and the audio side kept no reference to it.
        assert_eq!(Arc::strong_count(&old), 1);
    }

    #[test]
    fn a_stalled_device_keeps_only_the_newest_project() {
        let p = song();
        let (remote, mut player) = pair(&p);
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
            player.render(&mut buf, 0);
            remote.maintain();
        }
        assert!(remote.backlog.borrow().is_empty());
        assert_eq!(player.project.name, format!("v{}", 2 * QUEUE_LEN - 1));
    }

    #[test]
    fn a_count_in_clicks_then_plays_from_the_playhead() {
        let p = song();
        let (remote, mut player) = pair(&p);
        let mut buf = block();
        remote.seek(4.0);
        remote.count_in(4.0);
        player.render(&mut buf, 0);
        // Counting in: the playhead waits, the first click sounds.
        assert_eq!(remote.position(), 4.0);
        let left = remote.counting_in().unwrap();
        assert!(left > 3.8 && left < 4.0, "{left}");
        assert!(peak(&buf) > 0.1, "click: {}", peak(&buf));
        // Four beats at 120 BPM are two seconds: 86 blocks of 512 frames.
        let mut blocks = 1;
        while remote.counting_in().is_some() {
            player.render(&mut buf, 0);
            blocks += 1;
            assert!(blocks < 100);
        }
        assert_eq!(blocks, 87);
        // Playing now, from beat 4 on.
        let pos = remote.position();
        assert!(pos > 4.0 && pos < 4.05, "{pos}");
        player.render(&mut buf, 0);
        assert!(remote.position() > pos);
    }

    #[test]
    fn the_clock_tells_which_beat_is_heard_when() {
        let p = song();
        let clock = Arc::new(Clock::new());
        let (remote, mut player) = connect(&p, SR, clock.clone());
        let mut buf = block();
        player.render(&mut buf, 1_000);
        // Stopped: the clock stands still.
        assert!(!clock.read().unwrap().moving());
        remote.seek(8.0);
        remote.count_in(4.0);
        // This block will be heard 50 ms from now.
        player.render(&mut buf, 50_000_000);
        let m = clock.read().unwrap();
        assert_eq!((m.heard_ns, m.beat), (50_000_000, 4.0));
        // At 120 BPM, two seconds later the song is at beat 8, its start.
        assert!((m.beat_at(2_050_000_000) - 8.0).abs() < 1e-9);
        remote.pause();
        player.render(&mut buf, 60_000_000);
        assert!(!clock.read().unwrap().moving());
    }

    #[test]
    fn recording_plays_on_past_the_end_of_the_song() {
        let p = song();
        let (remote, mut player) = pair(&p);
        let mut buf = block();
        remote.send(Command::Linear(true));
        remote.seek(7.9);
        remote.play();
        for _ in 0..20 {
            player.render(&mut buf, 0);
        }
        assert!(remote.position() > 8.5, "{}", remote.position());
        remote.send(Command::Linear(false));
        player.render(&mut buf, 0);
        assert!(
            remote.position() < 1.0,
            "loops again: {}",
            remote.position()
        );
    }

    #[test]
    fn midi_keys_play_the_live_track() {
        let p = song();
        let (remote, mut player) = pair(&p);
        let mut buf = block();
        // No live track yet: keys are ignored.
        remote
            .live
            .try_send(Live::On {
                pitch: 60,
                velocity: 100,
            })
            .unwrap();
        player.render(&mut buf, 0);
        assert_eq!(player.seq.live_notes(), 0);
        remote.send(Command::LiveTrack(Some(0)));
        remote
            .live
            .try_send(Live::On {
                pitch: 60,
                velocity: 100,
            })
            .unwrap();
        remote
            .live
            .try_send(Live::On {
                pitch: 64,
                velocity: 90,
            })
            .unwrap();
        player.render(&mut buf, 0);
        assert_eq!(player.seq.live_notes(), 2);
        // The pad fades in slowly; a few blocks later it is clearly there.
        for _ in 0..8 {
            player.render(&mut buf, 0);
        }
        assert!(peak(&buf) > 0.01, "{}", peak(&buf));
        remote.live.try_send(Live::Off { pitch: 60 }).unwrap();
        player.render(&mut buf, 0);
        assert_eq!(player.seq.live_notes(), 1);
        remote.live.try_send(Live::AllOff).unwrap();
        player.render(&mut buf, 0);
        assert_eq!(player.seq.live_notes(), 0);
    }

    #[test]
    fn monitored_input_is_heard_at_its_tracks_level() {
        let mut p = song();
        p.tracks[0].pan = -1.0;
        let (remote, mut player) = pair(&p);
        let (mut tx, feed) = rtrb::RingBuffer::new(8192);
        remote.send(Command::Monitor(Some(Monitor {
            feed,
            track: 0,
            cushion: 64,
            max_cushion: 1024,
            primed: false,
        })));
        let mut buf = block();
        // Not enough input buffered yet: silence.
        for _ in 0..32 {
            tx.push(0.5).unwrap();
        }
        player.render(&mut buf, 0);
        assert_eq!(peak(&buf), 0.0);
        for _ in 0..2048 {
            tx.push(0.5).unwrap();
        }
        player.render(&mut buf, 0);
        // Panned hard left: all of it on the left.
        let [l, r, _] = track_gains_at(&p.tracks[0], false, 0.0);
        let want = 0.5 * l * p.master_gain;
        assert!((buf[0] - want).abs() < 1e-6, "{} vs {want}", buf[0]);
        assert!(buf[1].abs() < 1e-6 && r.abs() < 1e-6);
        // Too much queued up: it skips ahead to stay in time.
        for _ in 0..6000 {
            let _ = tx.push(0.25);
        }
        player.render(&mut buf, 0);
        let m = player.monitor.as_ref().unwrap();
        assert!(m.feed.slots() / 2 <= m.cushion + 512, "{}", m.feed.slots());
        // Switching it off sends the feed back to be freed.
        remote.send(Command::Monitor(None));
        player.render(&mut buf, 0);
        assert!(player.monitor.is_none());
        assert!(matches!(remote.retired.try_recv(), Ok(Retired::Monitor(_))));
    }

    #[test]
    fn the_audio_callback_never_touches_the_heap() {
        use counting::heap_calls;
        let p = song();
        let (remote, mut player) = pair(&p);
        let mut buf = block();
        let mut render = |player: &mut Player| heap_calls(|| player.render(&mut buf, 0));
        // The counter does see allocations.
        assert!(heap_calls(|| drop(std::hint::black_box(vec![0u8; 64]))) >= 2);

        // Previews while stopped.
        remote.preview(0, 60);
        remote.hold(0, 64);
        assert_eq!(render(&mut player), 0);
        remote.release();
        assert_eq!(render(&mut player), 0);

        // A count-in with the metronome on, then playing, seeking into held
        // notes, looping around the end.
        remote.send(Command::Metronome(true));
        remote.count_in(1.0);
        for _ in 0..30 {
            assert_eq!(render(&mut player), 0);
        }
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

        // Sound files arrive, and an audio track plays one.
        let mut pool = AudioPool::new();
        let wave = (0..4000).map(|i| (i as f32 * 0.05).sin() * 0.3).collect();
        pool.insert("a.wav", Arc::new(AudioData::new(SR, 1, wave)));
        let mut q = q.clone();
        apply_all(
            &mut q,
            vec![
                Op::AddTrack {
                    name: "Vox".into(),
                    instrument: Some(aidaw_core::Instrument::Audio),
                    gain: None,
                    pan: None,
                    reverb: None,
                },
                Op::AddAudioClip {
                    track_id: "t3".into(),
                    start: 0.0,
                    length: 8.0,
                    file: "a.wav".into(),
                    offset: 0.0,
                    gain: None,
                    name: None,
                    replace: false,
                },
            ],
        )
        .unwrap();
        remote.set_project(&q);
        remote.send(Command::SetAudio(Arc::new(pool)));
        remote.seek(0.0);
        for _ in 0..20 {
            assert_eq!(render(&mut player), 0);
        }
        while remote.retired.try_recv().is_ok() {}

        // Keys on a MIDI keyboard, with the pedal.
        remote.send(Command::LiveTrack(Some(0)));
        assert_eq!(render(&mut player), 0);
        remote
            .live
            .try_send(Live::On {
                pitch: 60,
                velocity: 100,
            })
            .unwrap();
        remote.live.try_send(Live::Pedal(true)).unwrap();
        remote.live.try_send(Live::Off { pitch: 60 }).unwrap();
        assert_eq!(render(&mut player), 0);
        remote.live.try_send(Live::Pedal(false)).unwrap();
        assert_eq!(render(&mut player), 0);

        // Monitoring input, made and freed on this thread.
        let (mut tx, feed) = rtrb::RingBuffer::new(8192);
        remote.send(Command::Monitor(Some(Monitor {
            feed,
            track: 0,
            cushion: 64,
            max_cushion: 1024,
            primed: false,
        })));
        for _ in 0..4 {
            for _ in 0..1024 {
                let _ = tx.push(0.1);
            }
            assert_eq!(render(&mut player), 0);
        }
        remote.send(Command::Monitor(None));
        assert_eq!(render(&mut player), 0);
        while remote.retired.try_recv().is_ok() {}

        // Many snapshots without the UI collecting: they wait in the graveyard.
        for _ in 0..RETIRED_LEN + 8 {
            remote
                .commands
                .try_send(Command::SetProject {
                    project: Arc::new(q.clone()),
                    looping: q.playback_loop(),
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

    #[test]
    fn the_input_callback_never_touches_the_heap_either() {
        use counting::heap_calls;
        let clock = Arc::new(Clock::new());
        let (mut cap, commands, returned) = input::tests::capture(clock.clone());
        clock.publish(Moment {
            heard_ns: 0,
            beat: 0.0,
            beats_per_ns: 2e-9,
        });
        let input: Vec<f32> = (0..2048).map(|i| (i as f32 * 0.01).sin()).collect();
        let process = |cap: &mut input::Capture| heap_calls(|| cap.process(&input, 2, 0));
        let _take = input::tests::start(&commands, 1 << 14, &[1, 2]);
        let _monitor = input::tests::tap(&commands, 1 << 14, &[1]);
        for _ in 0..8 {
            assert_eq!(process(&mut cap), 0);
        }
        input::tests::stop_all(&cap, &commands);
        assert_eq!(process(&mut cap), 0);
        assert_eq!(returned.try_iter().count(), 2);
    }
}
