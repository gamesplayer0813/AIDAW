//! Application state, file sync with the AI, undo/redo and the top-level layout.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant, SystemTime};

use aidaw_core::edit::{self, Op};
use aidaw_core::{io as pio, render, Project};
use aidaw_engine::Engine;
use eframe::egui::{self, Color32, Key, KeyboardShortcut, Modifiers, Rect, RichText};

use crate::onboarding::{self, Tutorial, WelcomeChoice};
use crate::scale::Scale;
use crate::settings::Settings;
use crate::theme;
use aidaw_core::presets;

const SAVE_DEBOUNCE: Duration = Duration::from_millis(300);
const POLL_INTERVAL: Duration = Duration::from_millis(400);
const UNDO_LIMIT: usize = 200;

pub use crate::theme::track_color;

/// In-progress mouse drags, shared by the arrangement and the piano roll.
#[derive(Clone, Debug)]
pub enum Drag {
    MoveClip { clip_id: String, grab_offset: f64 },
    ResizeClip { clip_id: String },
    MoveNote { index: usize, grab_offset: f64 },
    ResizeNote { index: usize },
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

    undo: Vec<Project>,
    redo: Vec<Project>,
    /// Snapshot taken when an edit gesture began; pushed to `undo` when the mouse is released.
    pending_undo: Option<Project>,
    /// Undo (false) / redo (true) requested from a menu; applied before the next frame's snapshot.
    queued_history: Option<bool>,

    pub selected_track: Option<String>,
    pub selected_clip: Option<String>,
    pub selected_note: Option<usize>,
    pub drag: Option<Drag>,

    /// Grid size in beats.
    pub snap: f64,
    pub arrange_zoom: f32,
    pub roll_zoom: f32,
    pub roll_key_height: f32,
    pub roll_scroll_to_notes: bool,
    pub last_note_length: f64,
    pub playhead: f64,

    disk_mtime: Option<SystemTime>,
    last_poll: Instant,
    save_at: Option<Instant>,
    pub status: String,
    /// A WAV export running on a background thread.
    export: Option<ExportJob>,

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
    let mut p = Project::new("My Song", 110.0);
    let [keys, .., drums] = presets::all();
    let ops = vec![
        Op::AddTrack {
            name: "Piano".into(),
            instrument: Some(keys.instrument),
            gain: None,
            pan: None,
        },
        Op::AddTrack {
            name: "Drums".into(),
            instrument: Some(drums.instrument),
            gain: None,
            pan: None,
        },
        Op::AddClip {
            track_id: "t1".into(),
            start: 0.0,
            length: 16.0,
            name: Some("Melody".into()),
            notes: Vec::new(),
        },
        Op::AddClip {
            track_id: "t2".into(),
            start: 0.0,
            length: 16.0,
            name: Some("Beat".into()),
            notes: Vec::new(),
        },
    ];
    edit::apply_all(&mut p, ops).expect("static ops apply");
    p
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
        let (project, status) = match &path {
            Some(p) if p.exists() => match pio::load(p) {
                Ok(proj) => (proj, format!("Opened {}", p.display())),
                Err(e) => (starter_project(), format!("Failed to open: {e:#}")),
            },
            Some(p) => (
                starter_project(),
                format!("New project, will save to {}", p.display()),
            ),
            None => (
                starter_project(),
                "Welcome! Not sure where to start? Open ❓ Help › Tutorial.".into(),
            ),
        };
        let settings = Settings::load();
        let show_welcome = path.is_none() && settings.show_welcome;
        let opened = Engine::new(&project).map_err(|e| format!("{e:#}"));
        crate::sound::log_engine(&opened);
        let (engine, engine_error) = match opened {
            Ok(e) => (Some(e), None),
            Err(e) => (None, Some(e)),
        };
        let mut app = App {
            selected_track: project.tracks.first().map(|t| t.id.clone()),
            project,
            disk_mtime: path.as_deref().and_then(mtime),
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
            undo: Vec::new(),
            redo: Vec::new(),
            pending_undo: None,
            queued_history: None,
            selected_clip: None,
            selected_note: None,
            drag: None,
            snap: 0.25,
            arrange_zoom: 28.0,
            roll_zoom: 90.0,
            roll_key_height: 14.0,
            roll_scroll_to_notes: true,
            last_note_length: 1.0,
            playhead: 0.0,
            last_poll: Instant::now(),
            status,
            export: None,
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
        app.select_first_clip();
        app
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

    /// Replace the current song (the old one stays on the undo stack).
    pub fn load_project(&mut self, project: Project, path: Option<PathBuf>, status: &str) {
        let old = std::mem::replace(&mut self.project, project);
        self.push_undo(old);
        self.disk_mtime = path.as_deref().and_then(mtime);
        self.path = path;
        self.selected_clip = None;
        self.selected_note = None;
        self.stop();
        self.on_project_changed();
        self.save_at = None;
        self.select_first_clip();
        self.status = status.into();
    }

    /// Hear one note on a track's sound, even while stopped.
    pub fn preview_note(&self, track_index: usize, pitch: u8) {
        if let Some(e) = &self.engine {
            e.set_project(&self.project);
            e.preview(track_index, pitch);
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

    pub fn fit_pitch(&self, track_index: usize, pitch: u8) -> u8 {
        // Easy mode always keeps notes in key.
        if (self.scale_lock || !self.pro()) && self.scale_active(track_index) {
            self.scale.snap(self.scale_root, pitch)
        } else {
            pitch
        }
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
                    "No sound device. Opening the Sound window so you can pick one.".into();
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
                self.status = format!("Sound now plays through {}", e.device_name());
                e.test_tone();
                self.engine = Some(e);
                self.engine_error = None;
            }
            Err(err) => {
                self.status = "Couldn't open that sound device".into();
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
                    "The sound device isn't running. Pick another one in the Sound window.".into();
                self.show_sound = true;
            }
        }
    }

    pub fn stop(&mut self) {
        if let Some(e) = &self.engine {
            e.pause();
            e.seek(0.0);
        }
        self.playhead = 0.0;
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
                self.status = format!("Edit failed: {e:#}");
                None
            }
        }
    }

    fn on_project_changed(&mut self) {
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
                self.selected_note = None;
            }
            Some((_, clip)) => {
                if self.selected_note.is_some_and(|i| i >= clip.notes.len()) {
                    self.selected_note = None;
                }
            }
        }
    }

    fn push_undo(&mut self, snapshot: Project) {
        self.undo.push(snapshot);
        if self.undo.len() > UNDO_LIMIT {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    fn undo(&mut self) {
        if let Some(prev) = self.undo.pop() {
            self.redo.push(std::mem::replace(&mut self.project, prev));
            self.on_project_changed();
            self.ev_undos += 1;
            self.status = "Undone (Ctrl+Y to redo)".into();
        }
    }

    fn redo(&mut self) {
        if let Some(next) = self.redo.pop() {
            self.undo.push(std::mem::replace(&mut self.project, next));
            self.on_project_changed();
            self.status = "Redo".into();
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
            if let Ok(theirs) = pio::load(&path) {
                if theirs != self.project {
                    self.push_undo(theirs);
                    self.status =
                        "The AI changed the song while you were editing; kept your version (Ctrl+Z shows the AI's)"
                            .into();
                }
            }
        }
        match pio::save(&self.project, &path) {
            Ok(()) => self.disk_mtime = mtime(&path),
            Err(e) => self.status = format!("Save failed: {e:#}"),
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
        if Some(m) == self.disk_mtime {
            return;
        }
        // On a parse error (e.g. a non-atomic writer mid-write) just retry next poll.
        if let Ok(p) = pio::load(&path) {
            self.disk_mtime = Some(m);
            if p != self.project {
                // Keep the local version on the undo stack so AI edits can be reverted.
                let local = std::mem::replace(&mut self.project, p);
                self.push_undo(local);
                self.on_project_changed();
                // The file already holds this version; don't write it back.
                self.save_at = None;
                if self.selected_clip.is_none() {
                    self.select_first_clip();
                }
                self.status = "Reloaded external change (Ctrl+Z to revert)".into();
            }
        }
    }

    fn open_dialog(&mut self) {
        if let Some(p) = rfd::FileDialog::new()
            .add_filter("AIDAW project", &["json"])
            .pick_file()
        {
            match pio::load(&p) {
                Ok(proj) => {
                    let status = format!("Opened {}", p.display());
                    self.load_project(proj, Some(p), &status);
                }
                Err(e) => self.status = format!("Open failed: {e:#}"),
            }
        }
    }

    pub fn save_as_dialog(&mut self) {
        let name = format!("{}.{}", self.project.name, pio::EXTENSION);
        if let Some(p) = rfd::FileDialog::new()
            .add_filter("AIDAW project", &["json"])
            .set_file_name(name)
            .save_file()
        {
            self.status = format!("Saving to {} (AI tools can now use this file)", p.display());
            self.path = Some(p);
            self.save_now();
        }
    }

    fn export_wav_dialog(&mut self) {
        if self.export.is_some() {
            self.status = "An export is already running".into();
            return;
        }
        let name = format!("{}.wav", self.project.name);
        if let Some(p) = rfd::FileDialog::new()
            .add_filter("WAV audio", &["wav"])
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
            self.status = "Exporting audio… 0%".into();
        }
    }

    /// Show export progress and pick up the result when the thread is done.
    fn poll_export(&mut self) {
        let Some(job) = &self.export else { return };
        let outcome = match job.result.try_recv() {
            Ok(Ok(secs)) => Some(format!("Exported {secs:.1}s to {}", job.path.display())),
            Ok(Err(e)) => Some(format!("Export failed: {e}")),
            Err(mpsc::TryRecvError::Disconnected) => {
                Some("Export failed: the export stopped unexpectedly".into())
            }
            Err(mpsc::TryRecvError::Empty) => None,
        };
        match outcome {
            Some(status) => {
                self.status = status;
                self.export = None;
            }
            None => {
                let pct = job.progress.load(Ordering::Relaxed) / 10;
                self.status = format!("Exporting audio… {pct}%");
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
                self.status = "Saved".into();
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
                                ui.add_space(4.0);
                                let ai = theme::hero_button(
                                    ui,
                                    "✨ Ask AI",
                                    theme::ACCENT,
                                    egui::vec2(108.0, 34.0),
                                )
                                .on_hover_text(
                                    "Let an AI assistant (Claude, ChatGPT, …) write music into this song",
                                );
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
                    let failed = [
                        "Failed",
                        "Edit failed",
                        "Save failed",
                        "Open failed",
                        "Export failed",
                        "Couldn't",
                    ]
                    .iter()
                    .any(|p| self.status.starts_with(p));
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
                            ui.label(RichText::new(format!("⟳ {name}")).size(12.0).color(theme::TEXT_DIM))
                                .on_hover_text(
                                    "The project file the AI edits through aidaw-mcp; changes appear here live.",
                                );
                            ui.label(RichText::new("·").color(theme::TEXT_FAINT));
                        }
                        let text = if self.pro() {
                            format!(
                                "grid {} · key {} {}",
                                snap_label(self.snap),
                                crate::scale::ROOTS[self.scale_root as usize % 12],
                                self.scale.label()
                            )
                        } else {
                            "🐣 Easy mode · switch to 🔧 Pro (top left) for more controls".into()
                        };
                        ui.label(RichText::new(text).size(12.0).color(theme::TEXT_FAINT));
                    });
                });
            });
    }

    fn menus(&mut self, ui: &mut egui::Ui) {
        ui.menu_button("📁 File", |ui| {
            if ui.button("New song").clicked() {
                self.load_project(
                    starter_project(),
                    None,
                    "New song. Click in the piano roll to add notes!",
                );
                ui.close_menu();
            }
            if ui.button("Open…").clicked() {
                ui.close_menu();
                self.open_dialog();
            }
            if ui.button("Save As…  (Ctrl+S)").clicked() {
                ui.close_menu();
                self.save_as_dialog();
            }
            if ui
                .add_enabled(
                    self.export.is_none(),
                    egui::Button::new(if self.pro() {
                        "Export as WAV audio…"
                    } else {
                        "Save as audio file…"
                    }),
                )
                .on_hover_text("Make an audio file you can share or put in other apps")
                .clicked()
            {
                ui.close_menu();
                self.export_wav_dialog();
            }
        });
        let help = ui.menu_button("❓ Help", |ui| {
            if ui.button("🎓 Tutorial (2 minutes)").clicked() {
                self.start_tutorial();
                ui.close_menu();
            }
            if ui.button("✨ Let AI make music…").clicked() {
                self.show_ai_help = true;
                ui.close_menu();
            }
            if ui.button("🔊 Sound settings / no sound?").clicked() {
                self.show_sound = true;
                ui.close_menu();
            }
            if ui.button("⌨ Keyboard & mouse").clicked() {
                self.show_shortcuts = true;
                ui.close_menu();
            }
            if ui.button("👋 Welcome screen").clicked() {
                self.show_welcome = true;
                ui.close_menu();
            }
        });
        self.ui_rects.insert("help", help.response.rect);
        // Undo is the beginner's safety net, so it is a real button, not hidden in a menu.
        if ui
            .add_enabled(!self.undo.is_empty(), egui::Button::new("↩ Undo"))
            .on_hover_text("Take back the last change (Ctrl+Z). Works for AI changes too.")
            .clicked()
        {
            self.queued_history = Some(false);
        }
        if ui
            .add_enabled(!self.redo.is_empty(), egui::Button::new("↪ Redo"))
            .on_hover_text("Redo (Ctrl+Y)")
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
            ui.label(RichText::new("Grid").color(theme::TEXT_DIM))
                .on_hover_text(
                    "Notes and clips snap to this grid. 1/16 is a good default; 1/4 = one beat.",
                );
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
                .button(RichText::new("💾 Not saved. Click to save").color(theme::WARN))
                .on_hover_text("Save your song to a file. AI tools work on this file too.")
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
                egui::Button::new(RichText::new("⏹").size(16.0))
                    .corner_radius(17)
                    .min_size(egui::vec2(34.0, 34.0)),
            )
            .on_hover_text("Stop and go back to the start")
            .clicked()
        {
            self.stop();
        }
        let play = theme::hero_button(
            ui,
            if playing { "⏸  Pause" } else { "▶  Play" },
            if playing { theme::PAUSE } else { theme::PLAY },
            egui::vec2(104.0, 34.0),
        )
        .on_hover_text("Play or pause the song (Space bar)");
        if play.clicked() {
            self.toggle_play();
        }
        self.ui_rects.insert("play", play.rect);
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
                caption(ui, "BAR");
                let pos = if pro {
                    format!("{bar:>3}.{beat}")
                } else {
                    format!("{bar:>3}")
                };
                ui.label(lcd(pos))
                    .on_hover_text("Where the song is playing: bar number . beat");
                let (r, _) = ui.allocate_exact_size(egui::vec2(1.0, 22.0), egui::Sense::hover());
                ui.painter().rect_filled(r, 0.0, theme::BORDER);
                caption(ui, if pro { "BPM" } else { "SPEED" }).on_hover_text(if pro {
                    "Tempo in beats per minute. 70 = slow ballad, 110 = pop, 170 = drum & bass."
                } else {
                    "How fast the song plays (beats per minute).\n70 = slow and calm · 110 = pop · 140+ = fast and energetic."
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
                        .on_hover_text("Drag left/right or click to type")
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
                    if ui.add(small("−")).on_hover_text("Slower").clicked() {
                        bpm = (bpm - 5.0).round().max(20.0);
                    }
                    if ui.add(small("+")).on_hover_text("Faster").clicked() {
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
                crate::sound::meter(ui, peak).on_hover_text("Output level");
                let label = if self.pro() {
                    let name = e.device_name();
                    let short: String = name.chars().take(20).collect();
                    let ell = if short.len() < name.len() { "…" } else { "" };
                    format!("🔊 {short}{ell}")
                } else {
                    "🔊 Sound".to_string()
                };
                if ui
                    .button(label)
                    .on_hover_text("Sound settings: pick speakers, play a test beep")
                    .clicked()
                {
                    self.show_sound = true;
                }
            }
            None => {
                if ui
                    .add(
                        egui::Button::new(
                            RichText::new("🔇 No sound. Click to fix").color(theme::DANGER),
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
        .unwrap_or_else(|| format!("{s} beat"))
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_disk();
        self.poll_export();
        self.check_audio_alive();
        self.shortcuts(ctx);
        if let Some(e) = &self.engine {
            self.playhead = e.position();
        }

        let frame_start = self.project.clone();

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

        if self.project != frame_start {
            if self.pending_undo.is_none() {
                self.pending_undo = Some(frame_start);
            }
            self.on_project_changed();
        }
        // Coalesce a whole drag / slider gesture into one undo step.
        if self.pending_undo.is_some() && !ctx.input(|i| i.pointer.any_down()) {
            let snap = self.pending_undo.take().unwrap();
            if snap != self.project {
                self.push_undo(snap);
            }
        }
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
                        "Demo song loaded. Change anything you like!",
                    );
                    self.toggle_play();
                }
                WelcomeChoice::Blank => {
                    self.show_welcome = false;
                    self.load_project(
                        starter_project(),
                        None,
                        "New song. Click in the piano roll to add notes!",
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
        let base = presets::plain_name(preset.name);
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
            name: Some(format!("Part {}", parts + 1)),
            notes: Vec::new(),
        };
        if let Some(v) = self.run_op(op) {
            self.selected_track = Some(track_id.into());
            self.selected_clip = v["clip_id"].as_str().map(String::from);
            self.selected_note = None;
            self.roll_scroll_to_notes = true;
        }
    }

    pub fn delete_selected_clip(&mut self) {
        if let Some(id) = self.selected_clip.take() {
            self.run_op(Op::RemoveClip { clip_id: id });
            self.selected_note = None;
        }
    }
}
