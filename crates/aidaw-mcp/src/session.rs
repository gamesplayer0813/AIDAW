//! Tool execution. Arguments are normalised forgivingly before they reach
//! [`aidaw_core::edit::Op`]: tracks and clips by name or id, numbers as
//! strings, instrument presets by name, positions in bars, notes as melody
//! text. Errors say what was wrong and show what a valid call looks like.

use std::path::{Path, PathBuf};

use aidaw_core::edit::{self, Op};
use aidaw_core::music::{self, ChordStyle};
use aidaw_core::{
    io as pio, pitch, presets, render, DrumKit, Instrument, Note, Project, Synth, Waveform,
};
use anyhow::{anyhow, bail, Result};
use serde_json::{json, Map, Value};

use crate::tools::{self, Tool};

type Args = Map<String, Value>;

/// Keys whose string values are turned into numbers.
const NUMERIC_KEYS: &[&str] = &[
    "bpm",
    "beats_per_bar",
    "master_gain",
    "gain",
    "pan",
    "reverb",
    "reverb_size",
    "reverb_tone",
    "bar",
    "bars",
    "start",
    "length",
    "from",
    "to",
    "semitones",
    "duration",
    "velocity",
    "beats_per_chord",
    "octave",
    "steps_per_beat",
    "sample_rate",
    "attack",
    "decay",
    "sustain",
    "release",
    "cutoff",
    "resonance",
    "filter_env",
    "filter_decay",
    "unison",
    "detune",
    "sub",
    "noise",
    "fm",
    "fm_ratio",
    "fm_decay",
    "vibrato",
    "vibrato_rate",
    "pitch_env",
    "pitch_decay",
    "drive",
];
const BOOL_KEYS: &[&str] = &["mute", "solo", "overwrite"];
/// Accepted everywhere although not advertised in the schemas.
const HIDDEN_KEYS: &[&str] = &[
    "track_id",
    "track_name",
    "clip_id",
    "clip_name",
    "beat",
    "op",
];
const SYNTH_KEYS: &[&str] = &[
    "waveform",
    "attack",
    "decay",
    "sustain",
    "release",
    "cutoff",
    "resonance",
    "filter_env",
    "filter_decay",
    "unison",
    "detune",
    "sub",
    "noise",
    "fm",
    "fm_ratio",
    "fm_decay",
    "vibrato",
    "vibrato_rate",
    "pitch_env",
    "pitch_decay",
    "drive",
];

pub struct Session {
    pub path: Option<PathBuf>,
}

impl Session {
    pub fn new(path: Option<PathBuf>) -> Self {
        Session { path }
    }

    fn path(&self) -> Result<&Path> {
        self.path.as_deref().ok_or_else(|| {
            anyhow!(
                "no project is open. Call new_project first, e.g. {{\"path\": \"song.aidaw.json\", \"bpm\": 100}}, \
                 or open_project {{\"path\": \"existing.aidaw.json\"}}"
            )
        })
    }

    /// Run a tool and return its text result.
    pub fn call(&mut self, name: &str, args: Value) -> Result<String> {
        let tool = tools::find(name).ok_or_else(|| {
            anyhow!(
                "unknown tool {name:?}. Valid tools: {}. Call help for a guide.",
                tools::names()
            )
        })?;
        let mut args = match args {
            Value::Object(m) => m,
            Value::Null => Map::new(),
            // Some clients send the arguments as a JSON-encoded string.
            Value::String(s) if s.trim().is_empty() => Map::new(),
            Value::String(s) => match serde_json::from_str(&s) {
                Ok(Value::Object(m)) => m,
                _ => bail!("arguments must be a JSON object, e.g. {}", tool.example),
            },
            _ => bail!("arguments must be a JSON object, e.g. {}", tool.example),
        };
        coerce(&mut args);
        let warnings = unknown_keys(tool, &args);
        let mut out = self
            .run(tool, args)
            .map_err(|e| anyhow!("{e:#}\nExample {} call: {}", tool.name, tool.example))?;
        if !warnings.is_empty() {
            out.push_str(&format!("\nwarning: {}", warnings.join("; ")));
        }
        Ok(out)
    }

    fn run(&mut self, tool: &Tool, mut args: Args) -> Result<String> {
        match tool.name {
            "help" => Ok(tools::GUIDE.to_string()),
            "list_presets" => list_presets(&args),
            "new_project" => {
                let path = self.project_path_arg(str_arg(&args, "path")?)?;
                if path.exists() && args.get("overwrite").and_then(Value::as_bool) != Some(true) {
                    bail!(
                        "{} already exists; pass \"overwrite\": true to replace it, or use open_project",
                        path.display()
                    );
                }
                let name = args
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("Untitled");
                let bpm = args.get("bpm").and_then(Value::as_f64).unwrap_or(120.0);
                if !(20.0..=400.0).contains(&bpm) {
                    bail!("bpm must be within 20..=400 (got {bpm})");
                }
                let project = Project::new(name, bpm);
                pio::save(&project, &path)?;
                self.path = Some(path.clone());
                Ok(format!(
                    "created {} ({bpm} BPM, 4/4). Next: add_track, e.g. {{\"name\": \"Drums\", \"instrument\": \"drum_kit\"}}",
                    path.display()
                ))
            }
            "open_project" => {
                let path = self.resolve(str_arg(&args, "path")?);
                if !path.exists() {
                    bail!(
                        "{} does not exist. Use new_project to create a song, or give the full path of an existing .aidaw.json file",
                        path.display()
                    );
                }
                let project = pio::load(&path)?;
                self.path = Some(path.clone());
                Ok(format!("opened {}\n{}", path.display(), summary(&project)))
            }
            "get_project" => {
                let project = pio::load(self.path()?)?;
                let only = match args.get("track").and_then(Value::as_str) {
                    Some(q) => Some(find_track(&project, q)?.id.clone()),
                    None => None,
                };
                let detail = args
                    .get("detail")
                    .and_then(Value::as_str)
                    .unwrap_or(if args.get("full").and_then(Value::as_bool) == Some(true) {
                        "json"
                    } else {
                        "summary"
                    })
                    .to_ascii_lowercase();
                match detail.as_str() {
                    "summary" | "" => Ok(format!(
                        "{}\n(use detail \"notes\" to see every note, \"json\" for the raw file)",
                        overview(&project, only.as_deref(), false)
                    )),
                    "notes" => Ok(overview(&project, only.as_deref(), true)),
                    "json" | "full" => Ok(format!(
                        "{}\n\n{}",
                        summary(&project),
                        serde_json::to_string_pretty(&project)?
                    )),
                    other => {
                        bail!("unknown detail {other:?}; use \"summary\", \"notes\" or \"json\"")
                    }
                }
            }
            "render_wav" => {
                let path = self.path()?;
                let project = pio::load(path)?;
                let out = match args.get("output_path").and_then(Value::as_str) {
                    Some(p) => self.wav_path_arg(p)?,
                    None => default_wav_path(path),
                };
                let sr = match args.get("sample_rate") {
                    None => 44_100,
                    Some(v) => v
                        .as_u64()
                        .filter(|sr| (8_000..=192_000).contains(sr))
                        .ok_or_else(|| {
                            anyhow!("sample_rate must be a whole number from 8000 to 192000, e.g. 44100 (got {v})")
                        })? as u32,
                };
                let seconds = render::render_to_wav(&project, &out, sr)?;
                Ok(format!("rendered {seconds:.1}s to {}", out.display()))
            }
            "apply_ops" => {
                let ops = match args.remove("ops") {
                    Some(Value::Array(ops)) => ops,
                    Some(Value::String(s)) => match serde_json::from_str(&s) {
                        Ok(Value::Array(ops)) => ops,
                        _ => bail!("ops must be an array of objects"),
                    },
                    _ => bail!("ops must be an array of objects like {{\"op\": \"add_track\", \"name\": \"Bass\"}}"),
                };
                self.edit(|work| {
                    let mut results = Vec::with_capacity(ops.len());
                    for (i, item) in ops.clone().into_iter().enumerate() {
                        let mut item = match item {
                            Value::Object(m) => m,
                            _ => bail!("op #{i}: each op must be an object with an \"op\" field"),
                        };
                        let name = item
                            .remove("op")
                            .or_else(|| item.remove("tool"))
                            .and_then(|v| v.as_str().map(str::to_string))
                            .ok_or_else(|| {
                                anyhow!(
                                    "op #{i}: missing \"op\" (a tool name such as \"add_track\")"
                                )
                            })?;
                        let tool = tools::find(&name)
                            .filter(|t| is_edit_tool(t.name))
                            .ok_or_else(|| {
                                anyhow!(
                                    "op #{i}: {name:?} cannot be used in apply_ops; use one of: {}",
                                    edit_tool_names()
                                )
                            })?;
                        coerce(&mut item);
                        results.push(
                            apply_tool(work, tool, item)
                                .map_err(|e| anyhow!("op #{i} ({}): {e:#}", tool.name))?,
                        );
                    }
                    Ok(results)
                })
            }
            name if is_edit_tool(name) => {
                self.edit(|work| Ok(vec![apply_tool(work, tool, args.clone())?]))
            }
            other => bail!("tool {other} is not implemented"),
        }
    }

    /// Load, edit a working copy, save. Re-reading the file on every call
    /// picks up edits the human made in the GUI in the meantime; nothing is
    /// saved if any step fails.
    ///
    /// The GUI may save the same file between our load and our save (it
    /// autosaves a few hundred milliseconds after each edit). If the file's
    /// modification time changed meanwhile, the edit is replayed on the fresh
    /// version instead of overwriting the human's change.
    fn edit(&self, f: impl Fn(&mut Project) -> Result<Vec<Value>>) -> Result<String> {
        let path = self.path()?;
        for attempt in 0..EDIT_RETRIES {
            let before = pio::modified(path);
            let mut work = pio::load(path)?;
            let results = f(&mut work)?;
            if pio::modified(path) != before {
                // Somebody wrote the file while we were editing: start over.
                if attempt + 1 < EDIT_RETRIES {
                    std::thread::sleep(std::time::Duration::from_millis(50 * (attempt as u64 + 1)));
                }
                continue;
            }
            pio::save(&work, path)?;
            return Ok(format!(
                "{}\nProject now: {}",
                json!({ "results": results }),
                summary(&work)
            ));
        }
        bail!(
            "{} keeps changing while I try to save it (is something else writing it?); nothing was changed, please retry",
            path.display()
        )
    }

    /// Turn a path the model gave us into an absolute one. Relative paths are
    /// taken relative to the current project's folder (the model and the
    /// human both expect "song.wav" to land next to the song), or, with no
    /// project open, the user's Music folder; never the server's working
    /// directory, which under an MCP client is somewhere arbitrary.
    fn resolve(&self, given: &str) -> PathBuf {
        let given = PathBuf::from(given.trim());
        if given.is_absolute() {
            return given;
        }
        let base = self
            .path
            .as_ref()
            .and_then(|p| p.parent())
            .filter(|d| !d.as_os_str().is_empty())
            .map(Path::to_path_buf)
            .unwrap_or_else(default_dir);
        base.join(given)
    }

    /// Resolve the `path` of new_project: it always ends up as `*.aidaw.json`
    /// ("song" and "song.json" are completed), so the model can never make us
    /// overwrite a file that is not a project.
    fn project_path_arg(&self, given: &str) -> Result<PathBuf> {
        let mut path = self.resolve(given);
        if !pio::is_project_path(&path) {
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string();
            let stem = name
                .strip_suffix(".json")
                .or_else(|| name.strip_suffix(".JSON"))
                .unwrap_or(&name);
            if stem.is_empty() || stem.contains('.') {
                bail!(
                    "project files must be named *.{} (got {:?}); e.g. {{\"path\": \"song.aidaw.json\"}}",
                    pio::EXTENSION,
                    given
                );
            }
            path.set_file_name(format!("{stem}.{}", pio::EXTENSION));
        }
        Ok(path)
    }

    /// Resolve the `output_path` of render_wav: only `*.wav` may be written.
    fn wav_path_arg(&self, given: &str) -> Result<PathBuf> {
        let mut path = self.resolve(given);
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
        if name.is_empty() {
            bail!("output_path must name a .wav file, e.g. \"song.wav\"");
        }
        if name.to_ascii_lowercase().ends_with(".wav") {
            return Ok(path);
        }
        if name.contains('.') {
            bail!(
                "render_wav only writes .wav files (got {given:?}); e.g. {{\"output_path\": \"song.wav\"}}"
            );
        }
        path.set_file_name(format!("{name}.wav"));
        Ok(path)
    }
}

/// How often an edit is replayed when the file changes underneath it.
const EDIT_RETRIES: usize = 4;

/// Where relative paths go when no project is open: the user's Music folder
/// if there is one, else their home folder, else the working directory.
fn default_dir() -> PathBuf {
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from);
    if let Some(home) = home {
        let music = home.join("Music");
        return if music.is_dir() { music } else { home };
    }
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

const EDIT_TOOLS: &[&str] = &[
    "update_project",
    "add_track",
    "update_track",
    "remove_track",
    "add_clip",
    "update_clip",
    "remove_clip",
    "add_notes",
    "remove_notes",
    "transpose_clip",
    "add_chords",
    "add_drum_pattern",
    "add_melody",
];

fn is_edit_tool(name: &str) -> bool {
    EDIT_TOOLS.contains(&name)
}

fn edit_tool_names() -> String {
    EDIT_TOOLS.join(", ")
}

/// Apply one editing tool to `work`.
fn apply_tool(work: &mut Project, tool: &Tool, mut args: Args) -> Result<Value> {
    let bpb = work.beats_per_bar as f64;
    let mut warnings = Vec::new();
    for (key, lo, hi) in [
        ("gain", 0.0, 2.0),
        ("master_gain", 0.0, 2.0),
        ("pan", -1.0, 1.0),
        ("reverb", 0.0, 1.0),
        ("reverb_size", 0.0, 1.0),
        ("reverb_tone", 0.0, 1.0),
    ] {
        if let Some(x) = args.get(key).and_then(Value::as_f64) {
            if !(lo..=hi).contains(&x) {
                warnings.push(format!("{key} {x} is outside {lo}..{hi} and was clamped"));
            }
        }
    }
    match tool.name {
        "update_project" => {
            if let Some(v) = args.get("beats_per_bar") {
                if !v.as_u64().is_some_and(|b| (1..=32).contains(&b)) {
                    bail!("beats_per_bar must be a whole number from 1 to 32, e.g. 4 for 4/4 or 3 for 3/4 (got {v})");
                }
            }
        }
        "add_chords" | "add_drum_pattern" | "add_melody" => return musical(work, tool.name, args),
        "add_track" | "update_track" => {
            let existing = match tool.name {
                "update_track" => {
                    let id = resolve_track(work, &mut args)?;
                    Some(work.track(&id).expect("resolved").instrument.clone())
                }
                _ => None,
            };
            resolve_instrument(&mut args, existing)?;
            if tool.name == "add_track" && !args.contains_key("instrument") {
                // "Drums" gets a drum kit, "Bass" a bass, and so on.
                if let Some(p) = args
                    .get("name")
                    .and_then(Value::as_str)
                    .and_then(presets::find)
                {
                    args.insert("instrument".into(), serde_json::to_value(&p.instrument)?);
                }
            }
            if tool.name == "add_track" && !args.contains_key("reverb") {
                // The same starting reverb as picking that preset in the app.
                let inst = args
                    .get("instrument")
                    .and_then(|v| serde_json::from_value::<Instrument>(v.clone()).ok());
                if let Some(p) = inst.as_ref().and_then(presets::of) {
                    args.insert("reverb".into(), json!(p.category.reverb()));
                }
            }
        }
        "remove_track" => {
            resolve_track(work, &mut args)?;
        }
        "add_clip" => {
            let id = resolve_track(work, &mut args)?;
            let drums = work.track(&id).expect("resolved").instrument.is_drums();
            position(work, &mut args)?;
            if !args.contains_key("start") {
                args.insert("start".into(), json!(append_position(work, &id)));
            }
            bars_to_length(&mut args, bpb)?;
            normalize_notes(&mut args, drums, max_end(bpb))?;
            if !args.contains_key("length") {
                let end = note_end(args.get("notes"));
                args.insert("length".into(), json!(round_up_to_bar(end, bpb)));
            }
            let num = |k: &str| args.get(k).and_then(Value::as_f64).unwrap_or(0.0);
            check_end(num("start") + num("length"), bpb)?;
        }
        "update_clip" => {
            let id = resolve_clip(work, &mut args)?;
            position(work, &mut args)?;
            bars_to_length(&mut args, bpb)?;
            let clip = work.clip(&id).expect("resolved").1;
            let get = |k: &str, d: f64| args.get(k).and_then(Value::as_f64).unwrap_or(d);
            check_end(get("start", clip.start) + get("length", clip.length), bpb)?;
        }
        "remove_clip" => {
            resolve_clip(work, &mut args)?;
        }
        "transpose_clip" => {
            let id = resolve_clip(work, &mut args)?;
            let semis = match args.get("semitones") {
                Some(v) => v
                    .as_i64()
                    .filter(|s| s.abs() <= 48)
                    .ok_or_else(|| anyhow!("semitones must be a whole number from -48 to 48 (12 = one octave up), got {v}"))?,
                None => bail!("missing \"semitones\", e.g. 12 for one octave up or -5"),
            };
            let notes = &work.clip(&id).expect("resolved").1.notes;
            if let (Some(lo), Some(hi)) = (
                notes.iter().map(|n| n.pitch).min(),
                notes.iter().map(|n| n.pitch).max(),
            ) {
                if lo as i64 + semis < 0 || hi as i64 + semis > 127 {
                    bail!(
                        "transposing by {semis} would move notes outside the MIDI range 0-127 (clip spans {}..{}); use a smaller shift",
                        pitch::name(lo),
                        pitch::name(hi)
                    );
                }
            }
        }
        "add_notes" => {
            let id = resolve_clip(work, &mut args)?;
            let drums = work.clip(&id).expect("resolved").0.instrument.is_drums();
            normalize_notes(&mut args, drums, max_end(bpb))?;
        }
        "remove_notes" => {
            resolve_clip(work, &mut args)?;
            if let Some(Value::String(s)) = args.get("pitch") {
                if let Some(p) = pitch::parse(s).or_else(|| music::drum_note(s)) {
                    args.insert("pitch".into(), json!(p));
                }
            }
        }
        _ => {}
    }
    let created_instrument = (tool.name == "add_track").then(|| args.get("instrument").cloned());
    for key in ["bar", "bars", "beat", "track_name", "clip_name"] {
        args.remove(key);
    }
    args.insert("op".into(), Value::String(tool.name.into()));
    let op: Op = serde_json::from_value(Value::Object(args))
        .map_err(|e| anyhow!("invalid arguments for {}: {e}", tool.name))?;
    let mut result = edit::apply(work, op)?;
    if let Some(inst) = created_instrument {
        let inst: Instrument = match inst {
            Some(v) => serde_json::from_value(v)?,
            None => Instrument::default_synth(),
        };
        result["instrument"] = json!(instrument_label(&inst));
    }
    if !warnings.is_empty() {
        result["warnings"] = json!(warnings);
    }
    Ok(result)
}

/// Clips may not end after this beat (4096 bars).
fn max_end(bpb: f64) -> f64 {
    4096.0 * bpb
}

fn check_end(end: f64, bpb: f64) -> Result<()> {
    if end > max_end(bpb) + 1e-9 {
        bail!(
            "that would end at beat {} (bar {}); songs can be at most 4096 bars long",
            fmt_num(end),
            bar_of(end, bpb)
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// High-level musical tools

fn musical(work: &mut Project, tool: &str, mut args: Args) -> Result<Value> {
    let bpb = work.beats_per_bar as f64;
    let velocity = |args: &Args, default: u8| -> Result<u8> {
        match args.get("velocity") {
            None => Ok(default),
            Some(v) => v
                .as_u64()
                .filter(|v| (1..=127).contains(v))
                .map(|v| v as u8)
                .ok_or_else(|| anyhow!("velocity must be a whole number 1-127 (got {v})")),
        }
    };
    let mut warnings = Vec::new();

    // Which track (and maybe clip) do the notes go to?
    let default_preset = match tool {
        "add_chords" => "soft_keys",
        "add_drum_pattern" => "drum_kit",
        _ => "bright_lead",
    };
    let explicit_clip = has_any(&args, &["clip", "clip_id", "clip_name"]);
    let clip_id = if explicit_clip {
        Some(resolve_clip(work, &mut args)?)
    } else {
        None
    };
    let mut created_track = None;
    let track_id = match &clip_id {
        Some(cid) => work.clip(cid).expect("resolved").0.id.clone(),
        None => {
            let query = track_query(&mut args)
                .or_else(|| (tool == "add_drum_pattern").then(|| "Drums".to_string()))
                .ok_or_else(|| {
                    anyhow!(
                        "missing \"track\": the name or id of the track to write on, e.g. \"Piano\". {}",
                        track_list(work)
                    )
                })?;
            match find_track(work, &query) {
                Ok(t) => t.id.clone(),
                Err(not_found) => {
                    // Only create tracks whose purpose is clear: a preset-like
                    // name ("Drums", "Bass", "Piano") or an explicit instrument.
                    let preset = match args.get("instrument") {
                        Some(Value::String(s)) => presets::find(s).ok_or_else(|| {
                            anyhow!("unknown instrument {s:?}. Valid presets: {}", preset_ids())
                        })?,
                        Some(other) => bail!(
                            "instrument must be a preset id string such as {:?}; valid presets: {} (got {other})",
                            default_preset,
                            preset_ids()
                        ),
                        None => presets::find(&query).ok_or_else(|| {
                            anyhow!(
                                "{not_found:#}. Or pass \"instrument\" (e.g. \"{default_preset}\") to create track {query:?} here"
                            )
                        })?,
                    };
                    let r = edit::apply(
                        work,
                        Op::AddTrack {
                            name: query.clone(),
                            instrument: Some(preset.instrument.clone()),
                            gain: None,
                            pan: None,
                            reverb: Some(preset.category.reverb()),
                        },
                    )?;
                    let id = r["track_id"].as_str().expect("track id").to_string();
                    created_track = Some(format!(
                        "created new track {id} \"{query}\" ({})",
                        preset.id
                    ));
                    id
                }
            }
        }
    };
    let is_drums = work.track(&track_id).expect("track").instrument.is_drums();

    let (notes, span, clip_name, what) = match tool {
        "add_chords" => {
            let text = str_arg(&args, "chords")?;
            let per = match args.get("beats_per_chord") {
                Some(v) => v
                    .as_f64()
                    .filter(|b| *b > 0.0 && *b <= max_end(bpb))
                    .ok_or_else(|| {
                        anyhow!(
                            "beats_per_chord must be a number > 0 and at most {} (4096 bars), e.g. 2 or 4 (got {v})",
                            max_end(bpb)
                        )
                    })?,
                None => bpb,
            };
            let prog = music::parse_progression(text, per)?;
            let style = ChordStyle::parse(args.get("style").and_then(Value::as_str).unwrap_or(""))?;
            let octave = match args.get("octave") {
                None => 4,
                Some(v) => v.as_i64().filter(|o| (1..=7).contains(o)).ok_or_else(|| {
                    anyhow!("octave must be a whole number 1-7 (4 = around middle C), got {v}")
                })?,
            };
            let span: f64 = prog.iter().map(|c| c.1).sum();
            let notes = music::chords_to_notes(&prog, style, octave as i32, velocity(&args, 90)?);
            let n = prog.iter().filter(|c| c.0.is_some()).count();
            (notes, span, "Chords", format!("{n} chord(s)"))
        }
        "add_drum_pattern" => {
            let lanes = drum_lanes(&mut args)?;
            let spb = match args.get("steps_per_beat") {
                Some(v) => v
                    .as_u64()
                    .filter(|s| (1..=16).contains(s))
                    .ok_or_else(|| anyhow!("steps_per_beat must be 1-16 (4 = sixteenth notes)"))?,
                None => 4,
            } as usize;
            let steps_per_bar = spb * work.beats_per_bar as usize;
            let longest = lanes.iter().map(|(_, s)| step_count(s)).max().unwrap_or(0);
            let bars = match args.get("bars") {
                Some(v) => v
                    .as_u64()
                    .filter(|b| (1..=512).contains(b))
                    .ok_or_else(|| anyhow!("bars must be a whole number >= 1"))?
                    as usize,
                None => longest.div_ceil(steps_per_bar).max(1),
            };
            if longest > steps_per_bar && longest % steps_per_bar != 0 {
                warnings.push(format!(
                    "a pattern has {longest} steps, not a multiple of {steps_per_bar} (one bar); it was repeated anyway"
                ));
            }
            let notes = music::drum_pattern_to_notes(
                &lanes,
                bars * steps_per_bar,
                1.0 / spb as f64,
                velocity(&args, 100)?,
            )?;
            let span = bars as f64 * bpb;
            (notes, span, "Beat", format!("{bars} bar(s) of drums"))
        }
        _ => {
            // add_melody: a string, or note objects like add_notes takes.
            let (notes, span) = match args.get("notes").or_else(|| args.get("melody")) {
                Some(Value::String(s)) => music::parse_melody(s, velocity(&args, 100)?, is_drums)?,
                Some(v @ Value::Array(_)) => {
                    let mut tmp = Args::new();
                    tmp.insert("notes".into(), v.clone());
                    normalize_notes(&mut tmp, is_drums, max_end(bpb))?;
                    let notes: Vec<Note> = serde_json::from_value(tmp.remove("notes").unwrap())?;
                    let end = notes
                        .iter()
                        .map(|n| n.start + n.duration)
                        .fold(0.0, f64::max);
                    (notes, end)
                }
                _ => bail!(
                    "missing \"notes\": a melody string like \"{}\"",
                    music::MELODY_EXAMPLE
                ),
            };
            (notes, span, "Melody", "a melody".to_string())
        }
    };

    match (tool == "add_drum_pattern", is_drums) {
        (true, false) => warnings.push(format!(
            "track {track_id} is not a drum kit, so these notes will play as pitches; \
             use update_track {{\"track\": \"{track_id}\", \"instrument\": \"drum_kit\"}} to hear drums"
        )),
        (false, true) => warnings.push(format!(
            "track {track_id} is a drum kit, so pitches play as drum sounds; use a melodic track instead"
        )),
        _ => {}
    }

    position(work, &mut args)?;
    let start = match (args.get("start").and_then(Value::as_f64), &clip_id) {
        (Some(s), _) => s,
        (None, Some(cid)) => work.clip(cid).expect("clip").1.start,
        (None, None) => append_position(work, &track_id),
    };
    check_end(start + span, bpb)?;
    let target = clip_id.or_else(|| {
        work.track(&track_id)?
            .clips
            .iter()
            .find(|c| c.start <= start + 1e-9 && start < c.end() - 1e-9)
            .map(|c| c.id.clone())
    });

    let added = notes.len();
    let clip_id = match target {
        Some(cid) => {
            let clip = work.clip(&cid).expect("clip").1;
            let offset = start - clip.start;
            if offset < -1e-9 {
                bail!(
                    "position beat {start} is before clip {cid}, which starts at beat {} (bar {})",
                    clip.start,
                    bar_of(clip.start, bpb)
                );
            }
            let needed = round_up_to_bar(offset + span, bpb);
            check_end(clip.start + needed, bpb)?;
            let grow = needed > clip.length + 1e-9;
            let shifted = notes
                .into_iter()
                .map(|n| Note {
                    start: n.start + offset,
                    ..n
                })
                .collect();
            edit::apply(
                work,
                Op::AddNotes {
                    clip_id: cid.clone(),
                    notes: shifted,
                },
            )?;
            if grow {
                edit::apply(
                    work,
                    Op::UpdateClip {
                        clip_id: cid.clone(),
                        name: None,
                        start: None,
                        length: Some(needed),
                    },
                )?;
            }
            cid
        }
        None => {
            let r = edit::apply(
                work,
                Op::AddClip {
                    track_id: track_id.clone(),
                    start,
                    length: round_up_to_bar(span, bpb),
                    name: Some(clip_name.to_string()),
                    notes,
                },
            )?;
            r["clip_id"].as_str().expect("clip id").to_string()
        }
    };
    let track = work.track(&track_id).expect("track");
    let mut message = format!(
        "added {what} ({added} notes) to clip {clip_id} on track {track_id} \"{}\", {}",
        track.name,
        bar_range(start, start + span, bpb)
    );
    if let Some(c) = &created_track {
        message = format!("{c}; {message}");
    }
    let mut result =
        json!({"message": message, "track_id": track_id, "clip_id": clip_id, "notes_added": added});
    if let Some(c) = created_track {
        result["created"] = json!(c);
    }
    if !warnings.is_empty() {
        result["warnings"] = json!(warnings);
    }
    Ok(result)
}

/// Collect drum lanes from `pattern` (an object, a JSON string, or lines like
/// "kick: x...") and from top-level keys that are drum names.
fn drum_lanes(args: &mut Args) -> Result<Vec<(String, String)>> {
    let mut lanes = Vec::new();
    let push_obj = |m: &Args, lanes: &mut Vec<(String, String)>| -> Result<()> {
        for (k, v) in m {
            let steps = v.as_str().ok_or_else(|| {
                anyhow!("pattern for {k:?} must be a string of steps like \"x...x...x...x...\" (got {v})")
            })?;
            lanes.push((k.clone(), steps.to_string()));
        }
        Ok(())
    };
    match args.remove("pattern").or_else(|| args.remove("drums")) {
        Some(Value::Object(m)) => push_obj(&m, &mut lanes)?,
        Some(Value::String(s)) => match serde_json::from_str::<Value>(&s) {
            Ok(Value::Object(m)) => push_obj(&m, &mut lanes)?,
            _ => {
                for line in s.split(['\n', ';', ',']).filter(|l| !l.trim().is_empty()) {
                    let (name, steps) = line.split_once([':', '=']).ok_or_else(|| {
                        anyhow!("could not read pattern line {line:?}; write \"kick: x...x...x...x...\"")
                    })?;
                    lanes.push((name.trim().to_string(), steps.trim().to_string()));
                }
            }
        },
        Some(other) => {
            bail!("pattern must be an object like {{\"kick\": \"x...x...x...x...\"}} (got {other})")
        }
        None => {}
    }
    // Lanes given at the top level: {"track": "Drums", "kick": "x..."}.
    let tool = tools::find("add_drum_pattern").expect("tool");
    let top: Vec<String> = args
        .keys()
        .filter(|k| {
            !tool.params.iter().any(|p| p.name == k.as_str()) && music::drum_note(k).is_some()
        })
        .cloned()
        .collect();
    for k in top {
        if let Some(Value::String(s)) = args.remove(&k) {
            lanes.push((k, s));
        }
    }
    if lanes.is_empty() {
        bail!(
            "missing \"pattern\": an object of drum name -> steps, e.g. {{\"kick\": \"x...x...x...x...\", \"snare\": \"....x.......x...\", \"hat\": \"x.x.x.x.x.x.x.x.\"}}"
        );
    }
    Ok(lanes)
}

fn step_count(steps: &str) -> usize {
    steps
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '|')
        .count()
}

// ---------------------------------------------------------------------------
// Argument normalisation

/// Turn numeric strings into numbers, whole floats into integers and
/// "true"/"false" strings into booleans, recursively for known keys.
fn coerce(args: &mut Args) {
    for (k, v) in args.iter_mut() {
        match v {
            Value::String(s) if NUMERIC_KEYS.contains(&k.as_str()) => {
                let t = s.trim();
                if let Ok(i) = t.parse::<i64>() {
                    *v = json!(i);
                } else if let Ok(f) = t.parse::<f64>() {
                    *v = json!(f);
                }
            }
            Value::Number(n) if n.is_f64() => {
                let f = n.as_f64().unwrap_or(f64::NAN);
                if f.fract() == 0.0 && f.abs() < 1e15 {
                    *v = json!(f as i64);
                }
            }
            Value::String(s) if BOOL_KEYS.contains(&k.as_str()) => {
                match s.trim().to_ascii_lowercase().as_str() {
                    "true" | "yes" | "on" | "1" => *v = json!(true),
                    "false" | "no" | "off" | "0" => *v = json!(false),
                    _ => {}
                }
            }
            Value::Object(m) => coerce(m),
            Value::Array(items) => {
                for item in items {
                    if let Value::Object(m) = item {
                        coerce(m);
                    }
                }
            }
            _ => {}
        }
    }
}

fn unknown_keys(tool: &Tool, args: &Args) -> Vec<String> {
    let mut out = Vec::new();
    for k in args.keys() {
        let known = tool.params.iter().any(|p| p.name == k.as_str())
            || HIDDEN_KEYS.contains(&k.as_str())
            || (tool.name == "add_track" && (SYNTH_KEYS.contains(&k.as_str()) || k == "kit"))
            || (tool.name == "add_drum_pattern" && music::drum_note(k).is_some())
            || (tool.name == "add_melody" && k == "melody")
            || (tool.name.starts_with("add_") && k == "instrument")
            || (tool.name == "get_project" && k == "full");
        if !known {
            let valid: Vec<&str> = tool.params.iter().map(|p| p.name).collect();
            out.push(format!(
                "ignored unknown argument {k:?} (valid for {}: {})",
                tool.name,
                if valid.is_empty() {
                    "none".into()
                } else {
                    valid.join(", ")
                }
            ));
        }
    }
    out
}

fn has_any(args: &Args, keys: &[&str]) -> bool {
    keys.iter().any(|k| args.contains_key(*k))
}

fn str_arg<'a>(args: &'a Args, key: &str) -> Result<&'a str> {
    match args.get(key) {
        Some(Value::String(s)) => Ok(s),
        Some(other) => bail!("{key:?} must be a string (got {other})"),
        None => bail!("missing argument {key:?}"),
    }
}

fn ref_string(v: Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// Take the track reference out of `track` / `track_id` / `track_name`.
fn track_query(args: &mut Args) -> Option<String> {
    ["track", "track_id", "track_name"]
        .iter()
        .find_map(|k| args.remove(*k))
        .and_then(ref_string)
}

/// Resolve the track reference in `args` to an id stored as `track_id`.
fn resolve_track(work: &Project, args: &mut Args) -> Result<String> {
    let q = track_query(args).ok_or_else(|| {
        anyhow!(
            "missing \"track\" (a track name or id). {}",
            track_list(work)
        )
    })?;
    let id = find_track(work, &q)?.id.clone();
    args.insert("track_id".into(), json!(id));
    Ok(id)
}

fn norm(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Find a track by id, name (case and punctuation ignored), or position:
/// `"2"` or `"Track 2"` is the second track.
fn find_track<'a>(p: &'a Project, q: &str) -> Result<&'a aidaw_core::Track> {
    let q = q.trim();
    if norm(q).is_empty() {
        bail!("the track name is empty. {}", track_list(p));
    }
    let nth = || {
        let lower = q.to_ascii_lowercase();
        let n: usize = lower
            .strip_prefix("track")
            .unwrap_or(&lower)
            .trim()
            .trim_start_matches('#')
            .parse()
            .ok()?;
        p.tracks.get(n.checked_sub(1)?)
    };
    p.tracks
        .iter()
        .find(|t| t.id == q)
        .or_else(|| p.tracks.iter().find(|t| t.name.eq_ignore_ascii_case(q)))
        .or_else(|| {
            let n = norm(q);
            p.tracks.iter().find(|t| norm(&t.name) == n)
        })
        .or_else(nth)
        .ok_or_else(|| {
            anyhow!(
                "no track named or with id {q:?}. {}. To make a new one: add_track {{\"name\": \"{q}\"}}",
                track_list(p)
            )
        })
}

fn track_list(p: &Project) -> String {
    if p.tracks.is_empty() {
        return "The project has no tracks yet; create one with add_track, e.g. {\"name\": \"Bass\", \"instrument\": \"bass\"}".into();
    }
    let list: Vec<String> = p
        .tracks
        .iter()
        .map(|t| format!("{} \"{}\"", t.id, t.name))
        .collect();
    format!("Existing tracks: {}", list.join(", "))
}

/// Resolve `clip` / `clip_id` / `clip_name` (an id or a unique name) to `clip_id`.
fn resolve_clip(work: &Project, args: &mut Args) -> Result<String> {
    let q = ["clip", "clip_id", "clip_name"]
        .iter()
        .find_map(|k| args.remove(*k))
        .and_then(ref_string)
        .ok_or_else(|| {
            anyhow!(
                "missing \"clip\" (a clip id like \"c1\" or a clip name). {}",
                clip_list(work)
            )
        })?;
    let q = q.trim();
    let id = if work.clip(q).is_some() {
        q.to_string()
    } else {
        let matches: Vec<(&aidaw_core::Track, &aidaw_core::Clip)> = work
            .tracks
            .iter()
            .flat_map(|t| t.clips.iter().map(move |c| (t, c)))
            .filter(|(_, c)| !c.name.is_empty() && norm(&c.name) == norm(q))
            .collect();
        match matches.as_slice() {
            [(_, c)] => c.id.clone(),
            [] => bail!("no clip named or with id {q:?}. {}", clip_list(work)),
            many => bail!(
                "clip name {q:?} is ambiguous; use one of the ids: {}",
                many.iter()
                    .map(|(t, c)| format!(
                        "{} (track \"{}\", bar {})",
                        c.id,
                        t.name,
                        bar_of(c.start, work.beats_per_bar as f64)
                    ))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    };
    args.insert("clip_id".into(), json!(id));
    Ok(id)
}

fn clip_list(p: &Project) -> String {
    let bpb = p.beats_per_bar as f64;
    let list: Vec<String> = p
        .tracks
        .iter()
        .flat_map(|t| {
            t.clips.iter().map(move |c| {
                format!(
                    "{} (track \"{}\", {})",
                    c.id,
                    t.name,
                    bar_range(c.start, c.end(), bpb)
                )
            })
        })
        .collect();
    if list.is_empty() {
        "There are no clips yet; create one with add_clip or add_melody.".into()
    } else {
        format!("Existing clips: {}", list.join(", "))
    }
}

/// Convert `bar` (1-based) or `beat` into an absolute `start` in beats.
fn position(work: &Project, args: &mut Args) -> Result<()> {
    let bpb = work.beats_per_bar as f64;
    if let Some(v) = args.remove("bar") {
        let bar = v
            .as_f64()
            .ok_or_else(|| anyhow!("bar must be a number like 1 or 5 (got {v})"))?;
        if bar < 1.0 {
            bail!(
                "bars are numbered from 1 (bar 1 = beat 0, the start of the song); got bar {bar}"
            );
        }
        if bar > 4096.0 {
            bail!("bar {bar} is too far; songs can be at most 4096 bars long");
        }
        args.insert("start".into(), json!((bar - 1.0) * bpb));
    } else if let Some(v) = args.remove("beat") {
        args.insert("start".into(), v);
    }
    if let Some(s) = args.get("start") {
        match s.as_f64() {
            Some(s) if s >= 0.0 && s <= max_end(bpb) => {}
            _ => bail!("start must be a number of beats from 0 to {} (4096 bars) (got {s}); or give \"bar\": 1", max_end(bpb)),
        }
    }
    Ok(())
}

fn bars_to_length(args: &mut Args, bpb: f64) -> Result<()> {
    if let Some(v) = args.remove("bars") {
        let bars = v
            .as_f64()
            .filter(|b| *b > 0.0 && *b <= 4096.0)
            .ok_or_else(|| anyhow!("bars must be a number from 1 to 4096 (got {v})"))?;
        args.insert("length".into(), json!(bars * bpb));
    }
    Ok(())
}

/// The first bar line at or after the end of the track's last clip.
fn append_position(work: &Project, track_id: &str) -> f64 {
    let end = work
        .track(track_id)
        .map(|t| t.clips.iter().map(|c| c.end()).fold(0.0, f64::max))
        .unwrap_or(0.0);
    let bpb = work.beats_per_bar as f64;
    (end / bpb - 1e-9).ceil().max(0.0) * bpb
}

fn round_up_to_bar(beats: f64, bpb: f64) -> f64 {
    ((beats / bpb - 1e-9).ceil().max(1.0)) * bpb
}

fn bar_of(beat: f64, bpb: f64) -> String {
    fmt_num(beat / bpb + 1.0)
}

/// "bar 5" or "bars 1-4".
fn bar_range(start: f64, end: f64, bpb: f64) -> String {
    let (first, last) = (bar_of(start, bpb), fmt_num((end / bpb - 1e-9).ceil()));
    if last == first || (end - start) <= bpb + 1e-9 && start % bpb == 0.0 {
        format!("bar {first}")
    } else {
        format!("bars {first}-{last}")
    }
}

fn fmt_num(x: f64) -> String {
    let r = (x * 1000.0).round() / 1000.0;
    format!("{r}")
}

/// Make `notes` a JSON array of note objects: melody strings are parsed,
/// common key aliases accepted, drum names turned into GM numbers.
fn normalize_notes(args: &mut Args, drums: bool, limit: f64) -> Result<()> {
    let Some(v) = args.remove("notes") else {
        return Ok(());
    };
    let notes = match v {
        Value::String(s) if s.trim_start().starts_with('[') => serde_json::from_str(&s)
            .map_err(|e| anyhow!("notes looked like JSON but did not parse: {e}"))?,
        Value::String(s) => serde_json::to_value(music::parse_melody(&s, 100, drums)?.0)?,
        Value::Array(items) => {
            let mut out = Vec::with_capacity(items.len());
            for (i, item) in items.into_iter().enumerate() {
                let Value::Object(mut m) = item else {
                    bail!(
                        "note #{i} must be an object like {{\"pitch\": \"C4\", \"start\": 0, \"duration\": 1}} (got {item})"
                    );
                };
                for (alias, key) in [
                    ("note", "pitch"),
                    ("midi", "pitch"),
                    ("time", "start"),
                    ("beat", "start"),
                    ("offset", "start"),
                    ("position", "start"),
                    ("length", "duration"),
                    ("dur", "duration"),
                    ("len", "duration"),
                    ("vel", "velocity"),
                ] {
                    if !m.contains_key(key) {
                        if let Some(v) = m.remove(alias) {
                            m.insert(key.into(), v);
                        }
                    }
                }
                if let Some(Value::String(s)) = m.get("pitch") {
                    if pitch::parse(s).is_none() {
                        if !drums && music::drum_note(s).is_some() {
                            bail!("note #{i}: {s:?} is a drum name, but this is not a drum track; use note names like \"C4\"");
                        }
                        let p = music::drum_note(s).ok_or_else(|| {
                            anyhow!("note #{i}: bad pitch {s:?}; use a name like \"C4\", \"F#3\", \"Bb2\" (C4 = 60) or a MIDI number 0-127")
                        })?;
                        m.insert("pitch".into(), json!(p));
                    }
                }
                coerce(&mut m);
                for key in ["pitch", "start", "duration"] {
                    if !m.contains_key(key) {
                        bail!(
                            "note #{i} is missing {key:?}; each note needs pitch, start (beats from clip start) and duration (beats), e.g. {{\"pitch\": \"C4\", \"start\": 0, \"duration\": 1}}"
                        );
                    }
                }
                let num = |k: &str| m.get(k).and_then(Value::as_f64).unwrap_or(0.0);
                if num("start") + num("duration") > limit {
                    bail!("note #{i} ends after beat {limit}; songs can be at most 4096 bars long");
                }
                out.push(Value::Object(m));
            }
            Value::Array(out)
        }
        other => bail!("notes must be an array of note objects or a melody string like \"C4/1 E4/1\" (got {other})"),
    };
    args.insert("notes".into(), notes);
    Ok(())
}

fn note_end(notes: Option<&Value>) -> f64 {
    notes
        .and_then(Value::as_array)
        .map(|ns| {
            ns.iter()
                .filter_map(|n| Some(n.get("start")?.as_f64()? + n.get("duration")?.as_f64()?))
                .fold(0.0, f64::max)
        })
        .unwrap_or(0.0)
}

fn preset_ids() -> String {
    presets::all()
        .iter()
        .map(|p| p.id)
        .collect::<Vec<_>>()
        .join(", ")
}

fn kit_names() -> String {
    DrumKit::ALL
        .iter()
        .map(|k| k.label())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Turn the `instrument` argument (preset name, waveform name or object),
/// a drum `kit` and any flat synth settings into an [`Instrument`] JSON object.
fn resolve_instrument(args: &mut Args, existing: Option<Instrument>) -> Result<()> {
    let bad = |what: &str| {
        anyhow!(
            "unknown instrument {what}. Use a preset id: {} (or a plain waveform: sine, triangle, saw, square, pulse, noise)",
            preset_ids()
        )
    };
    let from_name = |s: &str| -> Result<Instrument> {
        if let Some(p) = presets::find(s) {
            return Ok(p.instrument.clone());
        }
        let wave = match norm(s).as_str() {
            "saw" | "sawtooth" | "synth" => Waveform::Saw,
            "square" => Waveform::Square,
            "triangle" | "tri" => Waveform::Triangle,
            "pulse" => Waveform::Pulse,
            "noise" => Waveform::Noise,
            _ => return Err(bad(&format!("{s:?}"))),
        };
        Ok(with_waveform(Instrument::default_synth(), wave))
    };
    let mut inst = match args.remove("instrument") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(from_name(&s)?),
        Some(Value::Object(m)) => {
            if let Some(Value::String(p)) = m.get("preset") {
                Some(from_name(p)?)
            } else {
                match m.get("type").and_then(Value::as_str) {
                    Some("synth" | "drums") => Some(
                        serde_json::from_value(Value::Object(m))
                            .map_err(|e| anyhow!("invalid instrument object: {e}"))?,
                    ),
                    Some(other) => Some(from_name(other)?),
                    None => bail!("instrument object needs \"type\": \"synth\" or \"drums\"; simpler: pass a preset id string like \"bass\""),
                }
            }
        }
        Some(other) => return Err(bad(&other.to_string())),
    };
    if let Some(k) = args.remove("kit") {
        let name = k.as_str().map(norm).unwrap_or_else(|| k.to_string());
        let kit = DrumKit::ALL
            .into_iter()
            .find(|d| norm(d.label()) == name || format!("tr{}", d.label()) == name)
            .ok_or_else(|| anyhow!("unknown drum kit {k}; kits: {}", kit_names()))?;
        let base = inst.as_ref().or(existing.as_ref());
        if base.is_some_and(|b| !b.is_drums()) && inst.is_some() {
            bail!(
                "\"kit\" is for drum tracks, but the instrument given is a synth; drop one of them"
            );
        }
        if existing.as_ref().is_some_and(|b| !b.is_drums()) && inst.is_none() {
            bail!("this track plays a synth, so it has no drum kit; set \"instrument\": \"drums_{}\" to turn it into drums", kit.label());
        }
        inst = Some(Instrument::Drums { kit });
    }
    if SYNTH_KEYS.iter().any(|k| args.contains_key(*k)) {
        let base = inst
            .take()
            .or(existing)
            .unwrap_or_else(Instrument::default_synth);
        let mut v = serde_json::to_value(base)?;
        if v.get("type").and_then(Value::as_str) == Some("drums") {
            bail!("drum kits have no synth settings (waveform, attack, ...); pick another kit with \"kit\" ({}) or change the instrument to a synth preset first", kit_names());
        }
        for k in SYNTH_KEYS {
            if let Some(x) = args.remove(*k) {
                v[*k] = x;
            }
        }
        inst = Some(
            serde_json::from_value(v)
                .map_err(|e| anyhow!("invalid synth setting: {e}; waveform is sine|triangle|saw|square|pulse|noise, times in seconds, unison 1-5"))?,
        );
    }
    if let Some(inst) = inst {
        args.insert("instrument".into(), serde_json::to_value(inst)?);
    }
    Ok(())
}

fn with_waveform(inst: Instrument, wave: Waveform) -> Instrument {
    match inst {
        Instrument::Synth(s) => Instrument::Synth(Synth {
            waveform: wave,
            ..s
        }),
        other => other,
    }
}

fn instrument_label(inst: &Instrument) -> String {
    if let Some(p) = presets::of(inst) {
        return p.id.to_string();
    }
    match inst {
        Instrument::Drums { kit } => format!("drums ({} kit)", kit.label()),
        Instrument::Synth(s) => format!("custom {} synth", s.waveform.label()),
    }
}

// ---------------------------------------------------------------------------
// Output

fn list_presets(args: &Args) -> Result<String> {
    let wanted = match args.get("category") {
        Some(Value::String(c)) => {
            let c = norm(c);
            let found = presets::Category::ALL
                .into_iter()
                .find(|x| norm(x.id()) == c || norm(presets::plain_name(x.label())) == c)
                .ok_or_else(|| {
                    let all: Vec<_> = presets::Category::ALL.iter().map(|c| c.id()).collect();
                    anyhow!("unknown category {c:?}; categories: {}", all.join(", "))
                })?;
            Some(found)
        }
        _ => None,
    };
    let mut s = format!(
        "{} instrument presets (pass the id as \"instrument\"):\n",
        presets::all().len()
    );
    for c in presets::Category::ALL {
        if wanted.is_some_and(|w| w != c) {
            continue;
        }
        s.push_str(&format!("\n[{}]\n", c.id()));
        for p in presets::Preset::in_category(c) {
            s.push_str(&format!(
                "- {} ({}): {}\n",
                p.id,
                presets::plain_name(p.name),
                p.hint
            ));
        }
    }
    s.push_str(
        "\nAlso accepted: plain waveforms sine, triangle, saw, square, pulse, noise, and loose names like \
         \"rhodes\", \"violin\", \"808\". Fine-tune a synth with update_track (waveform, attack, decay, sustain, \
         release, cutoff, resonance, filter_env, unison, detune, sub, noise, fm, vibrato, pitch_env, drive...). \
         Drum tracks: update_track {\"kit\": \"808\"}; drums also include percussion lanes such as cowbell, \
         tambourine, shaker, conga_high, bongo_high, claves, triangle.\nExample: add_track {\"name\": \"Bass\", \"instrument\": \"bass_808\"}",
    );
    Ok(s)
}

fn default_wav_path(project: &Path) -> PathBuf {
    let name = project
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("project");
    let stem = name
        .strip_suffix(".aidaw.json")
        .or_else(|| name.strip_suffix(".json"))
        .unwrap_or(name);
    project.with_file_name(format!("{stem}.wav"))
}

/// A compact overview so the model does not need to read the full JSON every time.
pub fn summary(p: &Project) -> String {
    overview(p, None, false)
}

fn overview(p: &Project, only: Option<&str>, with_notes: bool) -> String {
    let bpb = p.beats_per_bar as f64;
    let len = p.length_beats();
    let mut s = format!(
        "\"{}\" — {} BPM, {}/4, {} beats ({} bars), {} track(s), reverb room {:.2} tone {:.2}",
        p.name,
        p.bpm,
        p.beats_per_bar,
        fmt_num(len),
        fmt_num((len / bpb).ceil()),
        p.tracks.len(),
        p.reverb.size,
        p.reverb.tone,
    );
    for t in p.tracks.iter().filter(|t| only.is_none_or(|id| t.id == id)) {
        s.push_str(&format!(
            "\n- {} \"{}\" [{}] gain {:.2} pan {:.2} reverb {:.2}{}{}",
            t.id,
            t.name,
            instrument_label(&t.instrument),
            t.gain,
            t.pan,
            t.reverb,
            if t.mute { " MUTED" } else { "" },
            if t.solo { " SOLO" } else { "" }
        ));
        for c in &t.clips {
            let (lo, hi) = c.notes.iter().fold((127u8, 0u8), |(lo, hi), n| {
                (lo.min(n.pitch), hi.max(n.pitch))
            });
            let range = if c.notes.is_empty() || t.instrument.is_drums() {
                String::new()
            } else {
                format!(", {}..{}", pitch::name(lo), pitch::name(hi))
            };
            let name = if c.name.is_empty() {
                String::new()
            } else {
                format!(" \"{}\"", c.name)
            };
            s.push_str(&format!(
                "\n    - {}{name} {} (beats {}..{}) {} notes{range}",
                c.id,
                bar_range(c.start, c.end(), bpb),
                fmt_num(c.start),
                fmt_num(c.end()),
                c.notes.len(),
            ));
            if with_notes && !c.notes.is_empty() {
                s.push_str(&format!(
                    "\n      notes (start:PITCH/BEATS): {}",
                    music::format_notes(&c.notes, t.instrument.is_drums())
                ));
            }
        }
    }
    s
}
