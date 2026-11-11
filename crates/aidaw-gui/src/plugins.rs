//! VST3 plugins in the editor: the list of installed ones (looked for in
//! the background), the song's running plugins, their own windows, and the
//! Plugins window, where folders are added and plugins looked for again.
//!
//! The song's plugins play inside the editor, so they add no latency.
//! Looking for plugins and exporting a song that uses them run in worker
//! processes (see [`aidaw_plugin::worker`]), where a broken plugin can only
//! fail that one job.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{mpsc, Arc, Mutex};

use aidaw_core::Project;
use aidaw_plugin::editor::Editor;
use aidaw_plugin::rack::{Key, LiveRack};
use aidaw_plugin::scan::{self, Kind, PluginInfo, ScanReport, Store};
use aidaw_plugin::worker::Worker;
use eframe::egui::{self, RichText};

use crate::app::App;
use crate::i18n::{tr, trf};
use crate::icons::{self, Icon};
use crate::theme;

/// The trademark notice that goes with the name.
pub const TRADEMARK: &str = "VST is a registered trademark of Steinberg Media Technologies GmbH.";

/// A look for plugins, running on a background thread.
struct Scan {
    /// (plugin files looked at, plugin files to look at)
    progress: Arc<Mutex<(usize, usize)>>,
    done: mpsc::Receiver<(Store, ScanReport)>,
}

pub struct Plugins {
    /// The installed plugins as last looked for, and the extra folders.
    pub store: Store,
    /// `store`'s plugins, instruments first, by name.
    list: Vec<PluginInfo>,
    /// The song's running plugins.
    pub rack: LiveRack,
    editors: BTreeMap<Key, Editor>,
    scan: Option<Scan>,
    /// The folders changed while a look was running: look again after it.
    again: bool,
    pub show_window: bool,
}

impl Plugins {
    pub fn new(store: Store, sample_rate: f64) -> Plugins {
        Plugins {
            list: store.plugins(),
            store,
            rack: LiveRack::new(sample_rate),
            editors: BTreeMap::new(),
            scan: None,
            again: false,
            show_window: false,
        }
    }

    /// The installed plugins of one kind, by name.
    pub fn installed(&self, kind: Kind) -> impl Iterator<Item = &PluginInfo> {
        self.list.iter().filter(move |p| p.kind == kind)
    }

    /// Look for new and updated plugins in the background (at every plugin
    /// again when `force`).
    pub fn rescan(&mut self, force: bool) {
        if self.scan.is_some() {
            self.again = true;
            return;
        }
        let Ok(worker) = Worker::current_exe() else {
            return;
        };
        let mut store = self.store.clone();
        let progress = Arc::new(Mutex::new((0, 0)));
        let (tx, rx) = mpsc::channel();
        let shared = progress.clone();
        std::thread::spawn(move || {
            let report = store.scan(&worker, force, &mut |done, total, _| {
                if let Ok(mut p) = shared.lock() {
                    *p = (done, total);
                }
            });
            let _ = tx.send((store, report));
        });
        self.scan = Some(Scan { progress, done: rx });
    }

    /// (plugin files looked at, plugin files to look at) while a look is
    /// running.
    pub fn scanning(&self) -> Option<(usize, usize)> {
        let s = self.scan.as_ref()?;
        Some(s.progress.lock().map(|p| *p).unwrap_or((0, 0)))
    }

    /// Take in a look that finished; true if one did.
    fn poll_scan(&mut self) -> bool {
        let Some(s) = &self.scan else {
            return false;
        };
        let (store, report) = match s.done.try_recv() {
            Ok(found) => found,
            Err(mpsc::TryRecvError::Empty) => return false,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.scan = None;
                return false;
            }
        };
        self.scan = None;
        let folders = std::mem::take(&mut self.store.folders);
        self.store = store;
        if self.store.folders != folders {
            // Changed while it ran: keep the new ones and look there too.
            self.store.folders = folders;
            let _ = self.store.save();
            self.again = true;
        }
        self.list = self.store.plugins();
        crate::sound::log(&format!(
            "plugins: {} found, {} plugin files looked at, {} could not be used",
            report.plugins,
            report.scanned,
            report.problems.len()
        ));
        if std::mem::take(&mut self.again) {
            self.rescan(false);
        }
        true
    }

    /// Look in `folder` too.
    pub fn add_folder(&mut self, folder: PathBuf) {
        let text = folder.display().to_string();
        if !self.store.folders.contains(&text) && !scan::standard_folders().contains(&folder) {
            self.store.folders.push(text);
            let _ = self.store.save();
        }
        self.rescan(false);
    }

    pub fn remove_folder(&mut self, folder: &str) {
        self.store.folders.retain(|f| f != folder);
        let _ = self.store.save();
        self.rescan(false);
    }

    /// Whether any plugin's own window is open.
    pub fn editors_open(&self) -> bool {
        self.editors.values().any(Editor::is_open)
    }

    /// Whether the plugin's own window is open.
    pub fn editor_open(&self, key: &Key) -> bool {
        self.editors.get(key).is_some_and(Editor::is_open)
    }

    /// Show the plugin's own window, or bring it to the front.
    pub fn open_editor(&mut self, project: &Project, key: &Key) -> Result<(), String> {
        if let Some(e) = self.editors.get(key).filter(|e| e.is_open()) {
            e.focus();
            return Ok(());
        }
        let slot = self
            .rack
            .slot(key)
            .cloned()
            .ok_or_else(|| tr("the plugin is not running").to_string())?;
        let editor = Editor::open(slot, &title(project, key)).map_err(|e| format!("{e:#}"))?;
        self.editors.insert(key.clone(), editor);
        Ok(())
    }

    /// Close every plugin window (another song was opened).
    pub fn close_editors(&mut self) {
        self.editors.clear();
    }

    /// Every frame: take in finished looks for plugins, tidy up the running
    /// plugins, close the windows of plugins that left the song and open
    /// the ones plugins asked for. True when the song's plugins should be
    /// set up again (new plugins were found, or one asked to be reloaded).
    pub fn tend(&mut self, project: &Project) -> bool {
        let found = self.poll_scan();
        if found {
            self.rack.retry_failed();
        }
        self.rack.maintain();
        self.editors
            .retain(|_, e| e.is_open() && !e.slot.is_retired());
        let asked: Vec<Key> = self
            .rack
            .slots()
            .filter(|(_, s)| s.plugin.edits.open_editor.swap(false, Ordering::AcqRel))
            .map(|(k, _)| k.clone())
            .collect();
        for key in asked {
            let _ = self.open_editor(project, &key);
        }
        found || self.rack.wants_rebuild()
    }
}

/// The title of a plugin's window: "Keys: Surge XT".
fn title(project: &Project, key: &Key) -> String {
    let Some(t) = project.track(&key.track) else {
        return "AIDAW".into();
    };
    let plugin = match &key.effect {
        None => t.instrument.plugin(),
        Some(fx) => t.effects.iter().find(|e| &e.id == fx).map(|e| e.plugin()),
    };
    match plugin {
        Some(p) => format!("{}: {}", t.name, p.name),
        None => t.name.clone(),
    }
}

/// The file name at the end of a path.
fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

/// A label that is cut short rather than make the window wider.
fn short(ui: &mut egui::Ui, text: RichText) -> egui::Response {
    ui.add(egui::Label::new(text).truncate())
}

/// The Plugins window: what is installed, where AIDAW looks, looking again.
pub fn show_window(ctx: &egui::Context, app: &mut App) {
    let mut open = app.plugins.show_window;
    let mut add_folder = false;
    let mut remove = None;
    let mut rescan = None;
    egui::Window::new(tr("VST3 plugins"))
        .id(egui::Id::new("plugins_window"))
        .open(&mut open)
        .default_width(460.0)
        .resizable(true)
        .show(ctx, |ui| {
            let p = &app.plugins;
            let instruments = p.installed(Kind::Instrument).count();
            let effects = p.installed(Kind::Effect).count();
            ui.label(
                RichText::new(trf(
                    "{} instruments and {} effects installed",
                    &[&instruments, &effects],
                ))
                .strong(),
            );
            ui.weak(tr(
                "Pick an instrument plugin under Sound, or add effects to any track below its sound.",
            ));
            ui.horizontal(|ui| match p.scanning() {
                Some((done, total)) => {
                    ui.spinner();
                    if total > 0 {
                        ui.label(trf("Looking at plugins… {} of {}", &[&done, &total]));
                    } else {
                        ui.label(tr("Looking for plugins…"));
                    }
                }
                None => {
                    if ui
                        .add(icons::button(Icon::Sync, tr("Look again")))
                        .on_hover_text(tr(
                            "Find plugins installed or updated since the last look",
                        ))
                        .clicked()
                    {
                        rescan = Some(false);
                    }
                    if ui
                        .button(tr("Check every plugin"))
                        .on_hover_text(tr(
                            "Load every plugin again to see what it is, not only new ones (slower)",
                        ))
                        .clicked()
                    {
                        rescan = Some(true);
                    }
                }
            });
            ui.separator();
            ui.label(RichText::new(tr("Where AIDAW looks")).strong());
            for f in scan::standard_folders() {
                ui.horizontal(|ui| {
                    icons::label(ui, Icon::Folder);
                    short(ui, RichText::new(f.display().to_string()).small());
                });
            }
            for f in &p.store.folders {
                ui.horizontal(|ui| {
                    icons::label(ui, Icon::Folder);
                    if ui
                        .add(icons::icon_button(Icon::Trash, 14.0).small())
                        .on_hover_text(tr("Stop looking in this folder"))
                        .clicked()
                    {
                        remove = Some(f.clone());
                    }
                    short(ui, RichText::new(f).small());
                });
            }
            if ui
                .add(icons::button(Icon::Plus, tr("Add a folder…")))
                .on_hover_text(tr(
                    "Plugins installed somewhere else? Pick the folder they are in.",
                ))
                .clicked()
            {
                add_folder = true;
            }
            ui.separator();
            egui::ScrollArea::vertical()
                .max_height(280.0)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    for (kind, heading) in [
                        (Kind::Instrument, tr("Instruments")),
                        (Kind::Effect, tr("Effects")),
                    ] {
                        ui.label(RichText::new(heading).strong());
                        let mut none = true;
                        for info in p.installed(kind) {
                            none = false;
                            let text = if info.vendor.is_empty() {
                                RichText::new(&info.name)
                            } else {
                                RichText::new(format!("{}  ·  {}", info.name, info.vendor))
                            };
                            short(ui, text).on_hover_text(&info.path);
                        }
                        if none {
                            ui.weak(tr("None found"));
                        }
                        ui.add_space(4.0);
                    }
                    let problems = p.store.problems();
                    if !problems.is_empty() {
                        egui::CollapsingHeader::new(trf(
                            "{} plugin files could not be used",
                            &[&problems.len()],
                        ))
                        .show(ui, |ui| {
                            for (path, why) in &problems {
                                short(ui, RichText::new(file_name(path)).strong())
                                    .on_hover_text(path);
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(why).small().color(theme::TEXT_DIM),
                                    )
                                    .wrap(),
                                );
                            }
                        });
                    }
                });
            ui.separator();
            ui.label(RichText::new(TRADEMARK).small().color(theme::TEXT_FAINT));
        });
    app.plugins.show_window = open;
    if add_folder {
        if let Some(dir) = rfd::FileDialog::new().pick_folder() {
            app.plugins.add_folder(dir);
        }
    }
    if let Some(f) = remove {
        app.plugins.remove_folder(&f);
    }
    if let Some(force) = rescan {
        app.plugins.rescan(force);
    }
}
