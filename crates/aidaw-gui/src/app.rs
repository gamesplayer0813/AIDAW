//! Application state, file sync with the AI, undo/redo and the top-level layout.

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant, SystemTime};

use aidaw_core::edit::{self, Op};
use aidaw_core::{io as pio, render, Cue, LoopRegion, Note, Project};
use aidaw_engine::Engine;
use eframe::egui::{self, Color32, Key, KeyboardShortcut, Modifiers, Rect, RichText};

use crate::editing::{self, Copied};
use crate::history::{History, Selection, Version};
use crate::i18n::{self, tr, trf};
use crate::icons::{self, Icon};
use crate::lanes::LaneKey;
use crate::onboarding::{self, Tutorial, WelcomeChoice};
use crate::scale::Scale;
use crate::settings::Settings;
use crate::theme;
use aidaw_core::presets;

/// Status when the project file on disk was written by a newer AIDAW.
fn newer_file() -> &'static str {
    tr("A newer AIDAW saved this song, so this version won't overwrite it. Update AIDAW, or use Save As to keep your changes in another file.")
}

const SAVE_DEBOUNCE: Duration = Duration::from_millis(300);
const POLL_INTERVAL: Duration = Duration::from_millis(400);
/// How often a frame without input (playback) still looks for edits.
const CHECK_INTERVAL: Duration = Duration::from_millis(250);

pub use crate::theme::track_color;

/// In-progress mouse drags, shared by the arrangement and the piano roll.
#[derive(Clone, Debug)]
pub enum Drag {
    MoveClip {
        clip_id: String,
        grab_offset: f64,
        /// Alt+drag: `clip_id` is a fresh copy of this clip.
        copy_of: Option<String>,
    },
    ResizeClip {
        clip_id: String,
    },
    /// Moving the selected notes by the one grabbed (`anchor`). `from` holds
    /// each note's index, start and pitch when the drag began, so every
    /// frame works out the move from there.
    MoveNotes {
        anchor: usize,
        grab_offset: f64,
        from: Vec<(usize, f64, u8)>,
    },
    /// Lengthening the selected notes by the one grabbed: (index, length).
    ResizeNotes {
        anchor: usize,
        from: Vec<(usize, f64)>,
    },
    /// Drawing a box from this beat and key row to select notes, on top of
    /// the notes in `keep`.
    SelectNotes {
        beat: f64,
        row: f32,
        keep: BTreeSet<usize>,
    },
    /// Dragging a track's header to another row.
    MoveTrack {
        track_id: String,
    },
    /// Drawing a new loop region from `anchor` (beats) in the loop strip.
    LoopNew {
        anchor: f64,
    },
    /// Moving the loop region, grabbed `grab` beats after its start.
    LoopMove {
        grab: f64,
    },
    /// Dragging one end of the loop region (its start, or else its end).
    LoopEdge {
        start: bool,
    },
    /// Moving point `index` of an automation lane.
    MovePoint {
        key: LaneKey,
        index: usize,
    },
}

/// Where the editing keys (Delete, Ctrl+C, Ctrl+D, arrows…) act: the notes
/// in the piano roll, the clip in the song or an automation point,
/// whichever was clicked last.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditFocus {
    Notes,
    Clips,
    Automation,
}

pub struct App {
    pub project: Project,
    pub path: Option<PathBuf>,
    pub engine: Option<Engine>,
    pub engine_error: Option<String>,
    pub show_sound: bool,
    pub device_names: Vec<String>,
    /// When Play was last pressed, to notice a device that never starts.
    play_pressed_at: Option<Instant>,

    history: History,
    /// When the song was last looked at for edits (see `commit_edits`).
    last_check: Instant,
    /// Undo (false) / redo (true) requested from a menu; applied at the start of the next frame.
    queued_history: Option<bool>,

    pub selected_track: Option<String>,
    pub selected_clip: Option<String>,
    /// Indexes of the selected notes in the selected clip.
    pub selected_notes: BTreeSet<usize>,
    pub edit_focus: EditFocus,
    pub drag: Option<Drag>,
    /// Tracks whose automation lanes show under them, and Master's.
    pub lanes_open: BTreeSet<String>,
    pub master_lanes_open: bool,
    /// The automation point Delete acts on.
    pub selected_point: Option<(LaneKey, usize)>,

    /// Grid size in beats.
    pub snap: f64,
    pub arrange_zoom: f32,
    pub roll_zoom: f32,
    pub roll_key_height: f32,
    pub roll_scroll_to_notes: bool,
    /// Scroll the selected notes into view (after a keyboard edit moved them).
    pub roll_reveal: bool,
    /// The piano roll's scroll offset and visible size, last frame.
    pub roll_view: Option<(egui::Vec2, egui::Vec2)>,
    /// Changes on every Humanize click, so each one varies the notes anew.
    humanize_seed: u64,
    pub last_note_length: f64,
    pub playhead: f64,
    /// Scroll the song view so the playhead shows (the AI moved it).
    pub reveal_playhead: bool,
    /// Id of the AI's last playhead request carried out (see [`Cue`]).
    seen_cue: Option<u64>,

    disk_mtime: Option<SystemTime>,
    /// Modification time of the project file when a newer AIDAW wrote it.
    /// That version is never overwritten: saving would drop whatever this
    /// build does not understand.
    newer_on_disk: Option<SystemTime>,
    last_poll: Instant,
    save_at: Option<Instant>,
    pub status: String,
    /// A WAV export running on a background thread.
    export: Option<ExportJob>,
    /// Read when a name needs characters the bundled fonts lack.
    system_font: theme::SystemFont,
    /// The song changed, so its names are looked at for such characters.
    check_names: bool,

    // ---- beginner help ----
    pub settings: Settings,
    show_welcome: bool,
    pub show_ai_help: bool,
    show_shortcuts: bool,
    tutorial: Option<Tutorial>,
    /// Screen areas of key widgets, recorded each frame so the tutorial can point at them.
    pub ui_rects: HashMap<&'static str, Rect>,
    /// Widths of the top bar's menu, transport and right-hand groups last frame.
    top_bar_widths: [f32; 3],
    /// Share of the height the piano roll takes, and the height it was taken of.
    roll_share: f32,
    roll_room_h: f32,
    /// Event counters the tutorial watches.
    pub ev_note_drags: u32,
    pub ev_undos: u32,
    /// Pitch currently sounding because a piano key (or dragged note) is held.
    pub held_key: Option<u8>,

    // ---- key / scale helper ----
    pub scale_root: u8,
    pub scale: Scale,
    pub scale_lock: bool,
}

/// A blank song that is ready to draw in: a piano track and a drum track,
/// each with one empty 4-bar clip.
fn starter_project() -> Project {
    let mut p = Project::new(tr("My Song"), 110.0);
    let keys = presets::find("soft_keys").expect("preset");
    let drums = presets::find("drum_kit").expect("preset");
    let ops = vec![
        Op::AddTrack {
            name: tr("Piano").into(),
            instrument: Some(keys.instrument.clone()),
            gain: None,
            pan: None,
            reverb: Some(keys.category.reverb()),
        },
        Op::AddTrack {
            name: tr("Drums").into(),
            instrument: Some(drums.instrument.clone()),
            gain: None,
            pan: None,
            reverb: Some(drums.category.reverb()),
        },
        Op::AddClip {
            track_id: "t1".into(),
            start: 0.0,
            length: 16.0,
            name: Some(tr("Melody").into()),
            notes: Vec::new(),
        },
        Op::AddClip {
            track_id: "t2".into(),
            start: 0.0,
            length: 16.0,
            name: Some(tr("Beat").into()),
            notes: Vec::new(),
        },
    ];
    edit::apply_all(&mut p, ops).expect("static ops apply");
    p
}

/// Every name in the song: its own, its tracks' and its clips'.
fn song_names(p: &Project) -> impl Iterator<Item = &str> {
    let tracks = p.tracks.iter().flat_map(|t| {
        std::iter::once(t.name.as_str()).chain(t.clips.iter().map(|c| c.name.as_str()))
    });
    std::iter::once(p.name.as_str()).chain(tracks)
}

fn selection_of(clip: &Option<String>, notes: &BTreeSet<usize>) -> Selection {
    clip.clone().map(|clip| (clip, notes.clone()))
}

/// The notes of clip `clip_id` (none if the song has no such clip).
pub fn clip_notes<'a>(project: &'a Project, clip_id: &str) -> &'a [Note] {
    project.clip(clip_id).map_or(&[], |(_, c)| &c.notes)
}

/// The song to start with: the file asked for when it opens (or a new song
/// that will be saved there when there is no such file yet), else the
/// starter song with no file, so a file that failed to open is never
/// autosaved over.
fn first_song(path: Option<PathBuf>) -> (Project, Option<PathBuf>, String) {
    match path {
        Some(p) if p.exists() => match pio::load(&p) {
            Ok(project) => {
                let status = trf("Opened {}", &[&p.display()]);
                (project, Some(p), status)
            }
            Err(e) => {
                let status = trf("Failed to open: {}", &[&format!("{e:#}")]);
                (starter_project(), None, status)
            }
        },
        Some(p) => {
            let status = trf("New project, will save to {}", &[&p.display()]);
            (starter_project(), Some(p), status)
        }
        None => (
            starter_project(),
            None,
            tr("Welcome! Not sure where to start? Open ❓ Help › Tutorial.").into(),
        ),
    }
}

fn mtime(path: &std::path::Path) -> Option<SystemTime> {
    pio::modified(path)
}

/// A WAV export in progress on a background thread.
struct ExportJob {
    path: PathBuf,
    progress: Arc<AtomicU32>,
    result: mpsc::Receiver<Result<f64, String>>,
}

impl App {
    pub fn new(path: Option<PathBuf>) -> Self {
        let settings = Settings::load();
        i18n::set(settings.lang.unwrap_or_else(i18n::system));
        let (project, path, status) = first_song(path);
        let opened = Engine::new(&project).map_err(|e| format!("{e:#}"));
        crate::sound::log_engine(&opened);
        App::with(project, path, settings, opened, status)
    }

    /// The app around a song, with whatever sound device could be opened.
    fn with(
        project: Project,
        path: Option<PathBuf>,
        settings: Settings,
        opened: Result<Engine, String>,
        status: String,
    ) -> Self {
        let show_welcome = path.is_none() && settings.show_welcome;
        let (engine, engine_error) = match opened {
            Ok(e) => (Some(e), None),
            Err(e) => (None, Some(e)),
        };
        let mut app = App {
            selected_track: project.tracks.first().map(|t| t.id.clone()),
            history: History::new(&project),
            last_check: Instant::now(),
            project,
            disk_mtime: path.as_deref().and_then(mtime),
            newer_on_disk: None,
            save_at: path
                .as_ref()
                .filter(|p| !p.exists())
                .map(|_| Instant::now()),
            path,
            engine,
            engine_error,
            show_sound: false,
            device_names: Vec::new(),
            play_pressed_at: None,
            queued_history: None,
            selected_clip: None,
            selected_notes: BTreeSet::new(),
            edit_focus: EditFocus::Notes,
            drag: None,
            lanes_open: BTreeSet::new(),
            master_lanes_open: false,
            selected_point: None,
            snap: 0.25,
            arrange_zoom: 28.0,
            roll_zoom: 90.0,
            roll_key_height: 14.0,
            roll_scroll_to_notes: true,
            roll_reveal: false,
            roll_view: None,
            humanize_seed: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos() as u64),
            last_note_length: 1.0,
            playhead: 0.0,
            reveal_playhead: false,
            seen_cue: None,
            last_poll: Instant::now(),
            status,
            export: None,
            system_font: theme::SystemFont::default(),
            check_names: true,
            settings,
            show_welcome,
            show_ai_help: false,
            show_shortcuts: false,
            tutorial: None,
            ui_rects: HashMap::new(),
            top_bar_widths: [0.0; 3],
            roll_share: 0.5,
            roll_room_h: 0.0,
            ev_note_drags: 0,
            ev_undos: 0,
            held_key: None,
            scale_root: 0,
            scale: Scale::Major,
            scale_lock: true,
        };
        // A request already in the file is old news: don't act on it.
        app.seen_cue = app.project.transport.cue.map(|c| c.id);
        app.select_first_clip();
        app.reveal_automation(&Project::new("", 120.0));
        app
    }

    /// Open the lanes of tracks whose automation is new or changed since
    /// `before` (the AI just wrote some, or a song was opened), so nobody
    /// misses them.
    fn reveal_automation(&mut self, before: &Project) {
        let (tracks, song) = crate::arrange::changed_lanes(before, &self.project);
        self.lanes_open.extend(tracks);
        self.master_lanes_open |= song;
    }

    fn select_first_clip(&mut self) {
        if let Some((t, c)) = self
            .project
            .tracks
            .iter()
            .find_map(|t| t.clips.first().map(|c| (t, c)))
        {
            self.selected_track = Some(t.id.clone());
            self.selected_clip = Some(c.id.clone());
            self.roll_scroll_to_notes = true;
        }
    }

    /// Replace the current song. The old one stays on the undo stack unless
    /// the file changes too: undoing into another file's song would
    /// autosave that song over the file just opened.
    pub fn load_project(&mut self, project: Project, path: Option<PathBuf>, status: &str) {
        self.settle();
        let old = self.swap_in(Version::of(project));
        if path == self.path {
            self.history.push(old);
        } else {
            self.history.clear();
        }
        // A playhead request already in the file is old news.
        self.seen_cue = self.project.transport.cue.map(|c| c.id);
        self.disk_mtime = path.as_deref().and_then(mtime);
        self.newer_on_disk = None;
        self.path = path;
        self.selected_clip = None;
        self.selected_notes.clear();
        self.edit_focus = EditFocus::Notes;
        self.stop();
        self.save_at = None;
        self.select_first_clip();
        self.lanes_open.clear();
        self.master_lanes_open = false;
        self.reveal_automation(&Project::new("", 120.0));
        self.status = status.into();
    }

    /// Hear one note on a track's sound, even while stopped.
    pub fn preview_note(&self, track_index: usize, pitch: u8) {
        if let Some(e) = &self.engine {
            e.set_project(&self.project);
            e.preview(track_index, pitch);
        }
    }

    /// Hear a few notes together, e.g. a chord that was just moved.
    pub fn preview_notes(&self, track_index: usize, pitches: &[u8]) {
        if let Some(e) = &self.engine {
            e.set_project(&self.project);
            for &p in pitches {
                e.preview(track_index, p);
            }
        }
    }

    /// Start sounding a note until [`Self::release_note`] (holding a piano key).
    pub fn hold_note(&mut self, track_index: usize, pitch: u8) {
        if let Some(e) = &self.engine {
            if self.held_key.is_none() {
                e.set_project(&self.project);
            }
            e.hold(track_index, pitch);
        }
        self.held_key = Some(pitch);
    }

    pub fn release_note(&mut self) {
        if let Some(e) = &self.engine {
            e.release();
        }
        self.held_key = None;
    }

    /// Whether the key helper applies to this track (it never applies to drums).
    pub fn scale_active(&self, track_index: usize) -> bool {
        self.scale != Scale::Off
            && self
                .project
                .tracks
                .get(track_index)
                .is_some_and(|t| !t.instrument.is_drums())
    }

    /// The key notes on this track stay in: the chosen one when "Stay in
    /// key" applies (always in Easy mode), otherwise every key (`Off`).
    pub fn key_for(&self, track_index: usize) -> Scale {
        if (self.scale_lock || !self.pro()) && self.scale_active(track_index) {
            self.scale
        } else {
            Scale::Off
        }
    }

    pub fn fit_pitch(&self, track_index: usize, pitch: u8) -> u8 {
        self.key_for(track_index).snap(self.scale_root, pitch)
    }

    // ---- transport -------------------------------------------------------

    /// Pro mode shows every control; Easy mode (the default) only the essentials.
    pub fn pro(&self) -> bool {
        self.settings.pro
    }

    pub fn is_playing(&self) -> bool {
        self.engine.as_ref().is_some_and(|e| e.is_playing())
    }

    pub fn toggle_play(&mut self) {
        match &self.engine {
            Some(e) => {
                if e.is_playing() {
                    e.pause();
                } else {
                    e.play();
                    self.play_pressed_at = Some(Instant::now());
                }
            }
            None => {
                self.status =
                    tr("No sound device. Opening the Sound window so you can pick one.").into();
                self.show_sound = true;
            }
        }
    }

    /// (Re)open audio output, on the default device or a named one.
    pub fn open_audio(&mut self, device: Option<&str>) {
        self.engine = None; // release the old device first
        let opened = match device {
            Some(name) => Engine::with_device(&self.project, name),
            None => Engine::new(&self.project),
        }
        .map_err(|e| format!("{e:#}"));
        crate::sound::log_engine(&opened);
        match opened {
            Ok(e) => {
                self.status = trf("Sound now plays through {}", &[&e.device_name()]);
                e.test_tone();
                self.engine = Some(e);
                self.engine_error = None;
            }
            Err(err) => {
                self.status = tr("Couldn't open that sound device").into();
                self.engine_error = Some(err);
            }
        }
    }

    /// If Play was pressed but the device never asked for audio, say so.
    fn check_audio_alive(&mut self) {
        let Some(t) = self.play_pressed_at else {
            return;
        };
        if t.elapsed() < Duration::from_millis(1500) {
            return;
        }
        self.play_pressed_at = None;
        if let Some(e) = &self.engine {
            let d = e.diagnostics();
            crate::sound::log(&format!(
                "play: device={:?} callbacks={} peak={:.3} position={:.2} error={:?}",
                e.device_name(),
                d.callbacks,
                d.peak,
                e.position(),
                d.error
            ));
            if d.callbacks == 0 || d.error.is_some() {
                self.status =
                    tr("The sound device isn't running. Pick another one in the Sound window.")
                        .into();
                self.show_sound = true;
            }
        }
    }

    /// Stop, back at the start of the song (or of the loop, while it is on).
    pub fn stop(&mut self) {
        let home = self.project.active_loop().map_or(0.0, |(start, _)| start);
        if let Some(e) = &self.engine {
            e.pause();
            e.seek(home);
        }
        self.playhead = home;
    }

    pub fn seek(&mut self, beat: f64) {
        if let Some(e) = &self.engine {
            e.seek(beat);
        }
        self.playhead = beat.max(0.0);
    }

    // ---- editing helpers -------------------------------------------------

    /// The grid in beats. Easy mode hides the Grid control and uses 1/16.
    pub fn grid(&self) -> f64 {
        if self.pro() {
            self.snap
        } else {
            0.25
        }
    }

    pub fn snap_beat(&self, beat: f64) -> f64 {
        (beat / self.grid()).round() * self.grid()
    }

    pub fn snap_floor(&self, beat: f64) -> f64 {
        (beat / self.grid()).floor() * self.grid()
    }

    /// Run an [`Op`] from the UI, reporting errors in the status bar.
    pub fn run_op(&mut self, op: Op) -> Option<serde_json::Value> {
        match edit::apply(&mut self.project, op) {
            Ok(v) => Some(v),
            Err(e) => {
                self.status = trf("Edit failed: {}", &[&format!("{e:#}")]);
                None
            }
        }
    }

    fn on_project_changed(&mut self) {
        self.check_names = true;
        if let Some(e) = &self.engine {
            e.set_project(&self.project);
        }
        if self.path.is_some() {
            self.save_at = Some(Instant::now() + SAVE_DEBOUNCE);
        }
        self.fix_selection();
    }

    fn fix_selection(&mut self) {
        if let Some(id) = &self.selected_track {
            if self.project.track(id).is_none() {
                self.selected_track = self.project.tracks.first().map(|t| t.id.clone());
            }
        }
        match self
            .selected_clip
            .as_deref()
            .and_then(|id| self.project.clip(id))
        {
            None => {
                self.selected_clip = None;
                self.selected_notes.clear();
            }
            Some((_, clip)) => {
                let len = clip.notes.len();
                self.selected_notes.retain(|&i| i < len);
            }
        }
        if let Some((key, i)) = &self.selected_point {
            if key.lane(&self.project).is_none_or(|l| *i >= l.points.len()) {
                self.selected_point = None;
            }
        }
        self.lanes_open
            .retain(|id| self.project.tracks.iter().any(|t| &t.id == id));
    }

    // ---- undo history --------------------------------------------------

    /// The open clip and its selected notes, to keep with a version.
    fn selection(&self) -> Selection {
        selection_of(&self.selected_clip, &self.selected_notes)
    }

    /// Look at what this frame changed in the song. Only clicks, keys and
    /// drags edit it, so frames without them (playback, the mouse merely
    /// passing over) look just every [`CHECK_INTERVAL`].
    fn commit_edits(&mut self, ctx: &egui::Context) {
        let (input, held) = ctx.input(|i| {
            let held = i.pointer.any_down();
            let moved = |e: &egui::Event| {
                matches!(e, egui::Event::PointerMoved(_) | egui::Event::MouseMoved(_))
            };
            let input =
                held || i.raw.events.iter().any(|e| !moved(e)) || !i.raw.dropped_files.is_empty();
            (input, held)
        });
        if input || self.last_check.elapsed() >= CHECK_INTERVAL {
            self.last_check = Instant::now();
            self.notice_edits();
        }
        if !held {
            self.drag = None; // no drag outlives the mouse button
            if self.history.in_gesture() {
                // Notes go back in playing order once the gesture is over
                // (a drag holds note indexes).
                self.sort_notes_keeping_selection();
                self.notice_edits();
                self.history.end_gesture(&self.project);
            }
        }
        self.history
            .select(self.selected_clip.as_deref(), &self.selected_notes);
    }

    /// Keep every clip's notes in playing order, the order the file and the
    /// AI use, so that an index means the same note everywhere. The
    /// selection follows its notes.
    fn sort_notes_keeping_selection(&mut self) {
        let open = self.selected_clip.as_deref();
        for clip in self.project.tracks.iter_mut().flat_map(|t| &mut t.clips) {
            if Some(clip.id.as_str()) == open && !clip.notes_sorted() {
                let moved = editing::sort_notes(&mut clip.notes);
                self.selected_notes = editing::moved_selection(&self.selected_notes, &moved);
            } else {
                clip.sort_notes();
            }
        }
    }

    /// Start an undo step if the song changed, and tell the engine and the
    /// autosave.
    fn notice_edits(&mut self) {
        let (clip, notes) = (&self.selected_clip, &self.selected_notes);
        if self
            .history
            .notice(&self.project, || selection_of(clip, notes))
        {
            self.on_project_changed();
        }
    }

    /// Make what was edited so far its own undo step, before undo, redo or
    /// a reload replaces the song.
    fn settle(&mut self) {
        self.notice_edits();
        self.history.end_gesture(&self.project);
    }

    /// Put `version` in place of the song (undo, redo, a reload) and return
    /// the version it replaced. Its selection comes back with it when it was
    /// stored for the open clip; otherwise the selected notes stay selected
    /// if the new version still has them.
    fn swap_in(&mut self, version: Version) -> Version {
        let Version { project, selection } = version;
        let old = Version {
            project: std::mem::replace(&mut self.project, project),
            selection: self.selection(),
        };
        if let Some(open) = &self.selected_clip {
            self.selected_notes = match selection {
                Some((clip, notes)) if clip == *open => notes,
                _ => editing::follow_notes(
                    clip_notes(&old.project, open),
                    clip_notes(&self.project, open),
                    &self.selected_notes,
                ),
            };
        }
        self.drag = None; // it held indexes into the old song

        // A selected automation point stays selected only while its lane
        // has as many points as before; otherwise its index names another.
        let points = |p: &Project| {
            self.selected_point
                .as_ref()
                .and_then(|(key, _)| key.lane(p))
                .map(|l| l.points.len())
        };
        if points(&old.project) != points(&self.project) {
            self.selected_point = None;
        }
        self.sort_notes_keeping_selection();
        self.on_project_changed();
        self.history.replaced(&self.project, self.selection());
        old
    }

    fn undo(&mut self) {
        self.settle();
        if let Some(mut prev) = self.history.take_undo() {
            // The AI's playhead requests are not edits: never bring an old one back.
            prev.project.transport.cue = self.project.transport.cue;
            let current = self.swap_in(prev);
            self.history.undone(current);
            self.ev_undos += 1;
            self.status = tr("Undone (Ctrl+Y to redo)").into();
        }
    }

    fn redo(&mut self) {
        self.settle();
        if let Some(mut next) = self.history.take_redo() {
            next.project.transport.cue = self.project.transport.cue;
            let current = self.swap_in(next);
            self.history.redone(current);
            self.status = tr("Redone").into();
        }
    }

    // ---- files -----------------------------------------------------------

    fn save_now(&mut self) {
        self.save_at = None;
        let Some(path) = self.path.clone() else {
            return;
        };
        // The AI may have written the file since we last read it (between two
        // polls). Never overwrite that silently: keep the human's version, but
        // put the AI's on the undo stack so nothing is lost.
        let on_disk = mtime(&path);
        if on_disk.is_some() && self.disk_mtime.is_some() && on_disk != self.disk_mtime {
            match pio::load(&path) {
                Ok(theirs) => {
                    let cue = theirs.transport.cue;
                    if !theirs.same_music(&self.project)
                        || theirs.transport.loop_region != self.project.transport.loop_region
                    {
                        self.history.push(Version::of(theirs));
                        self.status = tr(
                            "The AI changed the song while you were editing; kept your version (Ctrl+Z shows the AI's)",
                        )
                        .into();
                    }
                    // A playhead request is not an edit: keep it and carry it out.
                    self.project.transport.cue = cue;
                    self.history.not_an_edit(|p| p.transport.cue = cue);
                    self.follow_cue(cue);
                }
                Err(e) if pio::is_newer_format(&e) => self.newer_on_disk = on_disk,
                Err(_) => {}
            }
        }
        if on_disk.is_some() && on_disk == self.newer_on_disk {
            self.status = newer_file().into();
            return;
        }
        match pio::save(&self.project, &path) {
            Ok(()) => self.disk_mtime = mtime(&path),
            Err(e) => self.status = trf("Save failed: {}", &[&format!("{e:#}")]),
        }
    }

    /// Pick up changes the AI (or anything else) wrote to the project file.
    fn poll_disk(&mut self) {
        if self.last_poll.elapsed() < POLL_INTERVAL {
            return;
        }
        self.last_poll = Instant::now();
        let Some(path) = self.path.clone() else {
            return;
        };
        let Some(m) = mtime(&path) else { return };
        if Some(m) == self.disk_mtime || Some(m) == self.newer_on_disk {
            return;
        }
        let loaded = pio::load(&path);
        if let Err(e) = &loaded {
            if pio::is_newer_format(e) {
                self.newer_on_disk = Some(m);
                self.status = newer_file().into();
            }
        }
        // On a parse error (e.g. a non-atomic writer mid-write) just retry next poll.
        if let Ok(p) = loaded {
            self.disk_mtime = Some(m);
            let cue = p.transport.cue;
            let music = p.same_music(&self.project);
            if p != self.project
                && music
                && p.transport.loop_region == self.project.transport.loop_region
            {
                // Only a playhead request: nothing to undo.
                self.project.transport.cue = cue;
                self.history.not_an_edit(|p| p.transport.cue = cue);
            } else if p != self.project {
                // Keep the local version on the undo stack so AI edits can be reverted.
                self.settle();
                let local = self.swap_in(Version::of(p));
                self.reveal_automation(&local.project);
                self.history.push(local);
                // The file already holds this version; don't write it back.
                self.save_at = None;
                if self.selected_clip.is_none() {
                    self.select_first_clip();
                }
                self.status = if music {
                    // Only the loop changed.
                    self.loop_status()
                } else {
                    tr("Reloaded external change (Ctrl+Z to revert)").into()
                };
            }
            self.follow_cue(cue);
        }
    }

    /// Carry out a playhead request from the AI, once.
    fn follow_cue(&mut self, cue: Option<Cue>) {
        let Some(cue) = cue.filter(|c| self.seen_cue != Some(c.id)) else {
            return;
        };
        self.seen_cue = Some(cue.id);
        self.seek(cue.beat);
        self.reveal_playhead = true;
        let bar_of = |beat: f64| (beat / self.bar()).floor() as i64 + 1;
        let bar = bar_of(cue.beat);
        // Play never starts outside a loop that is on (see the engine).
        let loop_start = self
            .project
            .active_loop()
            .filter(|&(start, end)| cue.beat < start || cue.beat >= end)
            .map(|(start, _)| bar_of(start));
        let playing = self.is_playing();
        match cue.play {
            Some(true) if !playing => self.toggle_play(),
            Some(false) if playing => self.toggle_play(),
            _ => {}
        }
        self.status = match (cue.play, loop_start) {
            (Some(true), Some(start)) => {
                trf("The AI started playback at the loop, bar {}", &[&start])
            }
            (Some(true), None) => trf("The AI started playback at bar {}", &[&bar]),
            (Some(false), _) => trf("The AI stopped playback at bar {}", &[&bar]),
            (None, Some(start)) => trf(
                "The AI moved the playhead to bar {}, outside the loop: Play starts at bar {}",
                &[&bar, &start],
            ),
            (None, None) if playing => trf("The AI moved the playhead to bar {}", &[&bar]),
            (None, None) => trf(
                "The AI moved the playhead to bar {} (Space plays from there)",
                &[&bar],
            ),
        };
    }

    fn open_dialog(&mut self) {
        if let Some(p) = rfd::FileDialog::new()
            .add_filter(tr("AIDAW project"), &["json"])
            .pick_file()
        {
            match pio::load(&p) {
                Ok(proj) => {
                    let status = trf("Opened {}", &[&p.display()]);
                    self.load_project(proj, Some(p), &status);
                }
                Err(e) => self.status = trf("Open failed: {}", &[&format!("{e:#}")]),
            }
        }
    }

    pub fn save_as_dialog(&mut self) {
        let name = format!("{}.{}", self.project.name, pio::EXTENSION);
        if let Some(p) = rfd::FileDialog::new()
            .add_filter(tr("AIDAW project"), &["json"])
            .set_file_name(name)
            .save_file()
        {
            self.status = trf(
                "Saving to {} (AI tools can now use this file)",
                &[&p.display()],
            );
            self.path = Some(p);
            self.save_now();
        }
    }

    fn export_wav_dialog(&mut self) {
        if self.export.is_some() {
            self.status = tr("An export is already running").into();
            return;
        }
        let name = format!("{}.wav", self.project.name);
        if let Some(p) = rfd::FileDialog::new()
            .add_filter(tr("WAV audio"), &["wav"])
            .set_file_name(name)
            .save_file()
        {
            let sr = self
                .engine
                .as_ref()
                .map(|e| e.sample_rate())
                .unwrap_or(44_100);
            let project = self.project.clone();
            let progress = Arc::new(AtomicU32::new(0));
            let (tx, rx) = mpsc::channel();
            let (path, prog) = (p.clone(), progress.clone());
            // Rendering a long song takes seconds; keep the window responsive.
            std::thread::spawn(move || {
                let r = render::render_to_wav_with_progress(&project, &path, sr, |x| {
                    prog.store((x * 1000.0) as u32, Ordering::Relaxed);
                })
                .map_err(|e| format!("{e:#}"));
                let _ = tx.send(r);
            });
            self.export = Some(ExportJob {
                path: p,
                progress,
                result: rx,
            });
            self.status = trf("Exporting audio… {}%", &[&0]);
        }
    }

    /// Show export progress and pick up the result when the thread is done.
    fn poll_export(&mut self) {
        let Some(job) = &self.export else { return };
        let outcome = match job.result.try_recv() {
            Ok(Ok(secs)) => Some(trf(
                "Exported {}s to {}",
                &[&format!("{secs:.1}"), &job.path.display()],
            )),
            Ok(Err(e)) => Some(trf("Export failed: {}", &[&e])),
            Err(mpsc::TryRecvError::Disconnected) => Some(trf(
                "Export failed: {}",
                &[&tr("the export stopped unexpectedly")],
            )),
            Err(mpsc::TryRecvError::Empty) => None,
        };
        match outcome {
            Some(status) => {
                self.status = status;
                self.export = None;
            }
            None => {
                let pct = job.progress.load(Ordering::Relaxed) / 10;
                self.status = trf("Exporting audio… {}%", &[&pct]);
            }
        }
    }

    // ---- UI --------------------------------------------------------------

    fn shortcuts(&mut self, ctx: &egui::Context) {
        let undo = KeyboardShortcut::new(Modifiers::COMMAND, Key::Z);
        let redo = KeyboardShortcut::new(Modifiers::COMMAND, Key::Y);
        let redo2 = KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z);
        let save = KeyboardShortcut::new(Modifiers::COMMAND, Key::S);
        match self.queued_history.take() {
            Some(true) => self.redo(),
            Some(false) => self.undo(),
            None => {}
        }
        if ctx.input_mut(|i| i.consume_shortcut(&redo) || i.consume_shortcut(&redo2)) {
            self.redo();
        }
        if ctx.input_mut(|i| i.consume_shortcut(&undo)) {
            self.undo();
        }
        if ctx.input_mut(|i| i.consume_shortcut(&save)) {
            if self.path.is_some() {
                self.save_now();
                self.status = tr("Saved").into();
            } else {
                self.save_as_dialog();
            }
        }
        if !ctx.wants_keyboard_input() && ctx.input(|i| i.key_pressed(Key::Space)) {
            self.toggle_play();
        }
    }

    fn top_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("menu")
            .frame(theme::chrome_frame())
            .show(ctx, |ui| {
                // One row when everything fits, two rows in a narrow window.
                // Group widths are measured each frame and used for the next.
                let [menus_w, transport_w, right_w] = self.top_bar_widths;
                let one_row =
                    menus_w + transport_w + right_w + 40.0 <= ui.available_width();
                let mut widths = [0.0; 3];
                ui.horizontal(|ui| {
                    widths[0] = ui
                        .horizontal(|ui| {
                            crate::mode::switch(ui, self);
                            ui.add_space(6.0);
                            self.menus(ui);
                        })
                        .response
                        .rect
                        .width();
                    if one_row {
                        ui.add_space(10.0);
                        widths[1] = ui.horizontal(|ui| self.transport_row(ui)).response.rect.width();
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        widths[2] = ui
                            .horizontal(|ui| {
                                self.sound_button(ui);
                                crate::mode::lang_switch(ui, self);
                                ui.add_space(4.0);
                                let ai = theme::hero_button(
                                    ui,
                                    Some(Icon::Sparkles),
                                    tr("Ask AI"),
                                    theme::ACCENT,
                                    egui::vec2(108.0, 34.0),
                                )
                                .on_hover_text(tr(
                                    "Let an AI assistant (Claude, ChatGPT, …) write music into this song",
                                ));
                                if ai.clicked() {
                                    self.show_ai_help = true;
                                }
                                self.ui_rects.insert("ai", ai.rect);
                            })
                            .response
                            .rect
                            .width();
                    });
                });
                if !one_row {
                    ui.add_space(4.0);
                    widths[1] = ui.horizontal(|ui| self.transport_row(ui)).response.rect.width();
                }
                self.top_bar_widths = widths;
            });
        egui::TopBottomPanel::bottom("status")
            .frame(theme::chrome_frame().inner_margin(egui::Margin::symmetric(12, 5)))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let failed = i18n::is_failure(&self.status);
                    let dot = if failed {
                        theme::DANGER
                    } else if self.is_playing() {
                        theme::PLAY
                    } else {
                        theme::ACCENT
                    };
                    let (r, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
                    ui.painter().circle_filled(r.center(), 3.5, dot);
                    let text = RichText::new(&self.status).size(12.5);
                    ui.label(if failed {
                        text.color(theme::DANGER)
                    } else {
                        text.color(theme::TEXT_DIM)
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if let (Some(p), true) = (&self.path, self.pro()) {
                            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                            let tip = tr(
                                "The project file the AI edits through aidaw-mcp; changes appear here live.",
                            );
                            ui.label(RichText::new(name).size(12.0).color(theme::TEXT_DIM))
                                .on_hover_text(tip);
                            icons::label(ui, Icon::Sync).on_hover_text(tip);
                            ui.label(RichText::new("·").color(theme::TEXT_FAINT));
                        }
                        let text = if self.pro() {
                            trf(
                                "grid {} · key {} {}",
                                &[
                                    &snap_label(self.snap),
                                    &crate::scale::ROOTS[self.scale_root as usize % 12],
                                    &self.scale.label(),
                                ],
                            )
                        } else {
                            tr("Easy mode · switch to Pro (top left) for more controls").into()
                        };
                        ui.label(RichText::new(text).size(12.0).color(theme::TEXT_FAINT));
                    });
                });
            });
    }

    fn menus(&mut self, ui: &mut egui::Ui) {
        ui.menu_image_text_button(
            icons::image(Icon::Folder, 16.0, theme::TEXT),
            tr("File"),
            |ui| {
                if ui.button(tr("New song")).clicked() {
                    self.load_project(
                        starter_project(),
                        None,
                        tr("New song. Click in the piano roll to add notes!"),
                    );
                    ui.close_menu();
                }
                if ui.button(tr("Open…")).clicked() {
                    ui.close_menu();
                    self.open_dialog();
                }
                if ui.button(tr("Save As…  (Ctrl+S)")).clicked() {
                    ui.close_menu();
                    self.save_as_dialog();
                }
                if ui
                    .add_enabled(
                        self.export.is_none(),
                        egui::Button::new(if self.pro() {
                            tr("Export as WAV audio…")
                        } else {
                            tr("Save as audio file…")
                        }),
                    )
                    .on_hover_text(tr("Make an audio file you can share or put in other apps"))
                    .clicked()
                {
                    ui.close_menu();
                    self.export_wav_dialog();
                }
            },
        );
        let help = ui.menu_image_text_button(
            icons::image(Icon::Help, 16.0, theme::TEXT),
            tr("Help"),
            |ui| {
                if ui
                    .add(icons::button(Icon::Cap, tr("Tutorial (2 minutes)")))
                    .clicked()
                {
                    self.start_tutorial();
                    ui.close_menu();
                }
                if ui
                    .add(icons::button(Icon::Sparkles, tr("Let AI make music…")))
                    .clicked()
                {
                    self.show_ai_help = true;
                    ui.close_menu();
                }
                if ui
                    .add(icons::button(
                        Icon::Volume,
                        tr("Sound settings / no sound?"),
                    ))
                    .clicked()
                {
                    self.show_sound = true;
                    ui.close_menu();
                }
                if ui
                    .add(icons::button(Icon::Keyboard, tr("Keyboard & mouse")))
                    .clicked()
                {
                    self.show_shortcuts = true;
                    ui.close_menu();
                }
                if ui
                    .add(icons::button(Icon::Home, tr("Welcome screen")))
                    .clicked()
                {
                    self.show_welcome = true;
                    ui.close_menu();
                }
            },
        );
        self.ui_rects.insert("help", help.response.rect);
        // Undo is the beginner's safety net, so it is a real button, not hidden in a menu.
        if ui
            .add_enabled(
                self.history.can_undo(),
                icons::button(Icon::Undo, tr("Undo")),
            )
            .on_hover_text(tr(
                "Take back the last change (Ctrl+Z). Works for AI changes too.",
            ))
            .clicked()
        {
            self.queued_history = Some(false);
        }
        if ui
            .add_enabled(
                self.history.can_redo(),
                icons::button(Icon::Redo, tr("Redo")),
            )
            .on_hover_text(tr("Redo (Ctrl+Y)"))
            .clicked()
        {
            self.queued_history = Some(true);
        }
    }

    /// Transport, the Pro grid picker and the "not saved" reminder.
    fn transport_row(&mut self, ui: &mut egui::Ui) {
        self.transport(ui);
        if self.pro() {
            ui.add_space(4.0);
            ui.label(RichText::new(tr("Grid")).color(theme::TEXT_DIM))
                .on_hover_text(tr(
                    "Notes and clips snap to this grid. 1/16 is a good default; 1/4 = one beat.",
                ));
            egui::ComboBox::from_id_salt("snap")
                .width(70.0)
                .selected_text(snap_label(self.snap))
                .show_ui(ui, |ui| {
                    for s in [1.0, 0.5, 0.25, 0.125, 1.0 / 3.0, 1.0 / 6.0] {
                        ui.selectable_value(&mut self.snap, s, snap_label(s));
                    }
                });
        }
        if self.path.is_none()
            && ui
                .add(icons::button_colored(
                    Icon::Save,
                    tr("Not saved. Click to save"),
                    theme::WARN,
                ))
                .on_hover_text(tr(
                    "Save your song to a file. AI tools work on this file too.",
                ))
                .clicked()
        {
            self.save_as_dialog();
        }
    }

    /// Stop, Play and the LCD with the song position and tempo.
    fn transport(&mut self, ui: &mut egui::Ui) {
        let playing = self.is_playing();
        if ui
            .add(
                icons::icon_button(Icon::Stop, 18.0)
                    .corner_radius(17)
                    .min_size(egui::vec2(34.0, 34.0)),
            )
            .on_hover_text(tr(
                "Stop and go back to the start (of the loop, while the loop is on)",
            ))
            .clicked()
        {
            self.stop();
        }
        let play = theme::hero_button(
            ui,
            Some(if playing { Icon::Pause } else { Icon::Play }),
            if playing { tr("Pause") } else { tr("Play") },
            if playing { theme::PAUSE } else { theme::PLAY },
            egui::vec2(104.0, 34.0),
        )
        .on_hover_text(tr("Play or pause the song (Space bar)"));
        if play.clicked() {
            self.toggle_play();
        }
        self.ui_rects.insert("play", play.rect);
        let looping = self.project.active_loop().is_some();
        if ui
            .add(
                egui::Button::image(icons::image(Icon::Repeat, 18.0, theme::TEXT))
                    .selected(looping)
                    .corner_radius(17)
                    .min_size(egui::vec2(34.0, 34.0)),
            )
            .on_hover_text(if looping {
                tr("Loop is on: playback repeats the marked part (L). Click to play the whole song again.")
            } else {
                tr("Loop: play one part over and over while you work on it (L).\nDrag in the thin strip above the bar numbers to choose the part.")
            })
            .clicked()
        {
            self.toggle_loop();
        }
        ui.add_space(4.0);

        let bpb = self.project.beats_per_bar.max(1) as f64;
        let bar = (self.playhead / bpb).floor() as i64 + 1;
        let beat = (self.playhead % bpb).floor() as i64 + 1;
        let pro = self.pro();
        theme::card(theme::CANVAS, egui::Stroke::new(1.0_f32, theme::BORDER))
            .inner_margin(egui::Margin::symmetric(12, 2))
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let caption = |ui: &mut egui::Ui, t: &str| {
                    ui.label(
                        RichText::new(t)
                            .font(theme::bold_font(9.5))
                            .extra_letter_spacing(1.0)
                            .color(theme::TEXT_FAINT),
                    )
                };
                let lcd = |t: String| {
                    RichText::new(t)
                        .font(egui::FontId::monospace(19.0))
                        .color(theme::CYAN)
                };
                caption(ui, tr("BAR"));
                let pos = if pro {
                    format!("{bar:>3}.{beat}")
                } else {
                    format!("{bar:>3}")
                };
                ui.label(lcd(pos))
                    .on_hover_text(tr("Where the song is playing: bar number . beat"));
                let (r, _) = ui.allocate_exact_size(egui::vec2(1.0, 22.0), egui::Sense::hover());
                ui.painter().rect_filled(r, 0.0, theme::BORDER);
                caption(ui, if pro { "BPM" } else { tr("SPEED") }).on_hover_text(if pro {
                    tr("Tempo in beats per minute. 70 = slow ballad, 110 = pop, 170 = drum & bass.")
                } else {
                    tr("How fast the song plays (beats per minute).\n70 = slow and calm · 110 = pop · 140+ = fast and energetic.")
                });
                let mut bpm = self.project.bpm;
                ui.scope(|ui| {
                    let s = ui.style_mut();
                    s.drag_value_text_style = egui::TextStyle::Name("lcd".into());
                    s.text_styles.insert(
                        egui::TextStyle::Name("lcd".into()),
                        egui::FontId::monospace(19.0),
                    );
                    for w in [&mut s.visuals.widgets.inactive, &mut s.visuals.widgets.hovered] {
                        w.weak_bg_fill = Color32::TRANSPARENT;
                        w.bg_fill = Color32::TRANSPARENT;
                        w.fg_stroke.color = theme::CYAN;
                    }
                    s.visuals.widgets.inactive.bg_stroke = egui::Stroke::NONE;
                    s.spacing.button_padding = egui::vec2(2.0, 0.0);
                    if ui
                        .add(
                            egui::DragValue::new(&mut bpm)
                                .range(20.0..=400.0)
                                .speed(0.2)
                                .fixed_decimals(0),
                        )
                        .on_hover_text(tr("Drag left/right or click to type"))
                        .changed()
                    {
                        self.project.bpm = bpm;
                    }
                });
                if !pro {
                    let small = |t: &str| {
                        egui::Button::new(RichText::new(t).size(14.0))
                            .corner_radius(11)
                            .min_size(egui::vec2(22.0, 22.0))
                    };
                    if ui.add(small("−")).on_hover_text(tr("Slower")).clicked() {
                        bpm = (bpm - 5.0).round().max(20.0);
                    }
                    if ui.add(small("+")).on_hover_text(tr("Faster")).clicked() {
                        bpm = (bpm + 5.0).round().min(400.0);
                    }
                }
                if bpm != self.project.bpm {
                    self.project.bpm = bpm;
                }
            });
    }

    fn sound_button(&mut self, ui: &mut egui::Ui) {
        match &self.engine {
            Some(e) => {
                let peak = e.diagnostics().peak;
                crate::sound::meter(ui, peak).on_hover_text(tr("Output level"));
                let label = if self.pro() {
                    let name = e.device_name();
                    let short: String = name.chars().take(20).collect();
                    let ell = if short.len() < name.len() { "…" } else { "" };
                    format!("{short}{ell}")
                } else {
                    tr("Audio").to_string()
                };
                if ui
                    .add(icons::button(Icon::Volume, label))
                    .on_hover_text(tr("Sound settings: pick speakers, play a test beep"))
                    .clicked()
                {
                    self.show_sound = true;
                }
            }
            None => {
                if ui
                    .add(
                        icons::button_colored(
                            Icon::Mute,
                            tr("No sound. Click to fix"),
                            theme::DANGER,
                        )
                        .fill(theme::alpha(theme::DANGER, 30))
                        .stroke(egui::Stroke::new(1.0_f32, theme::alpha(theme::DANGER, 120))),
                    )
                    .clicked()
                {
                    self.show_sound = true;
                }
            }
        }
    }
}

fn snap_label(s: f64) -> String {
    const LABELS: [(f64, &str); 6] = [
        (1.0, "1/4"),
        (0.5, "1/8"),
        (0.25, "1/16"),
        (0.125, "1/32"),
        (1.0 / 3.0, "1/8T"),
        (1.0 / 6.0, "1/16T"),
    ];
    LABELS
        .iter()
        .find(|(v, _)| (v - s).abs() < 1e-9)
        .map(|(_, l)| l.to_string())
        .unwrap_or_else(|| trf("{} beat", &[&s]))
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Not in the middle of a drag, which holds note indexes into the song.
        if !ctx.input(|i| i.pointer.any_down()) {
            self.poll_disk();
        }
        self.poll_export();
        self.check_audio_alive();
        self.shortcuts(ctx);
        if let Some(e) = &self.engine {
            e.maintain();
            self.playhead = e.position();
        }

        self.edit_shortcuts(ctx);

        self.top_bar(ctx);
        // Panel sizes follow the window, so a small window still shows the
        // song, the notes and the side bar without anything overlapping.
        let room = ctx.available_rect();
        egui::SidePanel::left("inspector")
            .frame(theme::panel_frame())
            .resizable(true)
            .default_width((room.width() * 0.22).clamp(240.0, 300.0))
            .min_width(220.0)
            .max_width((room.width() * 0.4).max(220.0))
            .show(ctx, |ui| {
                crate::inspector::show(ui, self);
            });
        // The piano roll keeps its share of the height when the window is
        // resized; dragging its edge changes that share.
        let h = room.height();
        let mut roll = egui::TopBottomPanel::bottom("piano_roll")
            .frame(theme::panel_frame())
            .resizable(true)
            .min_height(150.0)
            .max_height((h - 170.0).max(150.0));
        if (h - self.roll_room_h).abs() > 0.5 {
            roll = roll.exact_height((self.roll_share * h).clamp(150.0, (h - 170.0).max(150.0)));
        }
        let shown = roll.show(ctx, |ui| {
            crate::piano_roll::show(ui, self);
        });
        if (h - self.roll_room_h).abs() <= 0.5 && h > 0.0 {
            self.roll_share = shown.response.rect.height() / h;
        }
        self.roll_room_h = h;
        egui::CentralPanel::default()
            .frame(theme::panel_frame())
            .show(ctx, |ui| {
                crate::arrange::show(ui, self);
            });

        self.overlays(ctx);

        // A whole drag or slider gesture is one undo step.
        self.commit_edits(ctx);
        self.system_font.poll(ctx);
        let names = std::mem::take(&mut self.check_names)
            .then(|| song_names(&self.project))
            .into_iter()
            .flatten();
        let devices = self.show_sound.then_some(&self.device_names);
        let devices = devices.into_iter().flatten().map(String::as_str);
        self.system_font
            .want_for(ctx, names.chain(devices).chain([self.status.as_str()]));
        // Safety net: never leave a held note ringing after the mouse is released.
        if self.held_key.is_some() && !ctx.input(|i| i.pointer.any_down()) {
            self.release_note();
        }
        if self.save_at.is_some_and(|t| Instant::now() >= t) {
            self.save_now();
        }

        if self.is_playing() {
            ctx.request_repaint();
        } else if self.export.is_some() {
            ctx.request_repaint_after(Duration::from_millis(100));
        } else {
            ctx.request_repaint_after(POLL_INTERVAL);
        }
    }
}

impl App {
    fn start_tutorial(&mut self) {
        self.show_welcome = false;
        self.tutorial = Some(Tutorial::start(self));
    }

    /// Welcome screen, tutorial and help windows, drawn on top of everything.
    fn overlays(&mut self, ctx: &egui::Context) {
        if self.show_welcome {
            match onboarding::show_welcome(ctx, self) {
                WelcomeChoice::None => {}
                WelcomeChoice::Tutorial => self.start_tutorial(),
                WelcomeChoice::Demo => {
                    self.show_welcome = false;
                    self.load_project(
                        onboarding::demo_project(),
                        None,
                        tr("Demo song loaded. Change anything you like!"),
                    );
                    self.toggle_play();
                }
                WelcomeChoice::Blank => {
                    self.show_welcome = false;
                    self.load_project(
                        starter_project(),
                        None,
                        tr("New song. Click in the piano roll to add notes!"),
                    );
                }
                WelcomeChoice::AiHelp => {
                    self.show_welcome = false;
                    self.show_ai_help = true;
                }
                WelcomeChoice::Close => self.show_welcome = false,
            }
        }
        if let Some(mut tut) = self.tutorial.take() {
            if onboarding::show_tutorial(ctx, self, &mut tut) {
                self.tutorial = Some(tut);
            }
        }
        if self.show_ai_help {
            let mut open = true;
            onboarding::show_ai_help(ctx, self, &mut open);
            self.show_ai_help &= open;
        }
        if self.show_shortcuts {
            let mut open = true;
            onboarding::show_shortcuts(ctx, &mut open);
            self.show_shortcuts = open;
        }
        if self.show_sound {
            let mut open = true;
            crate::sound::show_window(ctx, self, &mut open);
            self.show_sound &= open;
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }

    /// Add a track using a friendly preset and select it.
    pub fn add_track(&mut self, preset: &presets::Preset) {
        let base = presets::plain_name(tr(preset.name));
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
        if let Some(v) = self.run_op(Op::AddTrack {
            name,
            instrument: Some(preset.instrument.clone()),
            gain: None,
            pan: None,
            reverb: Some(preset.category.reverb()),
        }) {
            let id = v["track_id"].as_str().map(String::from);
            // Give the new track an empty clip so there is something to draw in right away.
            if let Some(id) = &id {
                let start = self.playhead.max(0.0);
                let bpb = self.project.beats_per_bar.max(1) as f64;
                self.add_clip(id, (start / bpb).floor() * bpb);
            }
            self.selected_track = id;
        }
    }

    pub fn add_clip(&mut self, track_id: &str, start: f64) {
        let length = self.project.beats_per_bar.max(1) as f64 * 4.0;
        let parts = self.project.track(track_id).map_or(0, |t| t.clips.len());
        let op = Op::AddClip {
            track_id: track_id.into(),
            start,
            length,
            name: Some(trf("Part {}", &[&(parts + 1)])),
            notes: Vec::new(),
        };
        if let Some(v) = self.run_op(op) {
            self.selected_track = Some(track_id.into());
            self.selected_clip = v["clip_id"].as_str().map(String::from);
            self.selected_notes.clear();
            self.roll_scroll_to_notes = true;
        }
    }

    pub fn delete_selected_clip(&mut self) {
        if let Some(id) = self.selected_clip.take() {
            self.run_op(Op::RemoveClip { clip_id: id });
            self.selected_notes.clear();
        }
    }
}

/// For pasted or duplicated notes that landed where the clip can't grow.
fn past_the_end() -> &'static str {
    tr("Some notes landed past the end of the clip and won't play: the next clip on this track starts there")
}

/// "1 note" / "3 notes".
pub fn notes_text(n: usize) -> String {
    if n == 1 {
        tr("1 note").into()
    } else {
        trf("{} notes", &[&n])
    }
}

// ---- editing keys ------------------------------------------------------------

impl App {
    pub fn bar(&self) -> f64 {
        self.project.beats_per_bar.max(1) as f64
    }

    /// The clip open in the piano roll.
    fn open_clip_mut(&mut self) -> Option<&mut aidaw_core::Clip> {
        let id = self.selected_clip.as_deref()?;
        self.project
            .tracks
            .iter_mut()
            .flat_map(|t| t.clips.iter_mut())
            .find(|c| c.id == id)
    }

    /// How long the open clip may grow before it runs into the next clip on
    /// its track.
    fn room_to_grow(&self) -> f64 {
        let Some((track, clip)) = self
            .selected_clip
            .as_deref()
            .and_then(|id| self.project.clip(id))
        else {
            return f64::INFINITY;
        };
        track
            .clips
            .iter()
            .filter(|c| c.id != clip.id && c.start >= clip.start)
            .map(|c| c.start - clip.start)
            .fold(f64::INFINITY, f64::min)
    }

    /// Index of the track whose clip is open in the piano roll.
    fn open_track(&self) -> Option<usize> {
        let id = self.selected_clip.as_deref()?;
        self.project
            .tracks
            .iter()
            .position(|t| t.clips.iter().any(|c| c.id == id))
    }

    /// Copy, paste, duplicate, delete, nudge and quantize from the keyboard.
    /// Each is one undo step (see `commit_edits`).
    fn edit_shortcuts(&mut self, ctx: &egui::Context) {
        if ctx.wants_keyboard_input() {
            return; // typing in a text box: the keys are its own
        }
        let (mut copy, mut cut, mut paste) = (false, false, None);
        let mut fresh_up_down = false;
        ctx.input(|i| {
            for e in &i.events {
                match e {
                    egui::Event::Copy => copy = true,
                    egui::Event::Cut => cut = true,
                    egui::Event::Paste(text) => paste = Some(text.clone()),
                    egui::Event::Key {
                        key: Key::ArrowUp | Key::ArrowDown,
                        pressed: true,
                        repeat: false,
                        ..
                    } => fresh_up_down = true,
                    _ => {}
                }
            }
        });
        let duplicate = ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::D));
        let select_all = ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::A));
        let (delete, escape, quantize, across, up, shift) = ctx.input(|i| {
            let plain = !i.modifiers.command && !i.modifiers.alt;
            let key = |k| plain && i.key_pressed(k);
            (
                key(Key::Delete) || key(Key::Backspace),
                i.key_pressed(Key::Escape),
                key(Key::Q) && !i.modifiers.shift,
                key(Key::ArrowRight) as i32 - key(Key::ArrowLeft) as i32,
                key(Key::ArrowUp) as i32 - key(Key::ArrowDown) as i32,
                i.modifiers.shift,
            )
        });
        let (toggle_loop, split) = ctx.input(|i| {
            let plain = !i.modifiers.command && !i.modifiers.alt && !i.modifiers.shift;
            (
                plain && i.key_pressed(Key::L),
                plain && i.key_pressed(Key::S),
            )
        });
        if toggle_loop {
            self.toggle_loop();
        }
        if split {
            self.split_at_playhead();
        }

        // What gets pasted depends on what was copied, not on the focus.
        if let Some(text) = paste {
            self.paste(&text);
        }
        if select_all {
            self.edit_focus = EditFocus::Notes;
            self.select_all_notes();
        }
        match self.edit_focus {
            EditFocus::Notes => {
                if escape {
                    self.selected_notes.clear();
                }
                if copy || cut {
                    self.copy_notes(ctx, cut);
                }
                if duplicate {
                    self.duplicate_notes();
                }
                if delete {
                    self.delete_notes();
                }
                if quantize {
                    self.quantize();
                }
                if across != 0 {
                    self.nudge_notes(across, shift);
                }
                if up != 0 {
                    self.transpose_notes(up, shift, fresh_up_down);
                }
            }
            EditFocus::Clips => {
                if copy || cut {
                    self.copy_clip(ctx, cut);
                }
                if duplicate {
                    self.duplicate_clip();
                }
                if delete {
                    self.delete_selected_clip();
                }
                if across != 0 {
                    self.nudge_clip(across);
                }
            }
            EditFocus::Automation => {
                if escape {
                    self.selected_point = None;
                }
                if delete {
                    if let Some((key, i)) = self.selected_point.take() {
                        crate::lanes::delete_point(self, &key, i);
                        self.status = tr("Deleted the point (Ctrl+Z brings it back)").into();
                    }
                }
            }
        }
    }

    pub fn select_all_notes(&mut self) {
        let count = self
            .selected_clip
            .as_deref()
            .and_then(|id| self.project.clip(id))
            .map_or(0, |(_, c)| c.notes.len());
        self.selected_notes = (0..count).collect();
    }

    fn copy_notes(&mut self, ctx: &egui::Context, cut: bool) {
        let bar = self.bar();
        let Some((_, clip)) = self
            .selected_clip
            .as_deref()
            .and_then(|id| self.project.clip(id))
        else {
            return;
        };
        let notes = editing::copy_notes(clip, &self.selected_notes, bar);
        if notes.is_empty() {
            self.status =
                tr("Select some notes first: click one, or Shift+drag around several").into();
            return;
        }
        let n = notes_text(notes.len());
        ctx.copy_text(Copied::Notes(notes).to_text());
        if cut {
            self.delete_notes();
            self.status = trf("Cut {} (Ctrl+V pastes at the playhead)", &[&n]);
        } else {
            self.status = trf("Copied {} (Ctrl+V pastes at the playhead)", &[&n]);
        }
    }

    /// Paste notes into the bar where the playhead is (the clip's first bar
    /// when the playhead is outside the clip), or a clip onto the selected
    /// track at the playhead's bar.
    fn paste(&mut self, text: &str) {
        let (bar, playhead) = (self.bar(), self.playhead.max(0.0));
        match Copied::from_text(text) {
            Some(Copied::Notes(mut notes)) => {
                editing::to_first_bar(&mut notes, bar);
                let room = self.room_to_grow();
                let Some(clip) = self.open_clip_mut() else {
                    self.status = tr("Click a clip first, then paste the notes into it").into();
                    return;
                };
                let rel = playhead - clip.start;
                let at = if (0.0..clip.length).contains(&rel) {
                    editing::bar_start(rel, bar)
                } else {
                    0.0
                };
                let added = editing::paste_notes(clip, &notes, at, bar, room);
                self.status = if editing::any_silent(clip, &added) {
                    past_the_end().into()
                } else {
                    trf("Pasted {}", &[&notes_text(added.len())])
                };
                self.selected_notes = added;
                self.edit_focus = EditFocus::Notes;
                self.roll_reveal = true;
            }
            Some(Copied::Clip(clip)) => {
                let track = self
                    .selected_track
                    .clone()
                    .or_else(|| self.project.tracks.first().map(|t| t.id.clone()));
                let Some(track_id) = track else {
                    self.status =
                        tr("Add an instrument first (+ Add instrument on the left)").into();
                    return;
                };
                let wanted = editing::bar_start(playhead, bar);
                let start = self.free_spot(&track_id, wanted, clip.length);
                let op = Op::AddClip {
                    track_id: track_id.clone(),
                    start,
                    length: clip.length,
                    name: Some(clip.name),
                    notes: clip.notes,
                };
                if let Some(v) = self.run_op(op) {
                    self.selected_track = Some(track_id);
                    self.selected_clip = v["clip_id"].as_str().map(String::from);
                    self.selected_notes.clear();
                    self.edit_focus = EditFocus::Clips;
                    self.roll_scroll_to_notes = true;
                    self.status = if start > wanted {
                        self.moved_to(start)
                    } else {
                        tr("Pasted the clip").into()
                    };
                }
            }
            None => {
                self.status =
                    tr("Nothing to paste: copy some notes or a clip first (Ctrl+C)").into();
            }
        }
    }

    fn duplicate_notes(&mut self) {
        let (grid, bar, room) = (self.grid(), self.bar(), self.room_to_grow());
        let sel = self.selected_notes.clone();
        if let Some(clip) = self.open_clip_mut() {
            let copies = editing::duplicate_notes(clip, &sel, grid, bar, room);
            if editing::any_silent(clip, &copies) {
                self.status = past_the_end().into();
            }
            if !copies.is_empty() {
                self.selected_notes = copies;
                self.roll_reveal = true;
            }
        }
    }

    fn delete_notes(&mut self) {
        let sel = std::mem::take(&mut self.selected_notes);
        if let Some(clip) = self.open_clip_mut() {
            editing::delete_notes(clip, &sel);
        }
    }

    /// Snap the selected notes (or the whole clip) to the grid.
    pub fn quantize(&mut self) {
        let grid = self.grid();
        let sel = self.selected_notes.clone();
        let Some(clip) = self.open_clip_mut() else {
            return;
        };
        let moved = editing::quantize_notes(clip, &sel, grid);
        self.status = if moved == 0 {
            tr("Already on the grid").into()
        } else {
            trf("Moved {} onto the grid", &[&notes_text(moved)])
        };
    }

    /// Loosen the selected notes (or the whole clip) a little, like a player.
    pub fn humanize(&mut self) {
        self.humanize_seed = self.humanize_seed.wrapping_add(1);
        let (sel, seed) = (self.selected_notes.clone(), self.humanize_seed);
        let Some(clip) = self.open_clip_mut() else {
            return;
        };
        let changed = editing::humanize_notes(clip, &sel, seed);
        self.status = trf(
            "Humanized {}: their timing and loudness vary a little now (Ctrl+Z undoes it)",
            &[&notes_text(changed)],
        );
    }

    /// Left/right: one grid step, or a bar with Shift.
    fn nudge_notes(&mut self, dir: i32, by_bar: bool) {
        let step = if by_bar { self.bar() } else { self.grid() } * dir as f64;
        let sel = self.selected_notes.clone();
        if let Some(clip) = self.open_clip_mut() {
            if editing::move_notes(clip, &sel, step) {
                self.roll_reveal = true;
            } else if !sel.is_empty() {
                self.status = tr("The notes can't move before the start of the clip").into();
            }
        }
    }

    /// Up/down: one note of the key (a semitone when the key is off), or an
    /// octave with Shift. Plays the moved notes on a fresh key press.
    fn transpose_notes(&mut self, dir: i32, octave: bool, hear: bool) {
        let Some(track) = self.open_track() else {
            return;
        };
        let (key, root) = (self.key_for(track), self.scale_root);
        let sel = self.selected_notes.clone();
        let Some(clip) = self.open_clip_mut() else {
            return;
        };
        let moved = editing::transpose_notes(clip, &sel, |p| {
            if octave {
                u8::try_from(p as i32 + 12 * dir).ok().filter(|p| *p <= 127)
            } else {
                key.step(root, p, dir)
            }
        });
        let pitches: Vec<u8> = sel
            .iter()
            .filter_map(|&i| clip.notes.get(i).map(|n| n.pitch))
            .take(6)
            .collect();
        self.roll_reveal |= moved;
        if moved && hear {
            self.preview_notes(track, &pitches);
        }
    }

    fn copy_clip(&mut self, ctx: &egui::Context, cut: bool) {
        let Some((_, clip)) = self
            .selected_clip
            .as_deref()
            .and_then(|id| self.project.clip(id))
        else {
            return;
        };
        ctx.copy_text(Copied::Clip(clip.clone()).to_text());
        if cut {
            self.delete_selected_clip();
            self.status = tr("Cut the clip (Ctrl+V pastes it at the playhead)").into();
        } else {
            self.status = tr("Copied the clip (Ctrl+V pastes it at the playhead)").into();
        }
    }

    /// Put a copy of the selected clip right after it and select the copy,
    /// so pressing Ctrl+D again keeps repeating it.
    pub fn duplicate_clip(&mut self) {
        let Some((track_id, clip)) = self
            .selected_clip
            .as_deref()
            .and_then(|id| self.project.clip(id))
            .map(|(t, c)| (t.id.clone(), c.clone()))
        else {
            return;
        };
        let after = clip.end();
        let start = self.free_spot(&track_id, after, clip.length);
        let op = Op::AddClip {
            track_id: track_id.clone(),
            start,
            length: clip.length,
            name: Some(clip.name),
            notes: clip.notes,
        };
        if let Some(v) = self.run_op(op) {
            self.selected_track = Some(track_id);
            self.selected_clip = v["clip_id"].as_str().map(String::from);
            self.selected_notes.clear();
            self.status = if start > after {
                self.moved_to(start)
            } else {
                tr("Duplicated the clip (Ctrl+D again for another copy)").into()
            };
        }
    }

    /// Where a clip `length` beats long fits on the track, at `from` or later.
    fn free_spot(&self, track_id: &str, from: f64, length: f64) -> f64 {
        let clips = self.project.track(track_id).map_or(&[][..], |t| &t.clips);
        editing::free_spot(clips, from, length)
    }

    fn moved_to(&self, start: f64) -> String {
        let bar = (start / self.bar()).floor() as i64 + 1;
        trf(
            "That spot was taken, so the copy went to bar {}",
            &[&bar.to_string()],
        )
    }

    /// Left/right: move the selected clip by a bar.
    fn nudge_clip(&mut self, dir: i32) {
        let bar = self.bar();
        if let Some(clip) = self.open_clip_mut() {
            clip.start = (clip.start + bar * dir as f64).max(0.0);
        }
    }
}

// ---- loop, split and track order ----------------------------------------------

impl App {
    /// What loop edges and splits snap to: the grid in Pro mode, whole bars
    /// in Easy mode (where parts always line up with bars).
    pub fn bar_or_grid(&self) -> f64 {
        if self.pro() {
            self.grid()
        } else {
            self.bar()
        }
    }

    pub fn snap_bar_or_grid(&self, beat: f64) -> f64 {
        let step = self.bar_or_grid();
        (beat / step).round() * step
    }

    /// Loop `start..end` (beats) and switch the loop on.
    pub fn set_loop(&mut self, start: f64, end: f64) {
        let region = LoopRegion {
            start: start.max(0.0),
            end,
            on: true,
        };
        if region.is_valid() {
            self.project.transport.loop_region = Some(region);
        }
    }

    /// The Loop button and L: switch the loop on or off. The first time it
    /// loops the clip open in the piano roll, or else the 4 bars from the
    /// playhead's bar.
    pub fn toggle_loop(&mut self) {
        let region = match self
            .project
            .transport
            .loop_region
            .filter(LoopRegion::is_valid)
        {
            Some(r) => LoopRegion { on: !r.on, ..r },
            None => {
                let bar = self.bar();
                let (start, end) = match self
                    .selected_clip
                    .as_deref()
                    .and_then(|id| self.project.clip(id))
                {
                    Some((_, c)) if c.length >= aidaw_core::transport::MIN_LOOP_BEATS => {
                        (c.start, c.end())
                    }
                    _ => {
                        let start = (self.playhead.max(0.0) / bar).floor() * bar;
                        (start, start + 4.0 * bar)
                    }
                };
                LoopRegion {
                    start,
                    end,
                    on: true,
                }
            }
        };
        self.project.transport.loop_region = Some(region);
        self.status = self.loop_status();
    }

    /// Loop just this clip.
    pub fn loop_clip(&mut self, clip_id: &str) {
        if let Some((_, c)) = self.project.clip(clip_id) {
            let (start, end) = (c.start, c.end());
            self.set_loop(start, end);
            self.status = self.loop_status();
        }
    }

    /// Take the loop region away; playback goes back to looping the song.
    pub fn remove_loop(&mut self) {
        self.project.transport.loop_region = None;
        self.status = tr("Loop removed: the whole song plays").into();
    }

    /// "Loop on: bars 5–8 play over and over", for the status bar.
    pub fn loop_status(&self) -> String {
        match self.project.transport.loop_region {
            Some(r) if r.on && r.is_valid() => {
                let bar = self.bar();
                let first = (r.start / bar + 1e-9).floor() as i64 + 1;
                let last = ((r.end / bar - 1e-9).ceil() as i64).max(first);
                if first == last {
                    trf(
                        "Loop on: bar {} plays over and over (L turns the loop off)",
                        &[&first],
                    )
                } else {
                    trf(
                        "Loop on: bars {}–{} play over and over (L turns the loop off)",
                        &[&first, &last],
                    )
                }
            }
            Some(_) => tr("Loop off: the whole song plays (L turns the loop back on)").into(),
            None => tr("No loop: the whole song plays").into(),
        }
    }

    /// S: cut the clip under the playhead in two there.
    pub fn split_at_playhead(&mut self) {
        let at = self.playhead;
        self.split(None, at);
    }

    /// Cut a clip in two near beat `at` (song time; snapped like the loop).
    /// Without a `clip_id` it cuts the open clip when `at` is inside it,
    /// else the selected track's clip there. The part after the cut is
    /// selected, so pressing S again while playing keeps cutting.
    pub fn split(&mut self, clip_id: Option<&str>, at: f64) {
        let inside = |c: &aidaw_core::Clip| at > c.start && at < c.end();
        let target = clip_id
            .and_then(|id| self.project.clip(id))
            .or_else(|| {
                self.selected_clip
                    .as_deref()
                    .and_then(|id| self.project.clip(id))
                    .filter(|(_, c)| inside(c))
            })
            .or_else(|| {
                let track = self.project.track(self.selected_track.as_deref()?)?;
                track.clips.iter().find(|c| inside(c)).map(|c| (track, c))
            })
            .map(|(t, c)| (t.id.clone(), c.id.clone(), c.start, c.end()));
        let Some((track_id, clip_id, start, end)) = target else {
            self.status = tr(
                "Put the playhead inside a clip first (click the bar numbers), then press S to cut it there",
            )
            .into();
            return;
        };
        let cut = self.snap_bar_or_grid(at);
        if cut <= start + 1e-9 || cut >= end - 1e-9 {
            self.status = if self.pro() {
                tr("That is the edge of the clip: cut a little further inside it")
            } else {
                tr("Parts are cut at bar lines, and the nearest one here is the edge of the part: move the playhead further into it")
            }
            .into();
            return;
        }
        let op = Op::SplitClip { clip_id, at: cut };
        if let Some(v) = self.run_op(op) {
            let bar = (cut / self.bar()).floor() as i64 + 1;
            self.selected_track = Some(track_id);
            self.selected_clip = v["new_clip_id"].as_str().map(String::from);
            self.selected_notes.clear();
            self.edit_focus = EditFocus::Clips;
            self.roll_scroll_to_notes = true;
            self.status = trf(
                "Cut the clip in two at bar {} (Ctrl+Z puts it back together)",
                &[&bar],
            );
        }
    }

    /// Put a track on row `index` (0 = the top one).
    pub fn move_track(&mut self, track_id: &str, index: usize) {
        self.run_op(Op::MoveTrack {
            track_id: track_id.into(),
            index,
        });
    }

    /// Alt+drag: put a copy of the clip on top of it (everything but the
    /// id is the same) and return the copy's id, to drag that instead.
    pub fn copy_clip_in_place(&mut self, clip_id: &str) -> Option<String> {
        let id = self.project.next_id("c");
        let track = self
            .project
            .tracks
            .iter_mut()
            .find(|t| t.clips.iter().any(|c| c.id == clip_id))?;
        let mut copy = track.clips.iter().find(|c| c.id == clip_id)?.clone();
        copy.id.clone_from(&id);
        track.clips.push(copy);
        track.clips.sort_by(|a, b| a.start.total_cmp(&b.start));
        Some(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aidaw_core::model::FORMAT_VERSION;
    use std::fs;

    /// The app around a song, without a window, sound device or settings file.
    fn app(project: Project, path: Option<PathBuf>) -> App {
        let no_sound = Err("no sound device in tests".to_string());
        App::with(project, path, Settings::default(), no_sound, String::new())
    }

    /// The starter song with these (pitch, start) notes in its piano clip
    /// `c1`; `c2` is the drum clip.
    fn song(notes: &[(u8, f64)]) -> Project {
        let mut p = starter_project();
        p.clip_mut("c1").unwrap().notes = notes
            .iter()
            .map(|&(pitch, start)| Note {
                pitch,
                start,
                duration: 1.0,
                velocity: 100,
            })
            .collect();
        p
    }

    fn notes(a: &App) -> Vec<(u8, f64)> {
        clip_notes(&a.project, "c1")
            .iter()
            .map(|n| (n.pitch, n.start))
            .collect()
    }

    fn sel(i: &[usize]) -> BTreeSet<usize> {
        i.iter().copied().collect()
    }

    /// One frame: `edit` changes things the way the panels would, then the
    /// app looks for edits, as at the end of `update`.
    fn frame(
        a: &mut App,
        ctx: &egui::Context,
        events: Vec<egui::Event>,
        edit: impl FnOnce(&mut App, &egui::Context),
    ) {
        let mut edit = Some(edit);
        let input = egui::RawInput {
            events,
            ..Default::default()
        };
        let _ = ctx.run(input, |ctx| {
            if let Some(edit) = edit.take() {
                edit(a, ctx);
            }
            a.commit_edits(ctx);
        });
    }

    fn button(down: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos: egui::pos2(100.0, 100.0),
            button: egui::PointerButton::Primary,
            pressed: down,
            modifiers: Modifiers::NONE,
        }
    }

    fn moved() -> egui::Event {
        egui::Event::PointerMoved(egui::pos2(120.0, 100.0))
    }

    fn key(key: Key) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        }
    }

    /// A fresh folder for files, removed again when the test is done.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> TempDir {
            let dir = std::env::temp_dir().join(format!("aidaw-gui-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            TempDir(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// Write `text` the way another program would, with a later file time
    /// than anything written before, however coarse the file system's clock.
    fn write_later(path: &std::path::Path, text: &str) {
        fs::write(path, text).unwrap();
        let later = SystemTime::now() + Duration::from_secs(5);
        fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(later)
            .unwrap();
    }

    fn poll_now(a: &mut App) {
        a.last_poll = Instant::now().checked_sub(POLL_INTERVAL).unwrap();
        a.poll_disk();
    }

    #[test]
    fn the_starter_song_is_ready_to_draw_in() {
        let p = starter_project();
        let drums: Vec<bool> = p.tracks.iter().map(|t| t.instrument.is_drums()).collect();
        assert_eq!(drums, [false, true], "keys, then drums");
        for t in &p.tracks {
            assert_eq!(t.clips.len(), 1);
            let c = &t.clips[0];
            assert_eq!((c.start, c.length), (0.0, 16.0), "4 bars from the start");
            assert!(c.notes.is_empty());
        }
        let a = app(p, None);
        assert_eq!(a.selected_track.as_deref(), Some("t1"));
        assert_eq!(
            a.selected_clip.as_deref(),
            Some("c1"),
            "the piano part is open"
        );
        assert!(!a.history.can_undo());
    }

    #[test]
    fn easy_mode_draws_on_sixteenths_and_pro_on_the_chosen_grid() {
        let mut a = app(song(&[]), None);
        a.snap = 1.0;
        assert_eq!(a.grid(), 0.25, "Easy mode hides the grid picker");
        assert_eq!((a.snap_beat(1.13), a.snap_floor(1.24)), (1.25, 1.0));
        a.settings.pro = true;
        assert_eq!(a.grid(), 1.0);
        assert_eq!((a.snap_beat(1.6), a.snap_floor(1.6)), (2.0, 1.0));
        a.snap = 1.0 / 3.0;
        assert!((a.snap_beat(0.7) - 2.0 / 3.0).abs() < 1e-12, "triplets");
    }

    #[test]
    fn notes_stay_in_key_except_on_drums() {
        let mut a = app(song(&[]), None); // C major, Stay in key on
        assert_eq!(a.fit_pitch(0, 61), 60, "C# is not in C major: down to C");
        assert_eq!(a.fit_pitch(0, 62), 62);
        assert_eq!(a.fit_pitch(1, 61), 61, "drums: every row is a drum");
        a.scale_lock = false;
        assert_eq!(a.fit_pitch(0, 61), 60, "Easy mode always stays in key");
        a.settings.pro = true;
        assert_eq!(a.fit_pitch(0, 61), 61, "Pro: only with Stay in key on");
        a.scale_lock = true;
        a.scale = Scale::Off;
        assert_eq!(a.fit_pitch(0, 61), 61);
        a.scale = Scale::Minor;
        a.scale_root = 9; // A minor
        assert_eq!(a.fit_pitch(0, 66), 65, "F# down to F");
    }

    #[test]
    fn a_drag_is_one_undo_step_and_the_notes_are_sorted_when_it_ends() {
        let mut a = app(song(&[(60, 0.0), (64, 1.0), (67, 2.0)]), None);
        let ctx = egui::Context::default();
        a.selected_notes = sel(&[0]);
        frame(&mut a, &ctx, vec![button(true)], |_, _| {});
        // C4 is dragged to beat 3, a beat per frame.
        for beat in [1.0, 2.0, 3.0] {
            frame(&mut a, &ctx, vec![moved()], |a, _| {
                a.project.clip_mut("c1").unwrap().notes[0].start = beat;
            });
        }
        assert!(!a.history.can_undo(), "not while the mouse is held");
        assert_eq!(notes(&a)[0], (60, 3.0), "no sorting under a drag");
        frame(&mut a, &ctx, vec![button(false)], |_, _| {});
        assert_eq!(notes(&a), [(64, 1.0), (67, 2.0), (60, 3.0)]);
        assert_eq!(a.selected_notes, sel(&[2]), "still the dragged note");

        a.undo();
        assert_eq!(notes(&a), [(60, 0.0), (64, 1.0), (67, 2.0)]);
        assert_eq!(a.selected_notes, sel(&[0]));
        assert!(!a.history.can_undo(), "the whole drag was one step");
        a.redo();
        assert_eq!(notes(&a), [(64, 1.0), (67, 2.0), (60, 3.0)]);
        assert_eq!(a.selected_notes, sel(&[2]));
    }

    #[test]
    fn deleted_notes_come_back_selected() {
        let mut a = app(song(&[(60, 0.0), (64, 1.0), (67, 2.0)]), None);
        let ctx = egui::Context::default();
        // Selected in one frame (a click), deleted in the next.
        frame(&mut a, &ctx, vec![button(false)], |a, _| {
            a.selected_notes = sel(&[0, 2])
        });
        frame(&mut a, &ctx, vec![key(Key::Delete)], |a, ctx| {
            a.edit_shortcuts(ctx)
        });
        assert_eq!(notes(&a), [(64, 1.0)]);
        assert!(a.selected_notes.is_empty());
        a.undo();
        assert_eq!(notes(&a).len(), 3);
        assert_eq!(a.selected_notes, sel(&[0, 2]));
    }

    #[test]
    fn pasted_notes_land_in_order_and_selected() {
        let mut a = app(song(&[(60, 0.0), (64, 4.0)]), None);
        let ctx = egui::Context::default();
        let chat = r#"Here you go: [{"pitch": "G4", "start": 1, "duration": 1}]"#;
        let paste = egui::Event::Paste(chat.into());
        frame(&mut a, &ctx, vec![paste], |a, ctx| a.edit_shortcuts(ctx));
        assert_eq!(notes(&a), [(60, 0.0), (67, 1.0), (64, 4.0)]);
        assert_eq!(a.selected_notes, sel(&[1]), "the pasted G4");
        a.undo();
        assert_eq!(notes(&a), [(60, 0.0), (64, 4.0)]);
    }

    #[test]
    fn frames_without_clicks_or_keys_look_for_edits_only_now_and_then() {
        let mut a = app(song(&[]), None);
        let ctx = egui::Context::default();
        a.last_check = Instant::now() + Duration::from_secs(3600);
        frame(&mut a, &ctx, vec![moved()], |a, _| a.project.bpm = 130.0);
        assert!(!a.history.can_undo(), "the mouse only passed over");
        frame(&mut a, &ctx, vec![key(Key::Escape)], |_, _| {});
        assert!(a.history.can_undo(), "a key press looks");
        // Playback: no input at all, but the song is still looked at.
        a.last_check = Instant::now().checked_sub(CHECK_INTERVAL).unwrap();
        frame(&mut a, &ctx, vec![], |a, _| a.project.bpm = 90.0);
        assert_eq!(a.history.take_undo().unwrap().project.bpm, 130.0);
    }

    #[test]
    fn the_ai_s_changes_load_keep_the_selection_and_can_be_undone() {
        let dir = TempDir::new("reload");
        let path = dir.0.join("song.aidaw.json");
        let mine = song(&[(60, 0.0), (64, 1.0), (67, 2.0)]);
        pio::save(&mine, &path).unwrap();
        let mut a = app(mine.clone(), Some(path.clone()));
        a.selected_notes = sel(&[1]); // E4

        // The AI adds a note in front of the others.
        let mut theirs = mine.clone();
        theirs.clip_mut("c1").unwrap().notes.insert(
            0,
            Note {
                pitch: 55,
                start: 0.0,
                duration: 0.5,
                velocity: 90,
            },
        );
        write_later(&path, &serde_json::to_string(&theirs).unwrap());
        poll_now(&mut a);
        assert_eq!(a.project, theirs);
        assert_eq!(a.selected_notes, sel(&[2]), "still E4");
        assert!(a.save_at.is_none(), "the file holds this version already");

        a.undo();
        assert_eq!(a.project, mine);
        assert_eq!(a.selected_notes, sel(&[1]));
        a.save_now();
        assert_eq!(
            pio::load(&path).unwrap(),
            mine,
            "undo saves the old version"
        );
    }

    #[test]
    fn a_save_keeps_the_human_s_version_and_the_ai_s_one_on_the_undo_stack() {
        let dir = TempDir::new("conflict");
        let path = dir.0.join("song.aidaw.json");
        let start = song(&[(60, 0.0)]);
        pio::save(&start, &path).unwrap();
        let mut a = app(start.clone(), Some(path.clone()));
        let ctx = egui::Context::default();
        frame(&mut a, &ctx, vec![key(Key::Escape)], |a, _| {
            a.project.bpm = 140.0
        });
        // The AI writes between two polls.
        let mut theirs = start.clone();
        theirs.bpm = 90.0;
        write_later(&path, &serde_json::to_string(&theirs).unwrap());
        a.save_now();
        assert_eq!(
            pio::load(&path).unwrap().bpm,
            140.0,
            "the human's edit is saved"
        );
        a.undo();
        assert_eq!(a.project.bpm, 90.0, "Ctrl+Z shows the AI's version");
        a.undo();
        assert_eq!(a.project.bpm, start.bpm);
    }

    #[test]
    fn a_file_from_a_newer_aidaw_is_never_overwritten() {
        let dir = TempDir::new("newer");
        let path = dir.0.join("song.aidaw.json");
        let mine = song(&[(60, 0.0)]);
        pio::save(&mine, &path).unwrap();
        let mut a = app(mine.clone(), Some(path.clone()));
        let newer = format!(
            r#"{{"version": {}, "name": "x", "bpm": 100, "tracks": [], "new_thing": 1}}"#,
            FORMAT_VERSION + 1
        );
        write_later(&path, &newer);
        poll_now(&mut a);
        assert!(a.newer_on_disk.is_some());
        assert_eq!(a.project, mine, "nothing was half-read");
        a.project.bpm = 120.0;
        a.save_now();
        assert_eq!(fs::read_to_string(&path).unwrap(), newer);
    }

    #[test]
    fn another_file_starts_a_fresh_undo_history() {
        let dir = TempDir::new("history");
        let other = dir.0.join("other.aidaw.json");
        let mut a = app(song(&[]), None);
        let ctx = egui::Context::default();
        frame(&mut a, &ctx, vec![key(Key::Escape)], |a, _| {
            a.project.bpm = 140.0
        });
        // The demo replaces a song that has no file: Ctrl+Z brings it back.
        a.load_project(song(&[(60, 0.0)]), None, "");
        assert!(a.history.can_undo());
        // A song from a file: undoing into the old song would save it there.
        a.load_project(song(&[(62, 0.0)]), Some(other), "");
        assert!(!a.history.can_undo() && !a.history.can_redo());
        assert_eq!(notes(&a), [(62, 0.0)]);
    }

    #[test]
    fn a_file_that_fails_to_open_is_never_autosaved_over() {
        let dir = TempDir::new("first");
        let broken = dir.0.join("broken.aidaw.json");
        fs::write(&broken, "{ this is not a song").unwrap();
        let (p, path, _) = first_song(Some(broken.clone()));
        assert_eq!(path, None, "no file to save to");
        assert_eq!(p.tracks.len(), 2, "the starter song instead");

        let new = dir.0.join("new.aidaw.json");
        let (p, path, _) = first_song(Some(new.clone()));
        assert_eq!(path.as_deref(), Some(new.as_path()));
        let a = app(p, path);
        assert!(a.save_at.is_some(), "a new file is written right away");

        let good = dir.0.join("good.aidaw.json");
        let s = song(&[(64, 1.0), (60, 0.0)]);
        pio::save(&s, &good).unwrap();
        let (p, path, _) = first_song(Some(good.clone()));
        assert_eq!(path, Some(good));
        assert_eq!(
            p.clip("c1").unwrap().1.notes[0].pitch,
            60,
            "loaded in order"
        );
        assert_eq!(fs::read_to_string(&broken).unwrap(), "{ this is not a song");
    }

    #[test]
    fn lanes_the_ai_writes_open_and_undo_never_selects_another_point() {
        use aidaw_core::automation::{Lane, Param, Point};
        let dir = TempDir::new("lanes");
        let path = dir.0.join("song.aidaw.json");
        let mine = song(&[(60, 0.0)]);
        pio::save(&mine, &path).unwrap();
        let mut a = app(mine.clone(), Some(path.clone()));
        let ctx = egui::Context::default();

        // The AI fades the keys in over the first bar.
        let mut theirs = mine.clone();
        let mut fade = Lane::new(Param::Volume);
        fade.points = vec![Point::new(0.0, 0.0), Point::new(4.0, 0.8)];
        theirs.tracks[0].automation.push(fade);
        write_later(&path, &serde_json::to_string(&theirs).unwrap());
        poll_now(&mut a);
        assert!(a.lanes_open.contains("t1"), "the AI's lane opens by itself");
        assert!(!a.lanes_open.contains("t2") && !a.master_lanes_open);

        // A click on the line adds a point between the two and selects it.
        let lane = LaneKey::new(Some("t1"), Param::Volume);
        let points = |a: &App| lane.lane(&a.project).map_or(0, |l| l.points.len());
        frame(&mut a, &ctx, vec![button(false)], |a, _| {
            let i = crate::lanes::add_point(a, &lane, 2.0, 0.4).unwrap();
            a.selected_point = Some((lane.clone(), i));
            a.edit_focus = EditFocus::Automation;
        });
        assert_eq!(a.selected_point, Some((lane.clone(), 1)));
        // Index 1 is the AI's last point again after undo: not the one picked.
        a.undo();
        assert_eq!((points(&a), a.selected_point.clone()), (2, None));
        a.redo();
        assert_eq!(points(&a), 3);

        // Delete takes the selected point, and Ctrl+Z brings it back.
        frame(&mut a, &ctx, vec![button(false)], |a, _| {
            a.selected_point = Some((lane.clone(), 0));
        });
        frame(&mut a, &ctx, vec![key(Key::Delete)], |a, ctx| {
            a.edit_shortcuts(ctx)
        });
        assert_eq!((points(&a), a.selected_point.clone()), (2, None));
        assert_eq!(lane.lane(&a.project).unwrap().points[0].beat, 2.0);
        a.undo();
        assert_eq!(points(&a), 3);
        assert_eq!(lane.value_at(&a.project, 0.0), Some(0.0));
    }
}
