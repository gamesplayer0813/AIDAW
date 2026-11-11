//! Racks: the plugins of one project snapshot, the way the sequencer drives
//! them (see [`aidaw_core::rack`]).
//!
//! - [`offline`] loads fresh plugins for one render (a WAV export, an
//!   analysis) from the settings saved in the project.
//! - [`LiveRack`] keeps the editor's plugins running while the song is
//!   edited: each new snapshot gets a [`PluginRack`] that reuses the running
//!   plugins, so editing a note never reloads a synth or cuts its sound.
//!   Plugins that drop out of the song are torn down on the UI thread once
//!   the audio thread has handed back the last rack that used them.

use std::cell::UnsafeCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use aidaw_core::plugin::Blob;
use aidaw_core::rack::{NoteEvent, Rack, Transport, MAX_BLOCK};
use aidaw_core::{Instrument, PluginRef, Project};
use anyhow::Result;
use vst3::Steinberg::Vst::RestartFlags_;

use crate::instance::{Plugin, Processor, Role, Setup};

/// A running plugin shared by the racks of several snapshots.
pub struct Slot {
    /// Unique for the whole run; notes are tied to it.
    pub id: u64,
    pub plugin: Plugin,
    rt: UnsafeCell<Processor>,
    busy: AtomicBool,
    /// Blocks run so far.
    blocks: AtomicU64,
    /// Dropped from the song; its editor should close.
    retired: AtomicBool,
}

// SAFETY: `rt` is only touched by the thread that holds `busy`.
unsafe impl Sync for Slot {}

impl Slot {
    pub fn new(plugin: Plugin, processor: Processor) -> Slot {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Slot {
            id: NEXT.fetch_add(1, Ordering::Relaxed),
            plugin,
            rt: UnsafeCell::new(processor),
            busy: AtomicBool::new(false),
            blocks: AtomicU64::new(0),
            retired: AtomicBool::new(false),
        }
    }

    /// Run one block (see [`Processor::run`]). A second thread arriving
    /// while the first is still in here is turned away rather than let in.
    pub fn process(
        &self,
        role: Role,
        frames: usize,
        events: &[NoteEvent],
        transport: &Transport,
        bus: [&mut [f32]; 2],
    ) {
        if self.busy.swap(true, Ordering::Acquire) {
            return;
        }
        // SAFETY: `busy` gives this thread the processor alone.
        unsafe { (*self.rt.get()).run(role, frames, events, transport, bus) };
        self.blocks.fetch_add(1, Ordering::Release);
        self.busy.store(false, Ordering::Release);
    }

    /// Blocks run so far.
    pub fn blocks(&self) -> u64 {
        self.blocks.load(Ordering::Acquire)
    }

    pub fn is_retired(&self) -> bool {
        self.retired.load(Ordering::Acquire)
    }
}

/// One track's plugins in a rack.
struct TrackPlugins {
    instrument: Option<Arc<Slot>>,
    /// The effects that run (bypassed ones are left out).
    effects: Vec<Arc<Slot>>,
    bus: [Vec<f32>; 2],
}

impl TrackPlugins {
    fn routed(&self) -> bool {
        self.instrument.is_some() || !self.effects.is_empty()
    }
}

/// The plugins of one project snapshot, indexed like its tracks.
pub struct PluginRack {
    tracks: Vec<TrackPlugins>,
    tail: f64,
}

/// One track's plugins: its instrument (if a plugin) and its effects.
type Loaded = (Option<Arc<Slot>>, Vec<Arc<Slot>>);

impl PluginRack {
    fn new(tracks: Vec<Loaded>) -> PluginRack {
        let tail = tracks
            .iter()
            .flat_map(|(i, fx)| i.iter().chain(fx))
            .map(|s| s.plugin.tail_seconds())
            .fold(0.0, f64::max);
        PluginRack {
            tracks: tracks
                .into_iter()
                .map(|(instrument, effects)| {
                    let routed = instrument.is_some() || !effects.is_empty();
                    let len = if routed { MAX_BLOCK } else { 0 };
                    TrackPlugins {
                        instrument,
                        effects,
                        bus: [vec![0.0; len], vec![0.0; len]],
                    }
                })
                .collect(),
            tail,
        }
    }

    /// Whether any track has a plugin that loaded.
    pub fn is_empty(&self) -> bool {
        !self.tracks.iter().any(TrackPlugins::routed)
    }
}

impl Rack for PluginRack {
    fn instrument(&self, track: usize) -> Option<u64> {
        self.tracks.get(track)?.instrument.as_ref().map(|s| s.id)
    }

    fn routes(&self, track: usize) -> bool {
        self.tracks.get(track).is_some_and(TrackPlugins::routed)
    }

    fn bus(&mut self, track: usize) -> Option<[&mut [f32]; 2]> {
        let t = self.tracks.get_mut(track).filter(|t| t.routed())?;
        let [l, r] = &mut t.bus;
        Some([l.as_mut_slice(), r.as_mut_slice()])
    }

    fn process(
        &mut self,
        track: usize,
        frames: usize,
        events: &[NoteEvent],
        transport: &Transport,
    ) {
        let Some(t) = self.tracks.get_mut(track) else {
            return;
        };
        let [l, r] = &mut t.bus;
        if let Some(s) = &t.instrument {
            s.process(
                Role::Instrument,
                frames,
                events,
                transport,
                [&mut l[..], &mut r[..]],
            );
        }
        for s in &t.effects {
            s.process(
                Role::Effect,
                frames,
                &[],
                transport,
                [&mut l[..], &mut r[..]],
            );
        }
    }

    fn tail_seconds(&self) -> f64 {
        self.tail
    }
}

/// Finds where a plugin is installed.
pub type Locate<'a> = &'a dyn Fn(&PluginRef) -> Result<PathBuf>;

/// Load `p` with its saved settings and parameter overrides.
fn load(p: &PluginRef, path: &Path, setup: Setup) -> Result<Slot> {
    let (plugin, processor) = Plugin::load(
        path,
        &p.uid,
        setup,
        p.state.as_ref().map(Blob::as_bytes),
        p.controller_state.as_ref().map(Blob::as_bytes),
    )?;
    for (id, v) in p.param_values() {
        plugin.set_param(id, v);
    }
    plugin.edits.dirty.store(false, Ordering::Release);
    Ok(Slot::new(plugin, processor))
}

/// Fresh plugins for rendering `project` once. Plugins that fail to load
/// leave their track silent; why is in the returned problems.
pub fn offline(project: &Project, sample_rate: f64, locate: Locate) -> (PluginRack, Vec<String>) {
    let setup = Setup {
        sample_rate,
        offline: true,
    };
    let mut problems = Vec::new();
    let mut get = |track: &str, p: &PluginRef| -> Option<Arc<Slot>> {
        let loaded = locate(p).and_then(|path| load(p, &path, setup));
        match loaded {
            Ok(s) => Some(Arc::new(s)),
            Err(e) => {
                problems.push(format!("{track}: {} did not load: {e:#}", p.name));
                None
            }
        }
    };
    let tracks = project
        .tracks
        .iter()
        .map(|t| {
            // Nobody hears a muted track (or one another track's solo
            // silences): its plugins can stay unloaded.
            if !project.track_audible(t) {
                return (None, Vec::new());
            }
            let instrument = match &t.instrument {
                Instrument::Vst3(p) => get(&t.name, p),
                _ => None,
            };
            let effects = t
                .effects
                .iter()
                .filter(|e| !e.bypass)
                .filter_map(|e| get(&t.name, e.plugin()))
                .collect();
            (instrument, effects)
        })
        .collect();
    (PluginRack::new(tracks), problems)
}

/// Which plugin of the song: a track's instrument, or one of its effects.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Key {
    pub track: String,
    /// The effect's id, or `None` for the track's instrument.
    pub effect: Option<String>,
}

impl Key {
    pub fn instrument(track: &str) -> Key {
        Key {
            track: track.into(),
            effect: None,
        }
    }

    pub fn effect(track: &str, effect: &str) -> Key {
        Key {
            track: track.into(),
            effect: Some(effect.into()),
        }
    }
}

struct Live {
    slot: Arc<Slot>,
    uid: String,
    /// Parameter overrides already sent to it.
    applied: BTreeMap<String, f64>,
    /// Blocks it had run when they were sent: once it has run another, its
    /// audio half has them and its settings include them.
    applied_at: u64,
    /// When its editor's changes were first noticed, and its block count then.
    dirty_since: Option<(Instant, u64)>,
    /// The settings the song has for it, which it was started with, last
    /// given or last read back from it (its own, its editor half's).
    synced: (Option<Blob>, Option<Blob>),
}

impl Live {
    /// Bring the running plugin in line with the song's entry for it: the
    /// song's settings when they are not the ones it has (undo, redo and
    /// reloads bring older ones back), then its parameter overrides. False
    /// when only a fresh start can do that: the song went back to before
    /// the plugin had settings of its own.
    fn follow(&mut self, p: &PluginRef) -> bool {
        let plugin = &self.slot.plugin;
        let other = p.state != self.synced.0 || p.controller_state != self.synced.1;
        let dropped = self.applied.keys().any(|id| !p.params.contains_key(id));
        if other || dropped {
            let Some(state) = &p.state else {
                return false;
            };
            if let Err(e) = plugin.set_state(
                state.as_bytes(),
                p.controller_state.as_ref().map(Blob::as_bytes),
            ) {
                // It keeps what it has; don't ask again every edit.
                eprintln!("{e:#}");
            }
            self.synced = (p.state.clone(), p.controller_state.clone());
            // Its parameters are all as the settings have them now.
            self.applied.clear();
            self.dirty_since = None;
            plugin.edits.dirty.store(false, Ordering::Release);
        }
        let mut sent = false;
        for (id, v) in &p.params {
            if self.applied.get(id) != Some(v) {
                if let Ok(n) = id.parse() {
                    plugin.set_param(n, *v);
                    sent = true;
                }
                self.applied.insert(id.clone(), *v);
            }
        }
        if sent {
            self.applied_at = self.slot.blocks();
        }
        true
    }
}

/// Settings read back from a running plugin, for the song (see
/// [`LiveRack::capture`] and [`apply`]).
#[derive(Debug, Clone, PartialEq)]
pub struct Captured {
    pub key: Key,
    pub uid: String,
    pub state: Blob,
    pub controller_state: Option<Blob>,
    /// The plugin's parameter overrides are part of `state` now: they go.
    pub clear_params: bool,
}

/// Which plugins [`LiveRack::capture`] reads back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capture {
    /// Those changed in their own window, a moment after the last knob
    /// move: edits, worth an undo step each.
    Edited,
    /// Those that took on the parameter overrides the AI set: the overrides
    /// become part of their settings. The sound stays the same, so this is
    /// no edit.
    Folded,
    /// Every change not read back yet, right away (before saving or
    /// exporting the song).
    Pending,
    /// Every plugin (when the editor closes).
    All,
}

/// Put settings read back from plugins into `project`, where the same
/// plugins still are. Returns whether `project` changed.
pub fn apply(project: &mut Project, captured: &[Captured]) -> bool {
    let mut changed = false;
    for c in captured {
        let Some(t) = project.tracks.iter_mut().find(|t| t.id == c.key.track) else {
            continue;
        };
        let p = match &c.key.effect {
            None => match &mut t.instrument {
                Instrument::Vst3(p) => p,
                _ => continue,
            },
            Some(fx) => match t.effects.iter_mut().find(|e| &e.id == fx) {
                Some(e) => e.plugin_mut(),
                None => continue,
            },
        };
        if !p.uid.eq_ignore_ascii_case(&c.uid) {
            continue;
        }
        if p.state.as_ref() != Some(&c.state) || p.controller_state != c.controller_state {
            p.state = Some(c.state.clone());
            p.controller_state = c.controller_state.clone();
            changed = true;
        }
        if c.clear_params && !p.params.is_empty() {
            p.params.clear();
            changed = true;
        }
    }
    changed
}

/// Settings read from a plugin that is being restarted.
struct Carry {
    uid: String,
    state: Blob,
    controller: Option<Blob>,
}

/// Why a plugin of the song is not running.
#[derive(Debug, Clone, PartialEq)]
pub struct Failure {
    pub uid: String,
    pub error: String,
}

/// The editor's running plugins.
pub struct LiveRack {
    setup: Setup,
    live: HashMap<Key, Live>,
    /// Plugins out of the song, waiting for the audio thread to let go.
    zombies: Vec<Arc<Slot>>,
    failed: HashMap<Key, Failure>,
    /// Settings for plugins being restarted (new sample rate, new layout).
    carry: HashMap<Key, Carry>,
    /// Set when a plugin asked to be reloaded (its layout changed).
    rebuild: bool,
}

/// How long after the last knob move a plugin's settings are read back.
const CAPTURE_DELAY: Duration = Duration::from_millis(700);
/// Read them back anyway after this long, even if no audio ran meanwhile.
const CAPTURE_LATEST: Duration = Duration::from_secs(3);

impl LiveRack {
    pub fn new(sample_rate: f64) -> LiveRack {
        LiveRack {
            setup: Setup {
                sample_rate,
                offline: false,
            },
            live: HashMap::new(),
            zombies: Vec::new(),
            failed: HashMap::new(),
            carry: HashMap::new(),
            rebuild: false,
        }
    }

    pub fn sample_rate(&self) -> f64 {
        self.setup.sample_rate
    }

    /// Plugins run at the audio device's rate: a new rate restarts them
    /// all with their current settings (at the next build).
    pub fn set_sample_rate(&mut self, sample_rate: f64) {
        if sample_rate == self.setup.sample_rate || sample_rate.is_nan() || sample_rate <= 0.0 {
            return;
        }
        self.setup.sample_rate = sample_rate;
        let keys: Vec<Key> = self.live.keys().cloned().collect();
        for k in keys {
            self.restart(&k);
        }
        self.failed.clear();
    }

    /// Forget every plugin (another song was opened).
    pub fn reset(&mut self) {
        let keys: Vec<Key> = self.live.keys().cloned().collect();
        for k in keys {
            self.retire(&k);
        }
        self.failed.clear();
        self.carry.clear();
    }

    /// Try plugins that failed to load again at the next build (after a
    /// rescan, say).
    pub fn retry_failed(&mut self) {
        self.failed.clear();
    }

    /// Whether a plugin asked to be reloaded: build a new rack soon.
    pub fn wants_rebuild(&self) -> bool {
        self.rebuild
    }

    /// The rack for a new snapshot of the song, reusing running plugins.
    /// Plugins load here, on the calling (UI) thread.
    pub fn build(&mut self, project: &Project, locate: Locate) -> PluginRack {
        self.rebuild = false;
        let mut used = HashSet::new();
        let mut tracks = Vec::with_capacity(project.tracks.len());
        for t in &project.tracks {
            let instrument = match &t.instrument {
                Instrument::Vst3(p) => {
                    let key = Key::instrument(&t.id);
                    let s = self.ensure(&key, p, locate);
                    used.insert(key);
                    s
                }
                _ => None,
            };
            let mut effects = Vec::new();
            for e in &t.effects {
                let key = Key::effect(&t.id, &e.id);
                // Bypassed effects keep running out of the chain, so
                // switching them back on is instant and keeps their sound.
                if let Some(s) = self.ensure(&key, e.plugin(), locate) {
                    if !e.bypass {
                        effects.push(s);
                    }
                }
                used.insert(key);
            }
            tracks.push((instrument, effects));
        }
        let gone: Vec<Key> = self
            .live
            .keys()
            .filter(|k| !used.contains(*k))
            .cloned()
            .collect();
        for k in gone {
            self.retire(&k);
        }
        self.failed.retain(|k, _| used.contains(k));
        self.carry.retain(|k, _| used.contains(k));
        PluginRack::new(tracks)
    }

    fn retire(&mut self, key: &Key) {
        if let Some(l) = self.live.remove(key) {
            l.slot.retired.store(true, Ordering::Release);
            self.zombies.push(l.slot);
        }
    }

    /// Stop the plugin for `key`, keeping its settings for the next build
    /// to start it again with.
    fn restart(&mut self, key: &Key) {
        if let Some(l) = self.live.get(key) {
            if let Ok((state, controller)) = l.slot.plugin.state() {
                self.carry.insert(
                    key.clone(),
                    Carry {
                        uid: l.uid.clone(),
                        state: Blob::new(state),
                        controller: controller.map(Blob::new),
                    },
                );
            }
        }
        self.retire(key);
    }

    /// The running plugin for `key`, starting it if needed.
    fn ensure(&mut self, key: &Key, p: &PluginRef, locate: Locate) -> Option<Arc<Slot>> {
        if let Some(l) = self.live.get_mut(key) {
            if l.uid.eq_ignore_ascii_case(&p.uid) && l.follow(p) {
                return Some(l.slot.clone());
            }
            // Another plugin took this place, or this one starts afresh.
            self.retire(key);
        }
        if self
            .failed
            .get(key)
            .is_some_and(|f| f.uid.eq_ignore_ascii_case(&p.uid))
        {
            return None;
        }
        let carried = self
            .carry
            .remove(key)
            .filter(|c| c.uid.eq_ignore_ascii_case(&p.uid));
        let mut from = p.clone();
        // Settings it had that the song does not have yet are an edit.
        let mut unsaved = false;
        if let Some(c) = carried {
            unsaved = Some(&c.state) != p.state.as_ref() || c.controller != p.controller_state;
            from.state = Some(c.state);
            from.controller_state = c.controller;
        }
        match locate(p).and_then(|path| load(&from, &path, self.setup)) {
            Ok(slot) => {
                slot.plugin.edits.dirty.store(unsaved, Ordering::Release);
                let slot = Arc::new(slot);
                self.live.insert(
                    key.clone(),
                    Live {
                        slot: slot.clone(),
                        uid: p.uid.clone(),
                        applied: p.params.clone(),
                        applied_at: 0,
                        dirty_since: None,
                        synced: (p.state.clone(), p.controller_state.clone()),
                    },
                );
                Some(slot)
            }
            Err(e) => {
                self.failed.insert(
                    key.clone(),
                    Failure {
                        uid: p.uid.clone(),
                        error: format!("{e:#}"),
                    },
                );
                None
            }
        }
    }

    /// The running plugin for `key`.
    pub fn slot(&self, key: &Key) -> Option<&Arc<Slot>> {
        self.live.get(key).map(|l| &l.slot)
    }

    /// Why the plugin for `key` is not running, if it failed to load.
    pub fn failure(&self, key: &Key) -> Option<&Failure> {
        self.failed.get(key)
    }

    /// Every running plugin.
    pub fn slots(&self) -> impl Iterator<Item = (&Key, &Arc<Slot>)> {
        self.live.iter().map(|(k, l)| (k, &l.slot))
    }

    /// Housekeeping, every UI frame: tear down plugins nothing uses any
    /// more, pass the audio half's reports to the editor half, and restart
    /// plugins that asked to be reloaded.
    pub fn maintain(&mut self) {
        self.zombies.retain(|z| Arc::strong_count(z) > 1);
        let mut reload = Vec::new();
        for (k, l) in &self.live {
            l.slot.plugin.sync();
            let flags = l.slot.plugin.edits.restart.swap(0, Ordering::AcqRel);
            if flags & (RestartFlags_::kReloadComponent | RestartFlags_::kIoChanged) != 0 {
                reload.push(k.clone());
            }
        }
        for k in reload {
            self.rebuild = true;
            self.restart(&k);
        }
    }

    /// Read settings back from the running plugins of `project` (see
    /// [`Capture`] for which). Put them in the song with [`apply`].
    pub fn capture(&mut self, project: &Project, what: Capture) -> Vec<Captured> {
        let now = Instant::now();
        let mut out = Vec::new();
        for t in &project.tracks {
            if let Instrument::Vst3(p) = &t.instrument {
                out.extend(self.capture_one(Key::instrument(&t.id), p, what, now));
            }
            for e in &t.effects {
                out.extend(self.capture_one(Key::effect(&t.id, &e.id), e.plugin(), what, now));
            }
        }
        out
    }

    fn capture_one(
        &mut self,
        key: Key,
        p: &PluginRef,
        what: Capture,
        now: Instant,
    ) -> Option<Captured> {
        let l = self.live.get_mut(&key)?;
        if !l.uid.eq_ignore_ascii_case(&p.uid) {
            return None;
        }
        let blocks = l.slot.blocks();
        let edits = &l.slot.plugin.edits;
        if edits.dirty.swap(false, Ordering::AcqRel) && l.dirty_since.is_none() {
            l.dirty_since = Some((now, blocks));
        }
        let settled = l.dirty_since.is_some_and(|(t, b)| {
            let waited = now.duration_since(t);
            edits.gestures.load(Ordering::Acquire) == 0
                && waited >= CAPTURE_DELAY
                && (blocks > b + 1 || waited >= CAPTURE_LATEST)
        });
        let edited = l.dirty_since.is_some();
        let fold = !p.params.is_empty()
            && p.params.iter().all(|(id, v)| l.applied.get(id) == Some(v))
            && blocks > l.applied_at + 1;
        let wanted = match what {
            Capture::Edited => settled,
            // A change in its window comes first: it is read back with it.
            Capture::Folded => fold && !edited,
            Capture::Pending => edited || fold,
            Capture::All => true,
        };
        if !wanted {
            return None;
        }
        let (state, controller) = l.slot.plugin.state().ok()?;
        let state = Blob::new(state);
        let controller = controller.map(Blob::new);
        if what != Capture::Folded {
            l.dirty_since = None;
        }
        if fold {
            l.applied.clear();
        }
        l.synced = (Some(state.clone()), controller.clone());
        let same = p.state.as_ref() == Some(&state) && p.controller_state == controller;
        if same && !fold {
            return None;
        }
        Some(Captured {
            key,
            uid: p.uid.clone(),
            state,
            controller_state: controller,
            clear_params: fold,
        })
    }

    /// Whether a plugin's editor has changes not yet read back.
    pub fn has_unsaved(&self) -> bool {
        self.live
            .values()
            .any(|l| l.dirty_since.is_some() || l.slot.plugin.edits.dirty.load(Ordering::Acquire))
    }
}
