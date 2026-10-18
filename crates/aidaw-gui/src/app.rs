//! Application state, file sync with the AI, undo/redo and the top-level layout.

use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

use aidaw_core::edit::{self, Op};
use aidaw_core::{io as pio, render, Instrument, Project};
use aidaw_engine::Engine;
use eframe::egui::{self, Color32, Key, KeyboardShortcut, Modifiers};

const SAVE_DEBOUNCE: Duration = Duration::from_millis(300);
const POLL_INTERVAL: Duration = Duration::from_millis(400);
const UNDO_LIMIT: usize = 200;

pub const TRACK_COLORS: [Color32; 8] = [
    Color32::from_rgb(0x4f, 0x9d, 0xff),
    Color32::from_rgb(0xff, 0x8a, 0x4f),
    Color32::from_rgb(0x6f, 0xd6, 0x7a),
    Color32::from_rgb(0xd0, 0x6f, 0xe0),
    Color32::from_rgb(0xff, 0xcf, 0x4f),
    Color32::from_rgb(0x4f, 0xd8, 0xd0),
    Color32::from_rgb(0xff, 0x6f, 0x8f),
    Color32::from_rgb(0xa0, 0xa8, 0xff),
];

pub fn track_color(index: usize) -> Color32 {
    TRACK_COLORS[index % TRACK_COLORS.len()]
}

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
    engine: Option<Engine>,
    engine_error: Option<String>,

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
}

fn starter_project() -> Project {
    let mut p = Project::new("Untitled", 120.0);
    let ops: Vec<Op> = serde_json::from_value(serde_json::json!([
        {"op": "add_track", "name": "Lead", "instrument": {"type": "synth", "waveform": "saw"}},
        {"op": "add_track", "name": "Drums", "instrument": {"type": "drums"}}
    ]))
    .expect("static ops");
    edit::apply_all(&mut p, ops).expect("static ops apply");
    p
}

fn mtime(path: &std::path::Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
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
                "Untitled project (use File > Save As to enable AI sync)".into(),
            ),
        };
        let (engine, engine_error) = match Engine::new(&project) {
            Ok(e) => (Some(e), None),
            Err(e) => (None, Some(format!("{e:#}"))),
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

    // ---- transport -------------------------------------------------------

    pub fn is_playing(&self) -> bool {
        self.engine.as_ref().is_some_and(|e| e.is_playing())
    }

    pub fn toggle_play(&mut self) {
        if let Some(e) = &self.engine {
            if e.is_playing() {
                e.pause();
            } else {
                e.play();
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

    pub fn snap_beat(&self, beat: f64) -> f64 {
        (beat / self.snap).round() * self.snap
    }

    pub fn snap_floor(&self, beat: f64) -> f64 {
        (beat / self.snap).floor() * self.snap
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
            self.status = "Undo".into();
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
        let Some(path) = &self.path else { return };
        match pio::save(&self.project, path) {
            Ok(()) => self.disk_mtime = mtime(path),
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
                    self.project = proj;
                    self.disk_mtime = mtime(&p);
                    self.status = format!("Opened {}", p.display());
                    self.path = Some(p);
                    self.selected_clip = None;
                    self.on_project_changed();
                    self.save_at = None;
                    self.select_first_clip();
                }
                Err(e) => self.status = format!("Open failed: {e:#}"),
            }
        }
    }

    fn save_as_dialog(&mut self) {
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
            self.status = match render::render_to_wav(&self.project, &p, sr) {
                Ok(secs) => format!("Exported {secs:.1}s to {}", p.display()),
                Err(e) => format!("Export failed: {e:#}"),
            };
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
        egui::TopBottomPanel::top("menu").show(ctx, |ui| {
            egui::menu::bar(ui, |ui| {
                ui.menu_button("File", |ui| {
                    if ui.button("New").clicked() {
                        self.project = starter_project();
                        self.path = None;
                        self.disk_mtime = None;
                        self.selected_clip = None;
                        self.on_project_changed();
                        ui.close_menu();
                    }
                    if ui.button("Open…").clicked() {
                        ui.close_menu();
                        self.open_dialog();
                    }
                    if ui.button("Save As…").clicked() {
                        ui.close_menu();
                        self.save_as_dialog();
                    }
                    if ui.button("Export WAV…").clicked() {
                        ui.close_menu();
                        self.export_wav_dialog();
                    }
                });
                ui.menu_button("Edit", |ui| {
                    if ui
                        .add_enabled(!self.undo.is_empty(), egui::Button::new("Undo  Ctrl+Z"))
                        .clicked()
                    {
                        self.queued_history = Some(false);
                        ui.close_menu();
                    }
                    if ui
                        .add_enabled(!self.redo.is_empty(), egui::Button::new("Redo  Ctrl+Y"))
                        .clicked()
                    {
                        self.queued_history = Some(true);
                        ui.close_menu();
                    }
                });
                ui.separator();
                let playing = self.is_playing();
                if ui
                    .button(if playing { "⏸ Pause" } else { "▶ Play" })
                    .clicked()
                {
                    self.toggle_play();
                }
                if ui.button("⏹ Stop").clicked() {
                    self.stop();
                }
                let bpb = self.project.beats_per_bar.max(1) as f64;
                let bar = (self.playhead / bpb).floor() as i64 + 1;
                let beat = (self.playhead % bpb).floor() as i64 + 1;
                ui.monospace(format!("{bar:>3}.{beat}"));
                ui.separator();
                ui.label("BPM");
                let mut bpm = self.project.bpm;
                if ui
                    .add(
                        egui::DragValue::new(&mut bpm)
                            .range(20.0..=400.0)
                            .speed(0.5)
                            .fixed_decimals(1),
                    )
                    .changed()
                {
                    self.project.bpm = bpm;
                }
                ui.label("Snap");
                egui::ComboBox::from_id_salt("snap")
                    .selected_text(snap_label(self.snap))
                    .show_ui(ui, |ui| {
                        for s in [1.0, 0.5, 0.25, 0.125, 1.0 / 3.0, 1.0 / 6.0] {
                            ui.selectable_value(&mut self.snap, s, snap_label(s));
                        }
                    });
                ui.separator();
                let sync = match &self.path {
                    Some(p) => {
                        format!("⟳ {}", p.file_name().and_then(|n| n.to_str()).unwrap_or(""))
                    }
                    None => "not saved".into(),
                };
                ui.label(sync).on_hover_text(
                    "The project file the AI edits through aidaw-mcp; changes appear here live.",
                );
                match (&self.engine, &self.engine_error) {
                    (Some(e), _) => {
                        ui.weak(format!("🔊 {} @ {} Hz", e.device_name(), e.sample_rate()))
                    }
                    (None, Some(err)) => {
                        ui.colored_label(Color32::LIGHT_RED, format!("No audio: {err}"))
                    }
                    _ => ui.label(""),
                };
            });
        });
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.weak(&self.status);
            });
        });
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
        self.shortcuts(ctx);
        if let Some(e) = &self.engine {
            self.playhead = e.position();
        }

        let frame_start = self.project.clone();

        self.top_bar(ctx);
        egui::SidePanel::left("inspector")
            .resizable(true)
            .default_width(270.0)
            .show(ctx, |ui| {
                crate::inspector::show(ui, self);
            });
        egui::TopBottomPanel::bottom("piano_roll")
            .resizable(true)
            .default_height(400.0)
            .show(ctx, |ui| {
                crate::piano_roll::show(ui, self);
            });
        egui::CentralPanel::default().show(ctx, |ui| {
            crate::arrange::show(ui, self);
        });

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
        if self.save_at.is_some_and(|t| Instant::now() >= t) {
            self.save_now();
        }

        if self.is_playing() {
            ctx.request_repaint();
        } else {
            ctx.request_repaint_after(POLL_INTERVAL);
        }
    }
}

impl App {
    pub fn add_track(&mut self, drums: bool) {
        let n = self.project.tracks.len() + 1;
        let (name, instrument) = if drums {
            (format!("Drums {n}"), Instrument::Drums)
        } else {
            (format!("Synth {n}"), Instrument::default_synth())
        };
        if let Some(v) = self.run_op(Op::AddTrack {
            name,
            instrument: Some(instrument),
            gain: None,
            pan: None,
        }) {
            self.selected_track = v["track_id"].as_str().map(String::from);
        }
    }

    pub fn add_clip(&mut self, track_id: &str, start: f64) {
        let length = self.project.beats_per_bar.max(1) as f64;
        let op = Op::AddClip {
            track_id: track_id.into(),
            start,
            length,
            name: None,
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
