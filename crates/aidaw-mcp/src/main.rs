//! `aidaw-mcp`: a Model Context Protocol server (stdio, JSON-RPC 2.0) that
//! lets an AI assistant compose music by editing an AIDAW project file.
//!
//! The GUI watches the same file and reloads it live, so a human sees every
//! change the AI makes and can fine-tune it (and undo it) in the editor.
//!
//! Usage: `aidaw-mcp [--project path/to/song.aidaw.json]`

use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};

use aidaw_core::edit::{self, Op};
use aidaw_core::{io as pio, pitch, render, Project};
use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Map, Value};

const PROTOCOL_VERSION: &str = "2025-06-18";

const INSTRUCTIONS: &str = "AIDAW is a DAW whose project is a JSON document. Time is measured in beats \
(quarter notes); clip positions are absolute beats, note starts are relative to their clip. \
Pitches are MIDI numbers or names like \"C4\" (= 60). Drum tracks use General MIDI drum notes \
(36 kick, 38 snare, 39 clap, 42 closed hat, 46 open hat, 41/45/48 toms, 49 crash). \
Typical flow: new_project or open_project -> add_track -> add_clip with notes -> render_wav to listen. \
Prefer apply_ops to make many changes in one atomic step. The human may edit the same project in \
the AIDAW GUI at any time, so call get_project before editing if you have not looked at it recently.";

struct Server {
    path: Option<PathBuf>,
}

fn main() -> Result<()> {
    let mut path = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--project" | "-p" => path = args.next().map(PathBuf::from),
            "--help" | "-h" => {
                eprintln!("usage: aidaw-mcp [--project <file.aidaw.json>]");
                return Ok(());
            }
            other => bail!("unknown argument {other}"),
        }
    }
    let mut server = Server { path };

    let stdin = io::stdin();
    let mut stdout = io::stdout().lock();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<Value>(&line) {
            Ok(msg) => server.handle(msg),
            Err(e) => Some(error_response(
                Value::Null,
                -32700,
                &format!("parse error: {e}"),
            )),
        };
        if let Some(resp) = response {
            serde_json::to_writer(&mut stdout, &resp)?;
            stdout.write_all(b"\n")?;
            stdout.flush()?;
        }
    }
    Ok(())
}

fn error_response(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

impl Server {
    fn handle(&mut self, msg: Value) -> Option<Value> {
        // Notifications (no id) never get a response.
        let id = msg.get("id")?.clone();
        let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
        let params = msg.get("params").cloned().unwrap_or(Value::Null);
        let result = match method {
            "initialize" => {
                let version = params
                    .get("protocolVersion")
                    .and_then(Value::as_str)
                    .unwrap_or(PROTOCOL_VERSION)
                    .to_string();
                json!({
                    "protocolVersion": version,
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": "aidaw", "version": env!("CARGO_PKG_VERSION")},
                    "instructions": INSTRUCTIONS,
                })
            }
            "ping" => json!({}),
            "tools/list" => json!({"tools": tool_definitions()}),
            "tools/call" => {
                let name = params
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let args = params
                    .get("arguments")
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                match self.call(&name, args) {
                    Ok(text) => {
                        json!({"content": [{"type": "text", "text": text}], "isError": false})
                    }
                    Err(e) => {
                        json!({"content": [{"type": "text", "text": format!("error: {e:#}")}], "isError": true})
                    }
                }
            }
            _ => {
                return Some(error_response(
                    id,
                    -32601,
                    &format!("method not found: {method}"),
                ))
            }
        };
        Some(json!({"jsonrpc": "2.0", "id": id, "result": result}))
    }

    fn path(&self) -> Result<&Path> {
        self.path
            .as_deref()
            .ok_or_else(|| anyhow!("no project is open; call new_project or open_project first"))
    }

    fn call(&mut self, name: &str, args: Value) -> Result<String> {
        match name {
            "new_project" => {
                let path = PathBuf::from(str_arg(&args, "path")?);
                if path.exists()
                    && !args
                        .get("overwrite")
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
                {
                    bail!(
                        "{} already exists; pass overwrite: true or use open_project",
                        path.display()
                    );
                }
                let name = args
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("Untitled");
                let bpm = args.get("bpm").and_then(Value::as_f64).unwrap_or(120.0);
                let project = Project::new(name, bpm);
                pio::save(&project, &path)?;
                self.path = Some(path.clone());
                Ok(format!("created {}", path.display()))
            }
            "open_project" => {
                let path = PathBuf::from(str_arg(&args, "path")?);
                let project = pio::load(&path)?;
                self.path = Some(path);
                Ok(summary(&project))
            }
            "get_project" => {
                let project = pio::load(self.path()?)?;
                Ok(format!(
                    "{}\n\n{}",
                    summary(&project),
                    serde_json::to_string_pretty(&project)?
                ))
            }
            "apply_ops" => {
                let ops: Vec<Op> =
                    serde_json::from_value(args.get("ops").cloned().unwrap_or(Value::Null))
                        .context("ops must be an array of operation objects")?;
                self.edit(ops)
            }
            "render_wav" => {
                let project = pio::load(self.path()?)?;
                let out = match args.get("output_path").and_then(Value::as_str) {
                    Some(p) => PathBuf::from(p),
                    None => default_wav_path(self.path()?),
                };
                let sr = args
                    .get("sample_rate")
                    .and_then(Value::as_u64)
                    .unwrap_or(44_100) as u32;
                let seconds = render::render_to_wav(&project, &out, sr)?;
                Ok(format!("rendered {seconds:.1}s to {}", out.display()))
            }
            op if OP_TOOLS.iter().any(|(n, _)| *n == op) => {
                let mut obj: Map<String, Value> = match args {
                    Value::Object(m) => m,
                    Value::Null => Map::new(),
                    _ => bail!("arguments must be an object"),
                };
                obj.insert("op".into(), Value::String(op.into()));
                let op: Op = serde_json::from_value(Value::Object(obj))?;
                self.edit(vec![op])
            }
            _ => bail!("unknown tool {name}"),
        }
    }

    /// Load, apply, save. Re-reading the file on every call picks up edits the
    /// human made in the GUI in the meantime.
    fn edit(&self, ops: Vec<Op>) -> Result<String> {
        let path = self.path()?;
        let mut project = pio::load(path)?;
        let results = edit::apply_all(&mut project, ops)?;
        pio::save(&project, path)?;
        Ok(serde_json::to_string(
            &json!({"results": results, "summary": summary(&project)}),
        )?)
    }
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

fn str_arg<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("missing string argument {key:?}"))
}

/// A compact overview so the model does not need to read the full JSON every time.
fn summary(p: &Project) -> String {
    let mut s = format!(
        "\"{}\" — {} BPM, {}/4, {:.1} beats long, {} track(s)",
        p.name,
        p.bpm,
        p.beats_per_bar,
        p.length_beats(),
        p.tracks.len()
    );
    for t in &p.tracks {
        let inst = match &t.instrument {
            aidaw_core::Instrument::Drums => "drums".to_string(),
            aidaw_core::Instrument::Synth { waveform, .. } => format!("synth/{}", waveform.label()),
        };
        s.push_str(&format!(
            "\n- {} \"{}\" [{}] gain {:.2} pan {:.2}{}{}",
            t.id,
            t.name,
            inst,
            t.gain,
            t.pan,
            if t.mute { " MUTED" } else { "" },
            if t.solo { " SOLO" } else { "" }
        ));
        for c in &t.clips {
            let (lo, hi) = c.notes.iter().fold((127u8, 0u8), |(lo, hi), n| {
                (lo.min(n.pitch), hi.max(n.pitch))
            });
            let range = if c.notes.is_empty() {
                String::new()
            } else {
                format!(", {}..{}", pitch::name(lo), pitch::name(hi))
            };
            s.push_str(&format!(
                "\n    - {} \"{}\" beats {}..{} ({} notes{})",
                c.id,
                c.name,
                c.start,
                c.end(),
                c.notes.len(),
                range
            ));
        }
    }
    s
}

/// Tools that map 1:1 onto an [`Op`] with the same name.
const OP_TOOLS: &[(&str, &str)] = &[
    ("update_project", "Change project name, bpm (20-400), beats_per_bar or master_gain (0-2)."),
    ("add_track", "Add a track. Returns its track_id. instrument defaults to a saw synth."),
    ("update_track", "Change a track's name, instrument, gain (0-2, 1 = unity), pan (-1..1), mute or solo."),
    ("remove_track", "Delete a track and all its clips."),
    ("add_clip", "Add a clip to a track at an absolute beat position, optionally with notes. Returns clip_id."),
    ("update_clip", "Move, resize or rename a clip."),
    ("remove_clip", "Delete a clip."),
    ("add_notes", "Append notes to a clip. Note start is in beats relative to the clip start."),
    ("remove_notes", "Remove notes whose start is in [from, to) and that match pitch (all filters optional; none clears the clip)."),
    ("transpose_clip", "Shift every note in a clip by a number of semitones."),
];

fn tool_definitions() -> Vec<Value> {
    let note = json!({
        "type": "object",
        "properties": {
            "pitch": {"type": ["integer", "string"], "description": "MIDI number 0-127 or a name like \"C4\", \"F#3\", \"Bb2\" (C4 = 60)"},
            "start": {"type": "number", "description": "beats from clip start, >= 0"},
            "duration": {"type": "number", "description": "beats, > 0"},
            "velocity": {"type": "integer", "minimum": 1, "maximum": 127, "default": 100}
        },
        "required": ["pitch", "start", "duration"]
    });
    let notes = json!({"type": "array", "items": note});
    let instrument = json!({
        "type": "object",
        "description": "{\"type\":\"drums\"} or {\"type\":\"synth\", waveform, attack, decay, sustain, release, cutoff}",
        "properties": {
            "type": {"type": "string", "enum": ["synth", "drums"]},
            "waveform": {"type": "string", "enum": ["sine", "triangle", "saw", "square"]},
            "attack": {"type": "number", "description": "seconds"},
            "decay": {"type": "number", "description": "seconds"},
            "sustain": {"type": "number", "description": "level 0-1"},
            "release": {"type": "number", "description": "seconds"},
            "cutoff": {"type": "number", "description": "low-pass cutoff in Hz"}
        },
        "required": ["type"]
    });
    let s = |d: &str| json!({"type": "string", "description": d});
    let n = |d: &str| json!({"type": "number", "description": d});
    let b = || json!({"type": "boolean"});

    let op_schema = |name: &str| -> Value {
        let (props, required): (Value, Vec<&str>) = match name {
            "update_project" => (
                json!({"name": s(""), "bpm": n(""), "beats_per_bar": {"type": "integer"}, "master_gain": n("")}),
                vec![],
            ),
            "add_track" => (
                json!({"name": s(""), "instrument": instrument, "gain": n(""), "pan": n("")}),
                vec!["name"],
            ),
            "update_track" => (
                json!({"track_id": s(""), "name": s(""), "instrument": instrument, "gain": n(""), "pan": n(""), "mute": b(), "solo": b()}),
                vec!["track_id"],
            ),
            "remove_track" => (json!({"track_id": s("")}), vec!["track_id"]),
            "add_clip" => (
                json!({"track_id": s(""), "start": n("absolute beat"), "length": n("beats"), "name": s(""), "notes": notes}),
                vec!["track_id", "start", "length"],
            ),
            "update_clip" => (
                json!({"clip_id": s(""), "name": s(""), "start": n("absolute beat"), "length": n("beats")}),
                vec!["clip_id"],
            ),
            "remove_clip" => (json!({"clip_id": s("")}), vec!["clip_id"]),
            "add_notes" => (
                json!({"clip_id": s(""), "notes": notes}),
                vec!["clip_id", "notes"],
            ),
            "remove_notes" => (
                json!({"clip_id": s(""), "from": n("beat, inclusive"), "to": n("beat, exclusive"), "pitch": {"type": ["integer", "string"]}}),
                vec!["clip_id"],
            ),
            "transpose_clip" => (
                json!({"clip_id": s(""), "semitones": {"type": "integer"}}),
                vec!["clip_id", "semitones"],
            ),
            _ => unreachable!(),
        };
        json!({"type": "object", "properties": props, "required": required})
    };

    let mut tools = vec![
        json!({
            "name": "new_project",
            "description": "Create a new empty project file and make it the current project.",
            "inputSchema": {"type": "object", "properties": {
                "path": s("file path, ideally ending in .aidaw.json"), "name": s(""), "bpm": n("default 120"),
                "overwrite": b()
            }, "required": ["path"]}
        }),
        json!({
            "name": "open_project",
            "description": "Open an existing .aidaw.json project and make it current. Returns a summary.",
            "inputSchema": {"type": "object", "properties": {"path": s("")}, "required": ["path"]}
        }),
        json!({
            "name": "get_project",
            "description": "Return a summary plus the full JSON of the current project (tracks, clips, notes).",
            "inputSchema": {"type": "object", "properties": {}}
        }),
        json!({
            "name": "apply_ops",
            "description": "Apply many edits atomically (all or nothing). Each item is an object with \"op\" set to one of: \
                            update_project, add_track, update_track, remove_track, add_clip, update_clip, remove_clip, \
                            add_notes, remove_notes, transpose_clip, plus that tool's arguments. Ids created earlier in \
                            the same batch follow the pattern t<N>/c<N>; prefer separate calls if you need returned ids.",
            "inputSchema": {"type": "object", "properties": {
                "ops": {"type": "array", "items": {"type": "object", "properties": {"op": {"type": "string"}}, "required": ["op"]}}
            }, "required": ["ops"]}
        }),
        json!({
            "name": "render_wav",
            "description": "Render the current project to a 16-bit stereo WAV file (defaults to <project>.wav).",
            "inputSchema": {"type": "object", "properties": {"output_path": s(""), "sample_rate": {"type": "integer"}}}
        }),
    ];
    for (name, desc) in OP_TOOLS {
        tools.push(json!({"name": name, "description": desc, "inputSchema": op_schema(name)}));
    }
    tools
}
