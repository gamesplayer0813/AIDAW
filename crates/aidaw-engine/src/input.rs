//! Recording: an input stream (microphone, line in, the inputs of an audio
//! interface) that meters every channel, feeds live monitoring and
//! captures takes to 24-bit WAV files.
//!
//! Like playback, the input callback never blocks and never allocates:
//! - Takes and monitor feeds reach it through a bounded queue, and the ring
//!   buffers it has finished with go back to the UI thread to be freed.
//! - Samples leave it through lock-free ring buffers: to a writer thread
//!   that saves the take, and to the output callback for monitoring.
//! - Levels and counters are atomics.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use aidaw_core::audio::to_i24;
use aidaw_core::AudioData;
use anyhow::{anyhow, bail, Context, Result};
use cpal::traits::{DeviceTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat, SizedSample};
use rtrb::{Consumer, Producer, RingBuffer};

use crate::clock::{self, Clock};

/// Channels with a level meter (an interface with more still records them).
pub const MAX_METERED: usize = 32;
/// How long the writer thread may fall behind before samples are lost.
const TAKE_RING_SECONDS: f64 = 4.0;
/// Most input the monitor buffers before it skips ahead (keeps it snappy).
const MONITOR_RING_SECONDS: f64 = 0.2;
/// How long a stopped take waits for the input callback to let go.
const STOP_GRACE: Duration = Duration::from_millis(500);
/// Samples converted at once in the callback (and so never reallocated).
const SCRATCH: usize = 1 << 16;
const QUEUE_LEN: usize = 16;
const RETURNED_LEN: usize = 16;

/// What the UI hands the input callback.
pub(crate) enum ToCallback {
    Take(Feed),
    Monitor(Option<Tap>),
}

/// What the callback hands back, to be freed on the UI thread.
pub(crate) enum FromCallback {
    Feed(#[allow(dead_code)] Feed),
    Tap(#[allow(dead_code)] Tap),
}

/// Which device channels (0-based) a take or the monitor uses: one (mono)
/// or two (stereo).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Channels {
    first: usize,
    second: Option<usize>,
}

impl Channels {
    /// From a track's 1-based input list; the first input when empty.
    /// Channels the device doesn't have are clamped to its last one.
    pub(crate) fn pick(inputs: &[u16], device_channels: u16) -> Channels {
        let last = device_channels.max(1) as usize - 1;
        let at = |i: usize| inputs.get(i).map(|&c| (c.max(1) as usize - 1).min(last));
        let first = at(0).unwrap_or(0);
        Channels {
            first,
            second: at(1).filter(|&c| c != first),
        }
    }

    pub(crate) fn count(&self) -> usize {
        1 + self.second.is_some() as usize
    }
}

/// A take being captured: where the callback writes and what it reports.
pub(crate) struct Feed {
    ring: Producer<f32>,
    channels: Channels,
    state: Arc<TakeState>,
}

/// Input for live monitoring, stereo, on its way to the output callback.
pub(crate) struct Tap {
    ring: Producer<f32>,
    channels: Channels,
}

/// Shared between the UI, the input callback and the writer of one take.
#[derive(Default)]
struct TakeState {
    /// Set by the UI: the callback lets go and the writer finishes.
    stop: AtomicBool,
    /// Set by the writer when it couldn't write (disk full, file gone).
    failed: AtomicBool,
    /// Frames the callback has seen since the take began.
    frames: AtomicU64,
    /// Frames lost because the writer fell behind.
    dropped: AtomicU64,
    /// One frame of the take and the song beat it was heard at.
    synced: AtomicBool,
    sync_frame: AtomicU64,
    sync_beat: AtomicU64,
    sync_beats_per_ns: AtomicU64,
}

impl TakeState {
    /// The song beat at the take's first frame (constant tempo assumed).
    fn first_beat(&self, sample_rate: u32) -> Option<f64> {
        if !self.synced.load(Ordering::Acquire) {
            return None;
        }
        let frame = self.sync_frame.load(Ordering::Relaxed) as f64;
        let beat = f64::from_bits(self.sync_beat.load(Ordering::Relaxed));
        let per_ns = f64::from_bits(self.sync_beats_per_ns.load(Ordering::Relaxed));
        Some(beat - frame / sample_rate.max(1) as f64 * per_ns * 1e9)
    }
}

/// Levels and health, written by the callback.
struct Shared {
    /// f32 bits of the loudest sample per channel since the UI last looked.
    peaks: [AtomicU32; MAX_METERED],
    callbacks: AtomicU64,
    /// How long ago the newest block was captured when it arrived, in µs.
    capture_delay_us: AtomicU32,
    error: Mutex<Option<String>>,
}

/// The input callback's side: kept apart from cpal so tests can drive it.
pub(crate) struct Capture {
    commands: Receiver<ToCallback>,
    returned: SyncSender<FromCallback>,
    /// Finished feeds that did not fit in `returned` yet.
    graveyard: Vec<FromCallback>,
    shared: Arc<Shared>,
    clock: Arc<Clock>,
    feed: Option<Feed>,
    tap: Option<Tap>,
    /// Frames of the current take so far.
    pushed: u64,
}

impl Capture {
    /// One block of interleaved input whose first frame was captured at
    /// `captured_ns` (on the song clock).
    pub(crate) fn process(&mut self, block: &[f32], channels: usize, captured_ns: u64) {
        self.drain_commands();
        let channels = channels.max(1);
        let frames = block.len() / channels;
        let block = &block[..frames * channels];
        for c in 0..channels.min(MAX_METERED) {
            let peak = block
                .iter()
                .skip(c)
                .step_by(channels)
                .fold(0.0f32, |m, s| m.max(s.abs()));
            // Bits of non-negative floats sort like the floats themselves.
            self.shared.peaks[c].fetch_max(peak.to_bits(), Ordering::Relaxed);
        }

        if self
            .feed
            .as_ref()
            .is_some_and(|f| f.state.stop.load(Ordering::Acquire))
        {
            if let Some(feed) = self.feed.take() {
                self.give_back(FromCallback::Feed(feed));
            }
        }
        if let Some(feed) = &mut self.feed {
            let state = &feed.state;
            if !state.synced.load(Ordering::Relaxed) {
                if let Some(m) = self.clock.read().filter(|m| m.moving()) {
                    state.sync_frame.store(self.pushed, Ordering::Relaxed);
                    state
                        .sync_beat
                        .store(m.beat_at(captured_ns).to_bits(), Ordering::Relaxed);
                    state
                        .sync_beats_per_ns
                        .store(m.beats_per_ns.to_bits(), Ordering::Relaxed);
                    state.synced.store(true, Ordering::Release);
                }
            }
            let written = push(&mut feed.ring, block, channels, feed.channels, false);
            self.pushed += frames as u64;
            state.frames.store(self.pushed, Ordering::Relaxed);
            if written < frames {
                state
                    .dropped
                    .fetch_add((frames - written) as u64, Ordering::Relaxed);
            }
        }
        if let Some(tap) = &mut self.tap {
            push(&mut tap.ring, block, channels, tap.channels, true);
        }
    }

    fn drain_commands(&mut self) {
        while let Some(old) = self.graveyard.pop() {
            if let Err(TrySendError::Full(old)) = self.returned.try_send(old) {
                self.graveyard.push(old);
                break;
            }
        }
        while let Ok(cmd) = self.commands.try_recv() {
            match cmd {
                ToCallback::Take(feed) => {
                    self.pushed = 0;
                    if let Some(old) = self.feed.replace(feed) {
                        self.give_back(FromCallback::Feed(old));
                    }
                }
                ToCallback::Monitor(tap) => {
                    if let Some(old) = std::mem::replace(&mut self.tap, tap) {
                        self.give_back(FromCallback::Tap(old));
                    }
                }
            }
        }
    }

    fn give_back(&mut self, item: FromCallback) {
        if let Err(TrySendError::Full(item)) = self.returned.try_send(item) {
            if self.graveyard.len() < self.graveyard.capacity() {
                self.graveyard.push(item);
            }
        }
    }
}

/// Write the chosen channels of `block` into `ring` (as stereo pairs when
/// `stereo`, doubling a mono input). Returns the frames that fit.
fn push(
    ring: &mut Producer<f32>,
    block: &[f32],
    channels: usize,
    pick: Channels,
    stereo: bool,
) -> usize {
    let per_frame = if stereo { 2 } else { pick.count() };
    let frames = block.len() / channels;
    let room = ring.slots() / per_frame;
    let n = frames.min(room);
    if n == 0 {
        return 0;
    }
    let Ok(chunk) = ring.write_chunk_uninit(n * per_frame) else {
        return 0;
    };
    let frames = block.chunks_exact(channels).take(n);
    let a = pick.first;
    match (pick.second, stereo) {
        (Some(b), _) => chunk.fill_from_iter(frames.flat_map(|f| [f[a], f[b]])),
        (None, true) => chunk.fill_from_iter(frames.flat_map(|f| [f[a], f[a]])),
        (None, false) => chunk.fill_from_iter(frames.map(|f| f[a])),
    };
    n
}

/// A finished take.
pub struct Take {
    pub path: PathBuf,
    pub audio: Arc<AudioData>,
    /// The song beat heard when the take's first sample was captured
    /// (already corrected for the devices' latency). `None` if the song
    /// never moved while it recorded.
    pub first_beat: Option<f64>,
    /// Frames lost because the disk was too slow (0 normally).
    pub dropped_frames: u64,
}

/// A take being recorded: the callback's state and the writer thread.
struct Recording {
    state: Arc<TakeState>,
    writer: JoinHandle<Result<Arc<AudioData>>>,
    path: PathBuf,
}

/// An open input device.
pub struct Input {
    stream: cpal::Stream,
    shared: Arc<Shared>,
    commands: SyncSender<ToCallback>,
    returned: Receiver<FromCallback>,
    name: String,
    channels: u16,
    sample_rate: u32,
    format: String,
    recording: Option<Recording>,
    /// Feeds sent to the callback but not yet seen there (the device
    /// stalled); kept so nothing is lost.
    backlog: Vec<ToCallback>,
}

impl Input {
    /// Open `device` for input, preferring `rate` (the output's sample
    /// rate, so monitoring needs no conversion) when the device offers it.
    pub(crate) fn open(device: cpal::Device, clock: Arc<Clock>, rate: u32) -> Result<Input> {
        let name = crate::devices::name_of(&device);
        let config = pick_config(&device, rate)?;
        let sample_rate = config.sample_rate();
        let channels = config.channels();
        let format = format!(
            "{:?}, {} ch, {} Hz",
            config.sample_format(),
            channels,
            sample_rate
        );
        let (commands, commands_rx) = mpsc::sync_channel(QUEUE_LEN);
        let (returned_tx, returned) = mpsc::sync_channel(RETURNED_LEN);
        let shared = Arc::new(Shared {
            peaks: std::array::from_fn(|_| AtomicU32::new(0)),
            callbacks: AtomicU64::new(0),
            capture_delay_us: AtomicU32::new(0),
            error: Mutex::new(None),
        });
        let capture = Capture {
            commands: commands_rx,
            returned: returned_tx,
            graveyard: Vec::with_capacity(RETURNED_LEN),
            shared: shared.clone(),
            clock,
            feed: None,
            tap: None,
            pushed: 0,
        };
        let sc = config.config();
        let stream = match config.sample_format() {
            SampleFormat::F32 => build::<f32>(&device, sc, capture),
            SampleFormat::I16 => build::<i16>(&device, sc, capture),
            SampleFormat::I24 => build::<cpal::I24>(&device, sc, capture),
            SampleFormat::I32 => build::<i32>(&device, sc, capture),
            SampleFormat::U16 => build::<u16>(&device, sc, capture),
            SampleFormat::F64 => build::<f64>(&device, sc, capture),
            SampleFormat::I8 => build::<i8>(&device, sc, capture),
            SampleFormat::U8 => build::<u8>(&device, sc, capture),
            other => bail!("unsupported input sample format {other:?}"),
        }
        .with_context(|| format!("opening input {name:?} ({format})"))?;
        stream.play().context("starting input")?;
        Ok(Input {
            stream,
            shared,
            commands,
            returned,
            name,
            channels,
            sample_rate,
            format,
            recording: None,
            backlog: Vec::new(),
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn channels(&self) -> u16 {
        self.channels
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn format(&self) -> &str {
        &self.format
    }

    /// Times the device has delivered input; 0 means it isn't running.
    pub fn callbacks(&self) -> u64 {
        self.shared.callbacks.load(Ordering::Relaxed)
    }

    /// The input side's latency as the device reports it.
    pub fn latency(&self) -> Duration {
        Duration::from_micros(self.shared.capture_delay_us.load(Ordering::Relaxed) as u64)
    }

    pub fn error(&self) -> Option<String> {
        self.shared.error.lock().ok().and_then(|e| e.clone())
    }

    /// The loudest sample of each channel since the last call (0..1).
    pub fn take_peaks(&self) -> Vec<f32> {
        (0..(self.channels as usize).min(MAX_METERED))
            .map(|c| f32::from_bits(self.shared.peaks[c].swap(0, Ordering::Relaxed)))
            .collect()
    }

    /// Deliver what waited for room and free what the callback is done with.
    pub(crate) fn maintain(&mut self) {
        while self.returned.try_recv().is_ok() {}
        let mut pending = std::mem::take(&mut self.backlog).into_iter();
        for cmd in pending.by_ref() {
            match self.commands.try_send(cmd) {
                Ok(()) => {}
                Err(TrySendError::Full(cmd)) => {
                    self.backlog.push(cmd);
                    break;
                }
                Err(TrySendError::Disconnected(_)) => break,
            }
        }
        self.backlog.extend(pending);
    }

    fn send(&mut self, cmd: ToCallback) {
        self.backlog.push(cmd);
        self.maintain();
    }

    /// Start capturing `inputs` (1-based device channels; one for mono, two
    /// for stereo) into a new WAV file at `path`.
    pub(crate) fn start_take(&mut self, path: &Path, inputs: &[u16]) -> Result<()> {
        if self.recording.is_some() {
            bail!("already recording");
        }
        let channels = Channels::pick(inputs, self.channels);
        let n = channels.count();
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir)
                .with_context(|| format!("creating the folder {}", dir.display()))?;
        }
        let spec = hound::WavSpec {
            channels: n as u16,
            sample_rate: self.sample_rate,
            bits_per_sample: 24,
            sample_format: hound::SampleFormat::Int,
        };
        // Created here so a bad path fails now, not after the take.
        let wav = hound::WavWriter::create(path, spec)
            .with_context(|| format!("creating {}", path.display()))?;
        let capacity = (self.sample_rate as f64 * TAKE_RING_SECONDS) as usize * n;
        let (ring, consumer) = RingBuffer::new(capacity.max(4096));
        let state = Arc::new(TakeState::default());
        let writer = {
            let state = state.clone();
            let rate = self.sample_rate;
            std::thread::Builder::new()
                .name("aidaw take writer".into())
                .spawn(move || write_take(consumer, wav, rate, n, &state))
                .context("starting the take writer")?
        };
        self.send(ToCallback::Take(Feed {
            ring,
            channels,
            state: state.clone(),
        }));
        self.recording = Some(Recording {
            state,
            writer,
            path: path.to_path_buf(),
        });
        Ok(())
    }

    pub(crate) fn is_recording(&self) -> bool {
        self.recording.is_some()
    }

    /// Seconds captured so far in the current take.
    pub(crate) fn take_seconds(&self) -> Option<f64> {
        let r = self.recording.as_ref()?;
        Some(r.state.frames.load(Ordering::Relaxed) as f64 / self.sample_rate.max(1) as f64)
    }

    /// Whether the take can't be written (the disk is full, say).
    pub(crate) fn take_failed(&self) -> bool {
        self.recording
            .as_ref()
            .is_some_and(|r| r.state.failed.load(Ordering::Relaxed))
    }

    /// Ask the take to finish; [`Self::finished_take`] picks it up.
    pub(crate) fn stop_take(&mut self) {
        if let Some(r) = &self.recording {
            r.state.stop.store(true, Ordering::Release);
        }
    }

    /// The take, once its file is complete (after [`Self::stop_take`]).
    pub(crate) fn finished_take(&mut self) -> Option<Result<Take>> {
        self.maintain();
        if !self.recording.as_ref()?.writer.is_finished() {
            return None;
        }
        let r = self.recording.take()?;
        let joined = r
            .writer
            .join()
            .map_err(|_| anyhow!("the take writer crashed"));
        Some(joined.and_then(|written| {
            let audio = written?;
            Ok(Take {
                path: r.path,
                audio,
                first_beat: r.state.first_beat(self.sample_rate),
                dropped_frames: r.state.dropped.load(Ordering::Relaxed),
            })
        }))
    }

    /// Start feeding `inputs` to the output for monitoring; returns the
    /// output callback's end of the feed.
    pub(crate) fn tap(&mut self, inputs: &[u16]) -> Consumer<f32> {
        let capacity = (self.sample_rate as f64 * MONITOR_RING_SECONDS) as usize * 2;
        let (ring, consumer) = RingBuffer::new(capacity.max(1024));
        let channels = Channels::pick(inputs, self.channels);
        self.send(ToCallback::Monitor(Some(Tap { ring, channels })));
        consumer
    }

    pub(crate) fn untap(&mut self) {
        self.send(ToCallback::Monitor(None));
    }

    pub(crate) fn pause(&self) {
        let _ = self.stream.pause();
    }
}

impl Drop for Input {
    fn drop(&mut self) {
        // Don't leave a writer thread waiting for samples forever.
        self.stop_take();
    }
}

/// The writer thread: everything the callback captures goes to the WAV
/// file and stays in memory, so the take plays as soon as it is done.
fn write_take(
    mut ring: Consumer<f32>,
    mut wav: hound::WavWriter<std::io::BufWriter<std::fs::File>>,
    sample_rate: u32,
    channels: usize,
    state: &TakeState,
) -> Result<Arc<AudioData>> {
    let mut samples: Vec<f32> = Vec::with_capacity(sample_rate as usize * channels * 30);
    let mut stopping: Option<Instant> = None;
    let mut save = |a: &[f32], samples: &mut Vec<f32>| -> Result<()> {
        for &s in a {
            wav.write_sample(to_i24(s))?;
        }
        samples.extend_from_slice(a);
        Ok(())
    };
    let result = loop {
        let n = ring.slots();
        if n > 0 {
            let chunk = ring.read_chunk(n).expect("the slots are there");
            let (a, b) = chunk.as_slices();
            let saved = save(a, &mut samples).and_then(|()| save(b, &mut samples));
            chunk.commit_all();
            if let Err(e) = saved {
                break Err(e);
            }
            continue;
        }
        if state.stop.load(Ordering::Acquire) {
            if ring.is_abandoned() {
                break Ok(());
            }
            let since = *stopping.get_or_insert_with(Instant::now);
            if since.elapsed() > STOP_GRACE {
                break Ok(());
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    if let Err(e) = result {
        state.failed.store(true, Ordering::Relaxed);
        return Err(e.context("writing the recording"));
    }
    wav.finalize().context("finishing the recording")?;
    Ok(Arc::new(AudioData::new(sample_rate, channels, samples)))
}

/// The device's default input format, at `rate` if it supports that.
fn pick_config(device: &cpal::Device, rate: u32) -> Result<cpal::SupportedStreamConfig> {
    let default = device
        .default_input_config()
        .map_err(|e| anyhow!("the device has no usable input format: {e}"));
    if let Ok(d) = &default {
        if d.sample_rate() == rate {
            return default;
        }
        let same = device.supported_input_configs().ok().and_then(|mut it| {
            it.find(|c| {
                c.channels() == d.channels()
                    && c.sample_format() == d.sample_format()
                    && c.min_sample_rate() <= rate
                    && rate <= c.max_sample_rate()
            })
        });
        if let Some(c) = same {
            return Ok(c.with_sample_rate(rate));
        }
    }
    default
}

fn build<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    mut capture: Capture,
) -> Result<cpal::Stream, cpal::Error>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let channels = config.channels.max(1) as usize;
    let rate = config.sample_rate.max(1) as u64;
    let shared = capture.shared.clone();
    let err_shared = capture.shared.clone();
    let clock = capture.clock.clone();
    let mut scratch: Vec<f32> = Vec::with_capacity(SCRATCH);
    let per_piece = (SCRATCH / channels).max(1) * channels;
    device.build_input_stream::<T, _, _>(
        config,
        move |data: &[T], info: &cpal::InputCallbackInfo| {
            shared.callbacks.fetch_add(1, Ordering::Relaxed);
            let ts = info.timestamp();
            let age = clock::delay(ts.callback, ts.capture);
            shared
                .capture_delay_us
                .store(age.as_micros() as u32, Ordering::Relaxed);
            let mut captured_ns = clock.now_ns().saturating_sub(age.as_nanos() as u64);
            for piece in data.chunks(per_piece) {
                scratch.clear();
                scratch.extend(piece.iter().map(|s| f32::from_sample(*s)));
                capture.process(&scratch, channels, captured_ns);
                captured_ns += (piece.len() / channels) as u64 * 1_000_000_000 / rate;
            }
        },
        move |err| {
            if crate::is_harmless(&err) {
                return;
            }
            if let Ok(mut e) = err_shared.error.lock() {
                *e = Some(err.to_string());
            }
        },
        None,
    )
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::clock::Moment;

    /// A capture end without a device, plus the UI's ends of its queues.
    pub(crate) fn capture(
        clock: Arc<Clock>,
    ) -> (Capture, SyncSender<ToCallback>, Receiver<FromCallback>) {
        let (commands, commands_rx) = mpsc::sync_channel(QUEUE_LEN);
        let (returned_tx, returned) = mpsc::sync_channel(RETURNED_LEN);
        let shared = Arc::new(Shared {
            peaks: std::array::from_fn(|_| AtomicU32::new(0)),
            callbacks: AtomicU64::new(0),
            capture_delay_us: AtomicU32::new(0),
            error: Mutex::new(None),
        });
        let c = Capture {
            commands: commands_rx,
            returned: returned_tx,
            graveyard: Vec::with_capacity(RETURNED_LEN),
            shared,
            clock,
            feed: None,
            tap: None,
            pushed: 0,
        };
        (c, commands, returned)
    }

    /// Start a take of `inputs` into a ring of `capacity` samples.
    pub(crate) fn start(
        commands: &SyncSender<ToCallback>,
        capacity: usize,
        inputs: &[u16],
    ) -> Consumer<f32> {
        let (ring, consumer) = RingBuffer::new(capacity);
        let feed = Feed {
            ring,
            channels: Channels::pick(inputs, 2),
            state: Arc::new(TakeState::default()),
        };
        commands.send(ToCallback::Take(feed)).unwrap();
        consumer
    }

    /// Monitor `inputs` into a ring of `capacity` samples.
    pub(crate) fn tap(
        commands: &SyncSender<ToCallback>,
        capacity: usize,
        inputs: &[u16],
    ) -> Consumer<f32> {
        let (ring, consumer) = RingBuffer::new(capacity);
        let channels = Channels::pick(inputs, 2);
        commands
            .send(ToCallback::Monitor(Some(Tap { ring, channels })))
            .unwrap();
        consumer
    }

    /// Stop the take and the monitor.
    pub(crate) fn stop_all(cap: &Capture, commands: &SyncSender<ToCallback>) {
        if let Some(f) = &cap.feed {
            f.state.stop.store(true, Ordering::Release);
        }
        commands.send(ToCallback::Monitor(None)).unwrap();
    }

    /// Four channels; channel c of frame f holds c + f / 1000.
    fn block(frames: usize) -> Vec<f32> {
        (0..frames)
            .flat_map(|f| (0..4).map(move |c| c as f32 + f as f32 / 1000.0))
            .collect()
    }

    #[test]
    fn track_inputs_pick_device_channels() {
        let ch = |first, second| Channels { first, second };
        assert_eq!(Channels::pick(&[], 2), ch(0, None));
        assert_eq!(Channels::pick(&[2], 2), ch(1, None));
        assert_eq!(Channels::pick(&[1, 2], 2), ch(0, Some(1)));
        // Inputs the device doesn't have fall back to its last one.
        assert_eq!(Channels::pick(&[5, 6], 2), ch(1, None));
        assert_eq!(Channels::pick(&[3, 4], 8), ch(2, Some(3)));
        assert_eq!(Channels::pick(&[0], 1), ch(0, None));
    }

    #[test]
    fn a_take_captures_the_chosen_channels_and_where_they_were_heard() {
        let clock = Arc::new(Clock::new());
        let (mut cap, commands, returned) = capture(clock.clone());
        let (ring, mut out) = RingBuffer::new(4096);
        let state = Arc::new(TakeState::default());
        commands
            .send(ToCallback::Take(Feed {
                ring,
                channels: Channels::pick(&[2, 4], 4),
                state: state.clone(),
            }))
            .unwrap();
        // The song isn't moving yet: no sync point.
        cap.process(&block(100), 4, 1_000);
        assert!(!state.synced.load(Ordering::Relaxed));
        // Playing at 120 BPM: beat 4 is heard at t = 10 ms.
        clock.publish(Moment {
            heard_ns: 10_000_000,
            beat: 4.0,
            beats_per_ns: 2.0 / 1e9,
        });
        // Frame 100 was captured at t = 35 ms, so it was played on beat 4.05.
        cap.process(&block(100), 4, 35_000_000);
        let first = state.first_beat(1000).unwrap();
        // 100 frames earlier at 1 kHz is 0.1 s = 0.2 beats before that.
        assert!((first - 3.85).abs() < 1e-9, "{first}");
        assert_eq!(state.frames.load(Ordering::Relaxed), 200);
        let samples: Vec<f32> = std::iter::from_fn(|| out.pop().ok()).collect();
        assert_eq!(samples.len(), 400);
        assert_eq!(&samples[..4], &[1.0, 3.0, 1.001, 3.001]);

        // Stopping hands the feed back for the UI to free.
        state.stop.store(true, Ordering::Release);
        cap.process(&block(10), 4, 40_000_000);
        assert!(cap.feed.is_none());
        assert!(matches!(returned.try_recv(), Ok(FromCallback::Feed(_))));
        assert!(out.is_abandoned() || std::iter::from_fn(|| out.pop().ok()).count() == 0);
    }

    #[test]
    fn a_full_ring_counts_lost_frames_and_meters_keep_going() {
        let (mut cap, commands, _returned) = capture(Arc::new(Clock::new()));
        let (ring, _out) = RingBuffer::new(150);
        let state = Arc::new(TakeState::default());
        commands
            .send(ToCallback::Take(Feed {
                ring,
                channels: Channels::pick(&[1], 4),
                state: state.clone(),
            }))
            .unwrap();
        cap.process(&block(100), 4, 0);
        cap.process(&block(100), 4, 0);
        assert_eq!(state.dropped.load(Ordering::Relaxed), 50);
        let peak = |c: usize| f32::from_bits(cap.shared.peaks[c].load(Ordering::Relaxed));
        assert!((peak(3) - 3.099).abs() < 1e-6, "{}", peak(3));
    }

    #[test]
    fn the_monitor_gets_stereo_pairs() {
        let (mut cap, commands, returned) = capture(Arc::new(Clock::new()));
        let (ring, mut out) = RingBuffer::new(64);
        commands
            .send(ToCallback::Monitor(Some(Tap {
                ring,
                channels: Channels::pick(&[3], 4),
            })))
            .unwrap();
        cap.process(&block(2), 4, 0);
        let samples: Vec<f32> = std::iter::from_fn(|| out.pop().ok()).collect();
        assert_eq!(samples, [2.0, 2.0, 2.001, 2.001]);
        commands.send(ToCallback::Monitor(None)).unwrap();
        cap.process(&block(2), 4, 0);
        assert!(matches!(returned.try_recv(), Ok(FromCallback::Tap(_))));
    }

    #[test]
    fn the_writer_saves_a_wav_and_keeps_it_in_memory() {
        let dir = std::env::temp_dir().join(format!("aidaw-take-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("take.wav");
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: 8000,
            bits_per_sample: 24,
            sample_format: hound::SampleFormat::Int,
        };
        let wav = hound::WavWriter::create(&path, spec).unwrap();
        let (mut ring, consumer) = RingBuffer::new(1024);
        let state = Arc::new(TakeState::default());
        let writer = {
            let state = state.clone();
            std::thread::spawn(move || write_take(consumer, wav, 8000, 2, &state))
        };
        for i in 0..800 {
            let v = (i % 100) as f32 / 200.0;
            while ring.push(v).is_err() {
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        state.stop.store(true, Ordering::Release);
        drop(ring);
        let audio = writer.join().unwrap().unwrap();
        assert_eq!((audio.channels(), audio.frames()), (2, 400));
        let back = aidaw_core::audio::load_wav(&path).unwrap();
        assert_eq!(back.frames(), 400);
        assert!((back.frame(1).0 - 2.0 / 200.0).abs() < 1e-6);
        assert!((back.frame(1).1 - 3.0 / 200.0).abs() < 1e-6);
    }
}
