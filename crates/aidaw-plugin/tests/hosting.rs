//! Hosting the test plugin module (`examples/test_plugin.rs`) for real:
//! loading, playing notes, effects, saved settings, racks, and the worker
//! process. `cargo test` builds the example before these run.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use aidaw_core::edit::{apply_all, Op};
use aidaw_core::plugin::Blob;
use aidaw_core::rack::{NoteEvent, Transport};
use aidaw_core::render::{self, Sequencer};
use aidaw_core::{Instrument, Note, PluginRef, Project};
use aidaw_plugin::instance::{Plugin, Role, Setup};
use aidaw_plugin::module::Module;
use aidaw_plugin::rack::{self, Capture, Key, LiveRack};
use aidaw_plugin::scan::{self, Kind, Store};
use aidaw_plugin::worker::{Job, Worker};
use serde_json::{json, Value};

const SYNTH: &str = "41494441570000015445535453594E54";
const GAIN: &str = "414944415700000354455354"; // + "4741494E"

fn gain_uid() -> String {
    format!("{GAIN}4741494E")
}

const SR: f64 = 48_000.0;

/// A fresh folder for one test.
fn scratch(name: &str) -> PathBuf {
    static N: AtomicU32 = AtomicU32::new(0);
    let dir = std::env::temp_dir().join(format!(
        "aidaw-hosting-{}-{name}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The example plugin, built by cargo next to this test.
fn built_plugin() -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    let dir = exe.parent().unwrap().parent().unwrap().join("examples");
    let name = if cfg!(windows) {
        "test_plugin.dll"
    } else {
        "libtest_plugin.so"
    };
    let lib = dir.join(name);
    assert!(
        lib.exists(),
        "{} is missing: run `cargo test -p aidaw-plugin` (which builds the example)",
        lib.display()
    );
    lib
}

/// The test plugin installed as a `.vst3` bundle in `folder`.
fn install(folder: &Path) -> PathBuf {
    let bundle = folder.join("AIDAW Test.vst3");
    let (arch, ext) = if cfg!(windows) {
        ("x86_64-win", "vst3")
    } else {
        ("x86_64-linux", "so")
    };
    let dir = bundle.join("Contents").join(arch);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::copy(built_plugin(), dir.join(format!("AIDAW Test.{ext}"))).unwrap();
    bundle
}

/// One bundle shared by the tests in this process (a module is loaded once).
fn bundle() -> PathBuf {
    static BUNDLE: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    BUNDLE.get_or_init(|| install(&scratch("shared"))).clone()
}

fn setup() -> Setup {
    Setup {
        sample_rate: SR,
        offline: true,
    }
}

fn transport(frames: u64) -> Transport {
    Transport {
        sample_rate: SR,
        bpm: 120.0,
        beats_per_bar: 4,
        time_signature: (4, 4),
        beat: 0.0,
        playing: true,
        cycle: None,
        frames,
    }
}

fn on(offset: u32, pitch: u8) -> NoteEvent {
    NoteEvent {
        offset,
        pitch,
        velocity: 1.0,
        on: true,
        id: 1,
    }
}

fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0.0, |m, s| m.max(s.abs()))
}

fn f64s(bytes: &[u8]) -> Vec<f64> {
    bytes
        .chunks(8)
        .map(|c| f64::from_le_bytes(c.try_into().unwrap()))
        .collect()
}

fn synth_ref(path: &Path) -> PluginRef {
    let mut p = PluginRef::new("AIDAW Test Synth", SYNTH);
    p.path = path.display().to_string();
    p
}

fn gain_ref(path: &Path) -> PluginRef {
    let mut p = PluginRef::new("AIDAW Test Gain", gain_uid());
    p.path = path.display().to_string();
    p
}

/// A song with one plugin-synth track playing A4 for two beats, run through
/// the gain effect.
fn song(path: &Path) -> Project {
    let mut p = Project::new("plugins", 120.0);
    apply_all(
        &mut p,
        vec![
            Op::AddTrack {
                name: "Lead".into(),
                instrument: Some(Instrument::Vst3(synth_ref(path))),
                gain: Some(1.0),
                pan: None,
                reverb: None,
            },
            Op::AddClip {
                track_id: "t1".into(),
                start: 0.0,
                length: 4.0,
                name: None,
                notes: vec![Note {
                    pitch: 69,
                    start: 0.0,
                    duration: 2.0,
                    velocity: 127,
                }],
            },
            Op::AddEffect {
                track_id: "t1".into(),
                plugin: gain_ref(path),
                index: None,
                bypass: false,
            },
        ],
    )
    .unwrap();
    p
}

#[test]
fn the_module_lists_its_two_plugins() {
    let m = Module::load(&bundle()).unwrap();
    let classes = m.classes();
    assert_eq!(classes.len(), 2, "{classes:?}");
    let synth = classes.iter().find(|c| c.uid == SYNTH).unwrap();
    assert_eq!(synth.name, "AIDAW Test Synth");
    assert_eq!(synth.vendor, "AIDAW");
    assert!(synth.is_instrument());
    let gain = classes.iter().find(|c| c.uid == gain_uid()).unwrap();
    assert_eq!(gain.sub_categories, "Fx|Dynamics");
    assert!(!gain.is_instrument());
    // Loading again hands back the same module.
    assert!(std::sync::Arc::ptr_eq(
        &m,
        &Module::load(&bundle()).unwrap()
    ));
}

#[test]
fn the_synth_plays_notes_and_its_settings_round_trip() {
    let (synth, mut proc) = Plugin::load(&bundle(), SYNTH, setup(), None, None).unwrap();
    assert_eq!(synth.name, "AIDAW Test Synth");
    assert!(synth.layout.events, "it takes notes");
    assert_eq!(synth.layout.outputs, vec![2]);

    let params = synth.params();
    assert_eq!(params.len(), 2, "{params:?}");
    assert_eq!((params[0].name.as_str(), params[0].value), ("Level", 0.5));
    assert_eq!(params[0].display, "50");
    assert_eq!(params[1].options, ["Low", "Middle", "High"]);

    let mut l = vec![0.0f32; 512];
    let mut r = vec![0.0f32; 512];
    proc.run(
        Role::Instrument,
        512,
        &[on(100, 69)],
        &transport(0),
        [&mut l, &mut r],
    );
    assert_eq!(peak(&l[..100]), 0.0, "silent before the note");
    let p = peak(&l[100..]);
    assert!((p - 0.5).abs() < 0.01, "a sine at level 0.5: {p}");
    assert_eq!(l, r);

    synth.set_param(0, 0.25);
    l.fill(0.0);
    r.fill(0.0);
    proc.run(
        Role::Instrument,
        512,
        &[],
        &transport(512),
        [&mut l, &mut r],
    );
    assert!(
        (peak(&l) - 0.25).abs() < 0.01,
        "level now 0.25: {}",
        peak(&l)
    );

    let (state, controller) = synth.state().unwrap();
    // Level, octave, and whether the editor half's greeting arrived
    // (through the host's messages and connection).
    assert_eq!(f64s(&state), [0.25, 0.5, 1.0]);
    assert_eq!(f64s(controller.as_deref().unwrap()), [1.0]);
    drop(proc);
    drop(synth);

    let again = Plugin::load(
        &bundle(),
        SYNTH,
        setup(),
        Some(&state),
        controller.as_deref(),
    );
    let (again, _proc) = again.unwrap();
    assert_eq!(
        again.params()[0].value,
        0.25,
        "the editor half was told too"
    );
    assert_eq!(f64s(&again.state().unwrap().0)[0], 0.25);
}

#[test]
fn the_gain_effect_reshapes_the_bus() {
    let (gain, mut proc) = Plugin::load(&bundle(), &gain_uid(), setup(), None, None).unwrap();
    assert!(!gain.layout.events);
    let mut l = vec![0.5f32; 256];
    let mut r = vec![-0.5f32; 256];
    proc.run(Role::Effect, 256, &[], &transport(0), [&mut l, &mut r]);
    assert_eq!((l[10], r[10]), (0.5, -0.5), "0.5 is unity gain");
    gain.set_param(0, 1.0);
    proc.run(Role::Effect, 256, &[], &transport(256), [&mut l, &mut r]);
    assert_eq!((l[10], r[10]), (1.0, -1.0));
    assert_eq!(gain.params()[0].display, "2.00");
    // One object plays both halves: no separate editor settings.
    let (state, controller) = gain.state().unwrap();
    assert_eq!(f64s(&state), [1.0]);
    assert!(controller.is_none());
}

#[test]
fn a_song_renders_through_its_plugins() {
    let song = song(&bundle());
    let locate = |p: &PluginRef| Ok(PathBuf::from(&p.path));
    let (mut rack, problems) = rack::offline(&song, SR, &locate);
    assert!(problems.is_empty(), "{problems:?}");
    let out = render::render_with(&song, Some(&mut rack), SR as u32);
    let beat = SR as usize / 2;
    let loud = peak(&out[..2 * beat * 2]);
    assert!(loud > 0.2, "the synth is heard: {loud}");
    let after = peak(&out[2 * (2 * beat + 2000)..2 * 3 * beat]);
    assert!(after < 1e-3, "and stops with the note: {after}");

    // The effect's gain turned to 0 silences the track.
    let mut quiet = song.clone();
    quiet.tracks[0].effects[0]
        .plugin_mut()
        .params
        .insert("0".into(), 0.0);
    let (mut rack, _) = rack::offline(&quiet, SR, &locate);
    let out = render::render_with(&quiet, Some(&mut rack), SR as u32);
    assert!(peak(&out) < 1e-6, "{}", peak(&out));

    // Bypassed, it is left out.
    let mut bypassed = quiet.clone();
    bypassed.tracks[0].effects[0].bypass = true;
    let (mut rack, _) = rack::offline(&bypassed, SR, &locate);
    let out = render::render_with(&bypassed, Some(&mut rack), SR as u32);
    assert!(peak(&out) > 0.2);
}

#[test]
fn missing_plugins_are_reported_not_fatal() {
    let mut song = song(&bundle());
    song.tracks[0].effects[0].plugin_mut().path = "/nowhere/Gone.vst3".into();
    let locate = |p: &PluginRef| Ok(PathBuf::from(&p.path));
    let (mut rack, problems) = rack::offline(&song, SR, &locate);
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(problems[0].contains("AIDAW Test Gain"), "{problems:?}");
    let out = render::render_with(&song, Some(&mut rack), SR as u32);
    assert!(peak(&out) > 0.2, "the synth still plays");
}

#[test]
fn the_live_rack_keeps_plugins_running_across_edits() {
    let path = bundle();
    let locate = |p: &PluginRef| Ok(PathBuf::from(&p.path));
    let mut song = song(&path);
    let mut live = LiveRack::new(SR);
    let mut rack = live.build(&song, &locate);
    let synth = live.slot(&Key::instrument("t1")).unwrap().id;
    let fx = live.slot(&Key::effect("t1", "fx1")).unwrap().id;

    // Play a little so the plugins run.
    let mut seq = Sequencer::new(SR as u32);
    let mut out = vec![0.0f32; 2 * 4096];
    for chunk in out.chunks_mut(1024) {
        seq.process_with(&song, Some(&mut rack), chunk);
    }
    assert!(peak(&out) > 0.2);

    // A new snapshot with an edited note reuses both plugins.
    song.tracks[0].clips[0].notes[0].pitch = 72;
    let rack2 = live.build(&song, &locate);
    assert_eq!(live.slot(&Key::instrument("t1")).unwrap().id, synth);
    assert_eq!(live.slot(&Key::effect("t1", "fx1")).unwrap().id, fx);

    // The AI turns the gain down: the running effect hears it, and once it
    // has run a block its settings take the change in (folded, cleared).
    song.tracks[0].effects[0]
        .plugin_mut()
        .params
        .insert("0".into(), 0.25);
    let mut rack3 = live.build(&song, &locate);
    drop((rack, rack2));
    for chunk in out.chunks_mut(1024) {
        seq.process_with(&song, Some(&mut rack3), chunk);
    }
    assert!(
        live.capture(&song, Capture::Edited).is_empty(),
        "nobody turned a knob in its window"
    );
    let folded = live.capture(&song, Capture::Folded);
    assert_eq!(folded.len(), 1);
    assert!(rack::apply(&mut song, &folded));
    let fx_ref = song.tracks[0].effects[0].plugin();
    assert!(fx_ref.params.is_empty(), "folded into its settings");
    assert_eq!(f64s(fx_ref.state.as_ref().unwrap().as_bytes()), [0.25]);
    // The synth's settings were never read back: the song has none yet.
    let all = live.capture(&song, Capture::All);
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].key, Key::instrument("t1"));
    assert!(rack::apply(&mut song, &all));
    assert!(
        live.capture(&song, Capture::All).is_empty(),
        "nothing new the second time"
    );

    // Other settings in the song (undo, redo, a reload) go to the running
    // plugin, which keeps running.
    song.tracks[0].effects[0].plugin_mut().state = Some(Blob::new(1.0f64.to_le_bytes().to_vec()));
    let _rack4 = live.build(&song, &locate);
    let slot = live.slot(&Key::effect("t1", "fx1")).unwrap().clone();
    assert_eq!(slot.id, fx);
    assert_eq!(f64s(&slot.plugin.state().unwrap().0), [1.0]);

    // Back to before it had settings of its own: it starts afresh, with
    // its defaults.
    song.tracks[0].effects[0].plugin_mut().state = None;
    let _rack5 = live.build(&song, &locate);
    let fresh = live.slot(&Key::effect("t1", "fx1")).unwrap().clone();
    assert_ne!(fresh.id, fx);
    assert!(slot.is_retired());
    assert_eq!(f64s(&fresh.plugin.state().unwrap().0), [0.5]);

    // Another plugin in its place starts too.
    song.tracks[0].effects[0].kind = aidaw_core::plugin::EffectKind::Vst3(synth_ref(&path));
    let _rack6 = live.build(&song, &locate);
    assert_ne!(live.slot(&Key::effect("t1", "fx1")).unwrap().id, fresh.id);

    // A track that goes takes its plugins with it.
    song.tracks.clear();
    let _rack7 = live.build(&song, &locate);
    assert_eq!(live.slots().count(), 0);
}

/// The synth's own window opens in a window of AIDAW's, takes the size it
/// asks for, and goes away again; the effect has no window.
#[cfg(windows)]
#[test]
fn plugin_windows_open_resize_and_close() {
    use aidaw_plugin::editor::Editor;
    let locate = |p: &PluginRef| Ok(PathBuf::from(&p.path));
    let song = song(&bundle());
    let mut live = LiveRack::new(SR);
    let _rack = live.build(&song, &locate);
    let synth = live.slot(&Key::instrument("t1")).unwrap().clone();
    let editor = Editor::open(synth.clone(), "Keys: AIDAW Test Synth").unwrap();
    assert!(editor.is_open());
    assert!(synth.plugin.editor_open.load(Ordering::Acquire));
    assert_eq!(editor.size(), Some((400, 250)), "the size it asked for");
    drop(editor);
    assert!(!synth.plugin.editor_open.load(Ordering::Acquire));
    let gain = live.slot(&Key::effect("t1", "fx1")).unwrap().clone();
    let err = Editor::open(gain, "Keys: AIDAW Test Gain").err().unwrap();
    assert!(
        format!("{err:#}").contains("no window of its own"),
        "{err:#}"
    );
}

#[test]
fn plugins_that_fail_are_not_retried_every_edit() {
    let mut song = song(&bundle());
    song.tracks[0].effects[0].plugin_mut().path = "/nowhere/Gone.vst3".into();
    let tries = AtomicU32::new(0);
    let locate = |p: &PluginRef| {
        tries.fetch_add(1, Ordering::Relaxed);
        Ok(PathBuf::from(&p.path))
    };
    let mut live = LiveRack::new(SR);
    let _a = live.build(&song, &locate);
    let _b = live.build(&song, &locate);
    assert_eq!(
        tries.load(Ordering::Relaxed),
        2,
        "synth once, missing effect once"
    );
    let f = live.failure(&Key::effect("t1", "fx1")).unwrap();
    assert!(f.error.contains("does not exist"), "{}", f.error);
    live.retry_failed();
    let _c = live.build(&song, &locate);
    assert_eq!(tries.load(Ordering::Relaxed), 3);
}

/// Runs as the worker process when the tests start this program again.
#[test]
fn worker_entry() {
    if std::env::var_os("AIDAW_TEST_WORKER").is_none() {
        return;
    }
    if std::env::var_os("AIDAW_TEST_WORKER_CRASH").is_some() {
        std::process::abort();
    }
    std::process::exit(aidaw_plugin::worker::serve());
}

fn worker() -> Worker {
    Worker {
        program: std::env::current_exe().unwrap(),
        args: ["worker_entry", "--exact", "--nocapture", "--test-threads=1"]
            .map(Into::into)
            .to_vec(),
        env: vec![("AIDAW_TEST_WORKER".into(), "1".into())],
    }
}

#[test]
fn workers_scan_render_and_report_crashes() {
    let w = worker();
    let classes = scan::scan_module(&w, &bundle()).unwrap();
    assert_eq!(classes.len(), 2);

    let dir = scratch("render");
    let wav = dir.join("song.wav");
    let job = Job::Render {
        project: Box::new(song(&bundle())),
        out: wav.display().to_string(),
        sample_rate: 44_100,
    };
    let mut progress = Vec::new();
    let done = w
        .run(&job, std::time::Duration::from_secs(60), &mut |p| {
            progress.push(p)
        })
        .unwrap();
    assert!(done["seconds"].as_f64().unwrap() > 1.9, "{done}");
    assert!(std::fs::metadata(&wav).unwrap().len() > 100_000);
    assert_eq!(progress.last(), Some(&1.0));

    let params = w
        .run(
            &Job::Params {
                plugin: Box::new(synth_ref(&bundle())),
            },
            std::time::Duration::from_secs(30),
            &mut |_| {},
        )
        .unwrap();
    assert_eq!(params["params"][1]["name"], "Octave", "{params}");

    // Values by option name, by what the plugin shows (read by the gain
    // plugin itself; found along the knob for the synth's level), and as
    // knob positions.
    let set = |plugin: PluginRef, wanted: &[(&str, Value)]| {
        let wanted = wanted
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect();
        w.run(
            &Job::SetParams {
                plugin: Box::new(plugin),
                wanted,
            },
            std::time::Duration::from_secs(30),
            &mut |_| {},
        )
    };
    let done = set(
        synth_ref(&bundle()),
        &[
            ("octave", json!("high")),
            ("Level", json!("30")),
            ("0", json!(0.25)),
        ],
    )
    .unwrap();
    let got: Vec<(u64, f64, &str)> = done["set"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| {
            (
                s["id"].as_u64().unwrap(),
                s["value"].as_f64().unwrap(),
                s["display"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(got[0], (1, 1.0, "High"), "{done}");
    assert_eq!((got[1].0, got[1].2), (0, "30"), "{done}");
    assert_eq!(got[2], (0, 0.25, "25"), "{done}");
    let mut gain = synth_ref(&bundle());
    gain.uid = "4149444157000003544553544741494E".into();
    let done = set(gain.clone(), &[("gain", json!("1.5"))]).unwrap();
    assert_eq!(done["set"][0]["value"], 0.75, "{done}");
    let err = set(gain, &[("Volume", json!(0.5))]).unwrap_err();
    assert!(err.to_string().contains("no parameter"), "{err}");

    let err = w
        .run(
            &Job::Scan {
                path: "/nowhere/Gone.vst3".into(),
            },
            std::time::Duration::from_secs(30),
            &mut |_| {},
        )
        .unwrap_err();
    assert!(err.to_string().contains("does not exist"), "{err}");

    let mut crashing = worker();
    crashing
        .env
        .push(("AIDAW_TEST_WORKER_CRASH".into(), "1".into()));
    let err = scan::scan_module(&crashing, &bundle()).unwrap_err();
    assert!(err.to_string().contains("crashed"), "{err}");
}

#[test]
fn the_store_scans_once_and_finds_plugins_by_name() {
    let dir = scratch("store");
    let plugins = dir.join("plugins");
    install(&plugins.join("Vendor"));
    let file = dir.join("plugins.json");
    let mut store = Store::open(Some(file.clone()));
    store.folders = vec![plugins.display().to_string()];
    let w = worker();
    let mut seen = Vec::new();
    let report = store.scan(&w, false, &mut |done, total, _| seen.push((done, total)));
    assert!(report.problems.is_empty(), "{:?}", report.problems);
    let ours: Vec<_> = store
        .plugins()
        .into_iter()
        .filter(|p| p.vendor == "AIDAW")
        .collect();
    assert_eq!(ours.len(), 2, "{ours:?}");
    assert_eq!(ours[0].kind, Kind::Instrument);
    assert!(seen.contains(&(1, report.scanned)));

    // Saved, and the second scan takes it from the cache.
    let mut again = Store::open(Some(file));
    assert_eq!(again.folders, store.folders);
    let report = again.scan(&w, false, &mut |_, _, _| {});
    assert_eq!(report.scanned, 0);
    let synth = again.resolve("test synth", Some(Kind::Instrument)).unwrap();
    assert_eq!(synth.uid, SYNTH);
    let mut p = synth.to_ref();
    p.path.clear();
    assert!(again.locate(&p).unwrap().ends_with("AIDAW Test.vst3"));
    let _ = BTreeMap::<String, f64>::new();
}
