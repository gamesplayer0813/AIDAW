//! Recording in the app: arming tracks, Record / Stop, the count-in,
//! monitoring, MIDI keyboards, and the sound files audio clips play
//! (loading them, placing new takes, importing WAVs).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use aidaw_core::audio::{self, AudioData, AudioPool};
use aidaw_core::edit::Op;
use aidaw_core::{Clip, Instrument, Note};
use aidaw_engine::{MidiEvent, Take};

use crate::app::App;
use crate::i18n::{tr, trf};

/// A recording in progress.
pub struct Recording {
    /// Where the playhead was when Record was pressed: takes start here.
    pub punch_in: f64,
    /// The audio track the input is recorded onto, if any.
    pub audio_track: Option<String>,
    /// The instrument track MIDI keyboard notes go onto, if any.
    pub midi_track: Option<String>,
    /// Notes played so far: pitch, velocity, start and end beat.
    pub notes: Vec<(u8, u8, f64, f64)>,
    /// Keys down (or held by the pedal): pitch -> (velocity, start beat).
    held: HashMap<u8, (u8, f64)>,
    /// Keys let go while the pedal was down: they end when it comes up.
    sustained: HashSet<u8>,
    pedal: bool,
}

impl Recording {
    fn new(punch_in: f64, audio_track: Option<String>, midi_track: Option<String>) -> Self {
        Recording {
            punch_in,
            audio_track,
            midi_track,
            notes: Vec::new(),
            held: HashMap::new(),
            sustained: HashSet::new(),
            pedal: false,
        }
    }

    /// A key or the pedal on a MIDI keyboard, while the song moves.
    pub fn midi(&mut self, event: MidiEvent) {
        match event {
            MidiEvent::NoteOn {
                pitch,
                velocity,
                beat: Some(beat),
            } => {
                // Striking a key that still sounds ends the old note.
                self.end(pitch, beat);
                self.held.insert(pitch, (velocity, beat));
            }
            MidiEvent::NoteOff {
                pitch,
                beat: Some(beat),
            } => {
                if self.pedal {
                    self.sustained.insert(pitch);
                } else {
                    self.end(pitch, beat);
                }
            }
            MidiEvent::Pedal {
                down,
                beat: Some(beat),
            } => {
                self.pedal = down;
                if !down {
                    for pitch in std::mem::take(&mut self.sustained) {
                        self.end(pitch, beat);
                    }
                }
            }
            _ => {}
        }
    }

    fn end(&mut self, pitch: u8, beat: f64) {
        self.sustained.remove(&pitch);
        if let Some((velocity, start)) = self.held.remove(&pitch) {
            self.notes.push((pitch, velocity, start, beat.max(start)));
        }
    }

    /// End every note still sounding at `beat`; the notes in play order.
    fn finish(&mut self, beat: f64) -> Vec<(u8, u8, f64, f64)> {
        let held: Vec<u8> = self.held.keys().copied().collect();
        for pitch in held {
            self.end(pitch, beat);
        }
        let mut notes = std::mem::take(&mut self.notes);
        notes.sort_by(|a, b| a.2.total_cmp(&b.2).then(a.0.cmp(&b.0)));
        notes
    }
}

/// A clip for the notes of a MIDI take: it starts on the bar of the punch-in
/// (or of an early first note) and ends on the bar line after the last one.
/// `None` when nothing was played.
pub fn midi_clip(
    notes: &[(u8, u8, f64, f64)],
    punch_in: f64,
    stop: f64,
    beats_per_bar: f64,
) -> Option<(f64, f64, Vec<Note>)> {
    let first = notes.iter().map(|n| n.2).fold(f64::INFINITY, f64::min);
    if !first.is_finite() {
        return None;
    }
    let bar = beats_per_bar.max(1.0);
    let start = ((first.min(punch_in) / bar).floor() * bar).max(0.0);
    let end = notes.iter().map(|n| n.3).fold(stop, f64::max);
    let length = (((end - start) / bar).ceil() * bar).max(bar);
    let notes = notes
        .iter()
        .map(|&(pitch, velocity, from, to)| {
            let from = (from - start).max(0.0);
            Note {
                pitch,
                start: from,
                // A tap shorter than a 64th still gets a hearable length.
                duration: (to - start - from).max(1.0 / 16.0),
                velocity: velocity.max(1),
            }
        })
        .collect();
    Some((start, length, notes))
}

/// Where a take lands: the clip starts at the punch-in (or later, when the
/// audio began later) and the part of the file before it is trimmed.
/// Returns (start beat, offset seconds, length beats).
pub fn place_audio(
    first_beat: f64,
    punch_in: f64,
    seconds: f64,
    seconds_per_beat: f64,
) -> (f64, f64, f64) {
    let start = first_beat.max(punch_in).max(0.0);
    let offset = ((start - first_beat) * seconds_per_beat).clamp(0.0, seconds);
    let length = (seconds - offset) / seconds_per_beat;
    (start, offset, length)
}

/// A file name made from a track name (Windows forbids a few characters).
fn file_safe(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| {
            if "\\/:*?\"<>|".contains(c) || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();
    let s = s.trim().trim_end_matches('.').trim().to_string();
    if s.is_empty() {
        "Take".into()
    } else {
        s
    }
}

/// A take whose file the writer thread is finishing.
pub struct PendingTake {
    track_id: String,
    punch_in: f64,
}

/// What a background load brings back: the files read, by name, and the
/// ones that could not be read, with why.
type Loaded = (Vec<(String, Arc<AudioData>)>, Vec<(String, String)>);

/// Sound files loading on a background thread.
pub struct AudioLoad {
    rx: mpsc::Receiver<Loaded>,
}

impl App {
    /// The project's folder: where relative recording paths start.
    pub fn folder(&self) -> Option<&Path> {
        self.path.as_deref().and_then(Path::parent)
    }

    /// The folder new recordings go to: "<song> audio" next to the song.
    fn takes_folder(&self) -> Option<PathBuf> {
        let path = self.path.as_ref()?;
        let name = path.file_name()?.to_string_lossy();
        let stem = name
            .strip_suffix(".aidaw.json")
            .or_else(|| name.strip_suffix(".json"))
            .unwrap_or(&name);
        Some(path.with_file_name(format!("{stem} audio")))
    }

    /// A file name for a new take on `track_name` that is not taken yet.
    fn take_path(&self, track_name: &str) -> Option<PathBuf> {
        let dir = self.takes_folder()?;
        let base = file_safe(track_name);
        (1..10_000)
            .map(|n| dir.join(format!("{base} {n}.wav")))
            .find(|p| !p.exists() && !self.audio.contains(&audio::file_name_for(self.folder(), p)))
    }

    pub fn is_recording(&self) -> bool {
        self.recording.is_some()
    }

    pub fn is_armed(&self, track_id: &str) -> bool {
        self.armed.contains(track_id)
    }

    pub fn toggle_arm(&mut self, track_id: &str) {
        if !self.armed.remove(track_id) {
            // One track records audio at a time (one input); the same for MIDI.
            let audio = self
                .project
                .track(track_id)
                .is_some_and(|t| t.instrument.is_audio());
            let project = &self.project;
            self.armed.retain(|id| {
                project
                    .track(id)
                    .is_some_and(|t| t.instrument.is_audio() != audio)
            });
            self.armed.insert(track_id.to_string());
            self.input_error = None;
        }
    }

    /// The first armed track of each kind: (audio, instrument).
    fn armed_tracks(&self) -> (Option<String>, Option<String>) {
        let mut audio = None;
        let mut midi = None;
        for t in &self.project.tracks {
            if !self.armed.contains(&t.id) {
                continue;
            }
            if t.instrument.is_audio() {
                audio.get_or_insert_with(|| t.id.clone());
            } else {
                midi.get_or_insert_with(|| t.id.clone());
            }
        }
        (audio, midi)
    }

    pub fn midi_connected(&self) -> bool {
        self.engine
            .as_ref()
            .is_some_and(|e| !e.midi_ports().is_empty())
    }

    /// Nothing armed: arm the selected audio track, or the selected
    /// instrument track when a MIDI keyboard is connected, or make a new
    /// audio track for the recording.
    fn auto_arm(&mut self) {
        let project = &self.project;
        self.armed.retain(|id| project.track(id).is_some());
        if !self.armed.is_empty() {
            return;
        }
        let selected = self
            .selected_track
            .as_deref()
            .and_then(|id| self.project.track(id))
            .map(|t| (t.id.clone(), t.instrument.is_audio()));
        match selected {
            Some((id, true)) => {
                self.armed.insert(id);
            }
            Some((id, false)) if self.midi_connected() => {
                self.armed.insert(id);
            }
            _ => {
                if let Some(id) = self.add_audio_track(tr("Recording")) {
                    self.armed.insert(id);
                }
            }
        }
    }

    /// Add an audio track (for recordings and WAV files) and select it.
    pub fn add_audio_track(&mut self, base: &str) -> Option<String> {
        let same = self
            .project
            .tracks
            .iter()
            .filter(|t| t.name.starts_with(base))
            .count();
        let name = if same == 0 {
            base.to_string()
        } else {
            format!("{base} {}", same + 1)
        };
        let v = self.run_op(Op::AddTrack {
            name,
            instrument: Some(Instrument::Audio),
            gain: None,
            pan: None,
            reverb: None,
        })?;
        let id = v["track_id"].as_str().map(String::from)?;
        self.selected_track = Some(id.clone());
        self.selected_clip = None;
        self.selected_notes.clear();
        Some(id)
    }

    pub fn toggle_record(&mut self) {
        if self.is_recording() {
            self.stop_recording();
        } else {
            self.start_recording();
        }
    }

    pub fn start_recording(&mut self) {
        if self.pending_take.is_some() {
            self.status = tr("Still saving the last take, one moment…").into();
            return;
        }
        if self.engine.is_none() {
            self.status =
                tr("No sound device. Opening the Sound window so you can pick one.").into();
            self.show_sound = true;
            return;
        }
        if self.path.is_none() {
            self.status =
                tr("Save the song first: recordings are kept in a folder next to it").into();
            self.save_as_dialog();
            if self.path.is_none() {
                return;
            }
        }
        self.auto_arm();
        let (audio_track, midi_track) = self.armed_tracks();
        if let Some(id) = &audio_track {
            let Some(track) = self.project.track(id) else {
                return;
            };
            let (name, inputs) = (track.name.clone(), track.input.clone());
            let Some(path) = self.take_path(&name) else {
                self.status =
                    tr("Save the song first: recordings are kept in a folder next to it").into();
                return;
            };
            if let Err(e) = self.ensure_input() {
                self.status = trf("Couldn't open the input: {}", &[&e]);
                self.show_sound = true;
                return;
            }
            let engine = self.engine.as_mut().expect("checked above");
            if let Err(e) = engine.start_take(&path, &inputs) {
                self.status = trf("Couldn't start recording: {}", &[&format!("{e:#}")]);
                return;
            }
        }
        let bar = self.project.beats_per_bar.max(1) as f64;
        let count_in = self.settings.count_in;
        self.send_project();
        let engine = self.engine.as_mut().expect("checked above");
        engine.set_recording(true);
        if !engine.is_playing() {
            if count_in {
                engine.play_after_count_in(bar);
            } else {
                engine.play();
            }
        }
        self.recording = Some(Recording::new(
            self.playhead.max(0.0),
            audio_track,
            midi_track,
        ));
        self.status = if count_in {
            tr("Recording after one bar of count-in… press R or Space to stop").into()
        } else {
            tr("Recording… press R or Space to stop").into()
        };
    }

    pub fn stop_recording(&mut self) {
        let Some(mut rec) = self.recording.take() else {
            return;
        };
        let stop = self.engine.as_ref().map_or(self.playhead, |e| e.position());
        if let Some(e) = &mut self.engine {
            e.pause();
            e.set_recording(false);
            e.stop_take();
        }
        if let Some(track_id) = rec.audio_track.take() {
            self.pending_take = Some(PendingTake {
                track_id,
                punch_in: rec.punch_in,
            });
            self.status = tr("Saving the take…").into();
        }
        if let Some(track_id) = rec.midi_track.clone() {
            let notes = rec.finish(stop);
            let bar = self.project.beats_per_bar.max(1) as f64;
            match midi_clip(&notes, rec.punch_in, stop, bar) {
                None => {
                    if self.pending_take.is_none() {
                        self.status =
                            tr("Nothing was played on the MIDI keyboard, so no clip was made")
                                .into();
                    }
                }
                Some((start, length, notes)) => {
                    let n = notes.len();
                    let takes = self.project.track(&track_id).map_or(0, |t| t.clips.len());
                    let op = Op::AddClip {
                        track_id: track_id.clone(),
                        start,
                        length,
                        name: Some(trf("Take {}", &[&(takes + 1)])),
                        notes,
                    };
                    if let Some(v) = self.run_op(op) {
                        self.selected_track = Some(track_id);
                        self.selected_clip = v["clip_id"].as_str().map(String::from);
                        self.selected_notes.clear();
                        self.roll_scroll_to_notes = true;
                        self.status = trf(
                            "Recorded {} from the MIDI keyboard",
                            &[&crate::app::notes_text(n)],
                        );
                    }
                }
            }
        }
    }

    /// Open the input device chosen in the Sound window, if it isn't open.
    fn ensure_input(&mut self) -> Result<(), String> {
        let wanted = self.settings.input.clone();
        let Some(engine) = &mut self.engine else {
            return Err(tr("no sound device").into());
        };
        if engine.input().is_some() {
            return Ok(());
        }
        let opened = engine
            .open_input(wanted.as_deref())
            .or_else(|e| match wanted {
                // The saved microphone is gone: try the default one.
                Some(_) => engine.open_input(None),
                None => Err(e),
            })
            .map_err(|e| format!("{e:#}"));
        self.input_error = opened.as_ref().err().cloned();
        match (&opened, engine.input()) {
            (Ok(()), Some(input)) => crate::sound::log(&format!(
                "input: opened {:?} ({})",
                input.name(),
                input.format()
            )),
            (Err(e), _) => crate::sound::log(&format!("input: {wanted:?} FAILED: {e}")),
            _ => {}
        }
        opened
    }

    /// Reopen the input (another device was picked in the Sound window).
    pub fn reopen_input(&mut self) {
        if let Some(e) = &mut self.engine {
            e.close_input();
        }
        self.monitoring = None;
        self.input_error = None;
    }

    /// Everything recording needs once per frame: MIDI notes, the take
    /// being saved, meters, monitoring, the live track and the sound files.
    pub fn record_frame(&mut self) {
        let events = self
            .engine
            .as_ref()
            .map(|e| e.midi_events())
            .unwrap_or_default();
        if !events.is_empty() {
            self.midi_seen = Some(std::time::Instant::now());
        }
        if let Some(rec) = &mut self.recording {
            if rec.midi_track.is_some() {
                for ev in events {
                    rec.midi(ev);
                }
            }
        }
        if self.recording.is_some() && self.engine.as_ref().is_some_and(|e| e.take_failed()) {
            self.stop_recording();
            self.status =
                tr("Recording stopped: the take couldn't be written (is the disk full?)").into();
        }
        self.collect_take();

        // The input is open while an audio track is armed (for its meter and
        // monitoring), while recording, and while the Sound window shows it.
        let (armed_audio, armed_midi) = self.armed_tracks();
        let want_input = armed_audio.is_some()
            || self.pending_take.is_some()
            || self
                .recording
                .as_ref()
                .is_some_and(|r| r.audio_track.is_some())
            || self.show_sound;
        let has_input = self.engine.as_ref().is_some_and(|e| e.input().is_some());
        if want_input && !has_input && self.input_error.is_none() && self.engine.is_some() {
            if let Err(e) = self.ensure_input() {
                self.status = trf("Couldn't open the input: {}", &[&e]);
            }
        } else if !want_input && has_input {
            if let Some(e) = &mut self.engine {
                e.close_input();
            }
            self.monitoring = None;
        }
        // Meters fall back slowly so short peaks stay visible.
        let peaks = self
            .engine
            .as_ref()
            .and_then(|e| e.input())
            .map(|i| i.take_peaks())
            .unwrap_or_default();
        self.input_peaks.resize(peaks.len(), 0.0);
        for (shown, new) in self.input_peaks.iter_mut().zip(peaks) {
            *shown = new.max(*shown * 0.85);
        }

        // Monitoring: the armed audio track's input, at its volume and pan.
        let monitor = armed_audio
            .as_deref()
            .filter(|_| self.settings.monitor)
            .and_then(|id| {
                let i = self.project.tracks.iter().position(|t| t.id == id)?;
                Some((i, self.project.tracks[i].input.clone()))
            })
            .filter(|_| self.engine.as_ref().is_some_and(|e| e.input().is_some()));
        if monitor != self.monitoring {
            if let Some(e) = &mut self.engine {
                let problem = e.set_monitor(monitor.as_ref().map(|(i, c)| (*i, c.as_slice())));
                if let Some(p) = problem {
                    self.status = trf("Couldn't monitor the input: {}", &[&p]);
                    self.settings.monitor = false;
                    self.settings.save();
                }
            }
            self.monitoring = monitor;
        }

        // MIDI keyboards play the armed instrument track, else the selected one.
        let live = armed_midi
            .or_else(|| self.selected_track.clone())
            .and_then(|id| self.project.tracks.iter().position(|t| t.id == id))
            .filter(|&i| !self.project.tracks[i].instrument.is_audio());
        if live != self.live_track {
            if let Some(e) = &self.engine {
                e.set_live_track(live);
            }
            self.live_track = live;
        }

        self.sync_audio();
    }

    /// Place the take being saved once its file is complete. Returns
    /// whether nothing is pending any more.
    fn collect_take(&mut self) -> bool {
        if self.pending_take.is_none() {
            return true;
        }
        if self.engine.is_none() {
            self.pending_take = None;
            return true;
        }
        let Some(done) = self.engine.as_mut().and_then(|e| e.finished_take()) else {
            return false;
        };
        let pending = self.pending_take.take().expect("checked above");
        match done {
            Ok(take) => self.place_take(take, pending),
            Err(e) => self.status = trf("Recording failed: {}", &[&format!("{e:#}")]),
        }
        true
    }

    /// Wait (a moment at most) for the take being saved and place it; used
    /// before the sound device is closed.
    pub fn finish_take_now(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while !self.collect_take() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        self.pending_take = None;
    }

    /// Put a finished take on its track.
    fn place_take(&mut self, take: Take, pending: PendingTake) {
        let spb = self.project.seconds_per_beat();
        let seconds = take.audio.seconds();
        let first = take.first_beat.unwrap_or(pending.punch_in);
        let (start, offset, length) = place_audio(first, pending.punch_in, seconds, spb);
        if length < 0.05 || self.project.track(&pending.track_id).is_none() {
            let _ = std::fs::remove_file(&take.path);
            self.status = tr("The recording was too short, so it was dropped").into();
            return;
        }
        let file = audio::file_name_for(self.folder(), &take.path);
        Arc::make_mut(&mut self.audio).insert(file.clone(), take.audio);
        if let Some(e) = &self.engine {
            e.set_audio(self.audio.clone());
        }
        let name = take
            .path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned());
        let op = Op::AddAudioClip {
            track_id: pending.track_id.clone(),
            start,
            length,
            file,
            offset,
            gain: None,
            name,
            replace: true,
        };
        if let Some(v) = self.run_op(op) {
            self.selected_track = Some(pending.track_id);
            self.selected_clip = v["clip_id"].as_str().map(String::from);
            self.selected_notes.clear();
            // The take is one undo step of its own, saved right away.
            self.settle();
            self.status = if take.dropped_frames > 0 {
                trf(
                    "Recorded {}s, but {} samples were lost because the computer was too busy",
                    &[&format!("{:.1}", seconds - offset), &take.dropped_frames],
                )
            } else {
                trf(
                    "Recorded {}s (Ctrl+Z removes the take)",
                    &[&format!("{:.1}", seconds - offset)],
                )
            };
        }
    }

    /// Load the sound files the song plays that aren't loaded yet, on a
    /// background thread, and hand them to the engine when they arrive.
    fn sync_audio(&mut self) {
        if let Some(load) = &self.audio_load {
            match load.rx.try_recv() {
                Ok((loaded, problems)) => {
                    self.audio_load = None;
                    if !loaded.is_empty() {
                        let pool = Arc::make_mut(&mut self.audio);
                        for (file, data) in loaded {
                            pool.insert(file, data);
                        }
                        if let Some(e) = &self.engine {
                            e.set_audio(self.audio.clone());
                        }
                    }
                    if let Some((file, why)) = problems.first() {
                        self.status = trf("Couldn't load the recording {}: {}", &[file, why]);
                    }
                    self.audio_failed
                        .extend(problems.into_iter().map(|(f, _)| f));
                }
                Err(mpsc::TryRecvError::Empty) => return,
                Err(mpsc::TryRecvError::Disconnected) => self.audio_load = None,
            }
        }
        let missing: Vec<String> = audio::files_used(&self.project)
            .into_iter()
            .filter(|f| !self.audio.contains(f) && !self.audio_failed.contains(*f))
            .map(String::from)
            .collect();
        if missing.is_empty() {
            return;
        }
        let base = self.folder().map(Path::to_path_buf);
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut loaded = Vec::new();
            let mut problems = Vec::new();
            for file in missing {
                match audio::load_wav(&audio::resolve(base.as_deref(), &file)) {
                    Ok(data) => loaded.push((file, Arc::new(data))),
                    Err(e) => problems.push((file, format!("{e:#}"))),
                }
            }
            let _ = tx.send((loaded, problems));
        });
        self.audio_load = Some(AudioLoad { rx });
    }

    /// Forget the loaded sound files (another song was opened).
    pub fn reset_audio(&mut self) {
        self.audio = Arc::new(AudioPool::new());
        self.audio_failed.clear();
        self.audio_load = None;
        if let Some(e) = &self.engine {
            e.set_audio(self.audio.clone());
        }
    }

    /// Try loading the files that failed before (after Save As moved the
    /// song, say).
    pub fn retry_audio(&mut self) {
        self.audio_failed.clear();
    }

    pub fn is_loading_audio(&self) -> bool {
        self.audio_load.is_some()
    }

    /// File › Import audio: put a WAV on the selected audio track (or a new
    /// one) at the playhead's bar. The file is copied next to the song when
    /// the song is saved, so the folder holds everything it plays.
    pub fn import_audio_dialog(&mut self) {
        let Some(src) = rfd::FileDialog::new()
            .add_filter(tr("WAV audio"), &["wav"])
            .pick_file()
        else {
            return;
        };
        let data = match audio::load_wav(&src) {
            Ok(d) => Arc::new(d),
            Err(e) => {
                self.status = trf(
                    "Couldn't load the recording {}: {}",
                    &[&src.display(), &format!("{e:#}")],
                );
                return;
            }
        };
        let path = match self.takes_folder() {
            Some(dir) => {
                let name = src
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "audio.wav".into());
                let stem = name
                    .strip_suffix(".wav")
                    .or_else(|| name.strip_suffix(".WAV"))
                    .unwrap_or(&name)
                    .to_string();
                let dest = (1..10_000)
                    .map(|n| {
                        if n == 1 {
                            dir.join(&name)
                        } else {
                            dir.join(format!("{stem} {n}.wav"))
                        }
                    })
                    .find(|p| !p.exists() || same_file(p, &src))
                    .unwrap_or_else(|| dir.join(&name));
                let copied = std::fs::create_dir_all(&dir).and_then(|()| {
                    if same_file(&dest, &src) {
                        Ok(0)
                    } else {
                        std::fs::copy(&src, &dest)
                    }
                });
                match copied {
                    Ok(_) => dest,
                    Err(e) => {
                        self.status = trf("Couldn't copy the file next to the song: {}", &[&e]);
                        return;
                    }
                }
            }
            None => src.clone(),
        };
        let track_id = match self
            .selected_track
            .as_deref()
            .and_then(|id| self.project.track(id))
            .filter(|t| t.instrument.is_audio())
        {
            Some(t) => t.id.clone(),
            None => match self.add_audio_track(tr("Audio track")) {
                Some(id) => id,
                None => return,
            },
        };
        let bar = self.project.beats_per_bar.max(1) as f64;
        let wanted = (self.playhead.max(0.0) / bar).floor() * bar;
        let length = data.seconds() / self.project.seconds_per_beat();
        let start = self.free_spot(&track_id, wanted, length);
        let file = audio::file_name_for(self.folder(), &path);
        Arc::make_mut(&mut self.audio).insert(file.clone(), data);
        if let Some(e) = &self.engine {
            e.set_audio(self.audio.clone());
        }
        let name = path.file_stem().map(|s| s.to_string_lossy().into_owned());
        let op = Op::AddAudioClip {
            track_id: track_id.clone(),
            start,
            length,
            file,
            offset: 0.0,
            gain: None,
            name,
            replace: false,
        };
        if let Some(v) = self.run_op(op) {
            self.selected_track = Some(track_id);
            self.selected_clip = v["clip_id"].as_str().map(String::from);
            self.status = if self.path.is_some() {
                tr("Imported the audio file (a copy is kept next to the song)").into()
            } else {
                tr("Imported the audio file. Save the song to keep a copy next to it.").into()
            };
        }
    }

    /// Add a copy of `clip` (notes or a recording) to a track; used by
    /// paste and duplicate.
    pub fn add_clip_copy(
        &mut self,
        track_id: &str,
        start: f64,
        clip: Clip,
    ) -> Option<serde_json::Value> {
        match clip.audio {
            Some(a) => {
                // Copied recordings carry their full path (see copy_clip).
                let file = audio::file_name_for(self.folder(), Path::new(&a.file));
                let v = self.run_op(Op::AddAudioClip {
                    track_id: track_id.into(),
                    start,
                    length: clip.length,
                    file,
                    offset: a.offset,
                    gain: Some(a.gain),
                    name: Some(clip.name),
                    replace: false,
                })?;
                if a.fade_in > 0.0 || a.fade_out > 0.0 {
                    let id = v["clip_id"].as_str().unwrap_or_default().to_string();
                    self.run_op(Op::UpdateAudio {
                        clip_id: id,
                        gain: None,
                        offset: None,
                        fade_in: Some(a.fade_in),
                        fade_out: Some(a.fade_out),
                    });
                }
                Some(v)
            }
            None => self.run_op(Op::AddClip {
                track_id: track_id.into(),
                start,
                length: clip.length,
                name: Some(clip.name),
                notes: clip.notes,
            }),
        }
    }
}

impl App {
    /// After Save As: copy the recordings the song plays from `old_folder`
    /// to the new song's folder, so that folder holds everything the song
    /// needs and its relative file names keep working. Files the clips name
    /// by a full path (imported before the song was saved) are copied into
    /// the song's audio folder and the clips renamed to the copies.
    pub fn bring_audio_along(&mut self, old_folder: Option<&Path>) {
        self.retry_audio();
        let (Some(new_folder), Some(takes)) =
            (self.folder().map(Path::to_path_buf), self.takes_folder())
        else {
            return;
        };
        let files: Vec<String> = audio::files_used(&self.project)
            .into_iter()
            .map(String::from)
            .collect();
        let mut renamed: Vec<(String, String)> = Vec::new();
        let mut problem = None;
        for file in files {
            let src = audio::resolve(old_folder, &file);
            let wanted = if Path::new(&file).is_absolute() {
                match src.file_name() {
                    Some(name) => takes.join(name),
                    None => continue,
                }
            } else {
                audio::resolve(Some(&new_folder), &file)
            };
            if same_file(&src, &wanted) {
                continue;
            }
            // Never overwrite another song's recording of the same name.
            let dest = free_name(&wanted, |p| !p.exists());
            let copied = match dest.parent() {
                Some(dir) => std::fs::create_dir_all(dir).map_err(|e| e.to_string()),
                None => Ok(()),
            }
            .and_then(|()| {
                if src.exists() {
                    std::fs::copy(&src, &dest)
                        .map(|_| ())
                        .map_err(|e| e.to_string())
                } else if let Some(data) = self.audio.get(&file) {
                    // The original is gone, but the sound is still in memory.
                    let channels = data.channels() as u16;
                    audio::write_wav_24(&dest, data.sample_rate(), channels, data.samples())
                        .map_err(|e| format!("{e:#}"))
                } else {
                    Err(tr("the file is missing").to_string())
                }
            });
            match copied {
                Ok(()) => {
                    let name = audio::file_name_for(Some(&new_folder), &dest);
                    if name != file {
                        renamed.push((file, name));
                    }
                }
                Err(e) => {
                    problem.get_or_insert((file, e));
                }
            }
        }
        if !renamed.is_empty() {
            let pool = Arc::make_mut(&mut self.audio);
            for (old, new) in &renamed {
                if let Some(data) = pool.get(old).cloned() {
                    pool.insert(new.clone(), data);
                }
                for clip in self
                    .project
                    .tracks
                    .iter_mut()
                    .flat_map(|t| t.clips.iter_mut())
                {
                    if let Some(a) = clip.audio.as_mut().filter(|a| a.file == *old) {
                        a.file = new.clone();
                    }
                }
            }
            if let Some(e) = &self.engine {
                e.set_audio(self.audio.clone());
            }
            self.send_project();
        }
        if let Some((file, why)) = problem {
            self.status = trf(
                "Couldn't copy the recording {} next to the song: {}",
                &[&file, &why],
            );
        }
    }
}

/// `wanted`, or "name 2.wav", "name 3.wav"… beside it: the first that `free` accepts.
fn free_name(wanted: &Path, free: impl Fn(&Path) -> bool) -> PathBuf {
    if free(wanted) {
        return wanted.to_path_buf();
    }
    let stem = wanted
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "audio".into());
    let ext = wanted
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    (2..10_000)
        .map(|n| wanted.with_file_name(format!("{stem} {n}{ext}")))
        .find(|p| free(p))
        .unwrap_or_else(|| wanted.to_path_buf())
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn on(pitch: u8, beat: f64) -> MidiEvent {
        MidiEvent::NoteOn {
            pitch,
            velocity: 100,
            beat: Some(beat),
        }
    }

    fn off(pitch: u8, beat: f64) -> MidiEvent {
        MidiEvent::NoteOff {
            pitch,
            beat: Some(beat),
        }
    }

    #[test]
    fn played_notes_are_recorded_with_the_pedal() {
        let mut r = Recording::new(4.0, None, Some("t1".into()));
        r.midi(on(60, 4.0));
        r.midi(off(60, 5.0));
        r.midi(MidiEvent::Pedal {
            down: true,
            beat: Some(5.5),
        });
        r.midi(on(64, 6.0));
        r.midi(off(64, 6.5));
        // Notes played while the song stood still are not recorded.
        r.midi(MidiEvent::NoteOn {
            pitch: 30,
            velocity: 1,
            beat: None,
        });
        r.midi(MidiEvent::Pedal {
            down: false,
            beat: Some(8.0),
        });
        r.midi(on(67, 9.0));
        let notes = r.finish(10.0);
        assert_eq!(
            notes,
            [
                (60, 100, 4.0, 5.0),
                (64, 100, 6.0, 8.0),
                (67, 100, 9.0, 10.0)
            ]
        );
    }

    #[test]
    fn a_midi_take_fills_whole_bars_from_the_punch_in() {
        let notes = [(60, 90, 4.2, 5.0), (62, 80, 7.9, 9.5)];
        let (start, length, n) = midi_clip(&notes, 4.0, 9.0, 4.0).unwrap();
        assert_eq!((start, length), (4.0, 8.0));
        assert!((n[0].start - 0.2).abs() < 1e-9 && (n[0].duration - 0.8).abs() < 1e-9);
        // A note played just before the punch-in (during the count-in).
        let early = [(60, 90, 3.9, 4.5)];
        let (start, length, n) = midi_clip(&early, 4.0, 6.0, 4.0).unwrap();
        assert_eq!((start, length, n[0].start), (0.0, 8.0, 3.9));
        assert!(midi_clip(&[], 4.0, 8.0, 4.0).is_none());
    }

    #[test]
    fn a_take_starts_at_the_punch_in_and_trims_the_count_in() {
        // The take began during the count-in, a bar before beat 8.
        let (start, offset, length) = place_audio(4.0, 8.0, 10.0, 0.5);
        assert_eq!((start, offset, length), (8.0, 2.0, 16.0));
        // Recording from the very start: the count-in lies before the song.
        let (start, offset, _) = place_audio(-4.0, 0.0, 10.0, 0.5);
        assert_eq!((start, offset), (0.0, 2.0));
        // Audio that began after the punch-in starts where it began.
        let (start, offset, _) = place_audio(8.25, 8.0, 10.0, 0.5);
        assert_eq!((start, offset), (8.25, 0.0));
    }

    #[test]
    fn take_names_are_safe_file_names() {
        assert_eq!(file_safe("Vocals"), "Vocals");
        assert_eq!(file_safe("Gtr: L/R?"), "Gtr_ L_R_");
        assert_eq!(file_safe("  ..."), "Take");
    }
}
