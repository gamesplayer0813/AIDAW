//! Drive the real binary over stdio the way an MCP client would.

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

use serde_json::{json, Value};

#[test]
fn compose_and_render_over_mcp() {
    let dir = std::env::temp_dir().join(format!("aidaw-mcp-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let project = dir.join("song.aidaw.json");
    let wav = dir.join("song.wav");

    let mut child = Command::new(env!("CARGO_BIN_EXE_aidaw-mcp"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut next_id = 0;
    let mut rpc = |method: &str, params: Value| -> Value {
        next_id += 1;
        let msg = json!({"jsonrpc": "2.0", "id": next_id, "method": method, "params": params});
        writeln!(stdin, "{msg}").unwrap();
        let mut line = String::new();
        stdout.read_line(&mut line).unwrap();
        let resp: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(resp["id"], next_id);
        resp["result"].clone()
    };
    let call = |rpc: &mut dyn FnMut(&str, Value) -> Value, name: &str, args: Value| -> String {
        let r = rpc("tools/call", json!({"name": name, "arguments": args}));
        assert_eq!(r["isError"], false, "{name} failed: {r}");
        r["content"][0]["text"].as_str().unwrap().to_string()
    };

    let init = rpc(
        "initialize",
        json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "test", "version": "0"}}),
    );
    assert_eq!(init["serverInfo"]["name"], "aidaw");
    let tools = rpc("tools/list", json!({}));
    let names: Vec<&str> = tools["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    for expected in [
        "new_project",
        "get_project",
        "apply_ops",
        "add_notes",
        "render_wav",
    ] {
        assert!(names.contains(&expected), "missing tool {expected}");
    }

    call(
        &mut rpc,
        "new_project",
        json!({"path": project, "name": "Test", "bpm": 100, "overwrite": true}),
    );
    let out = call(
        &mut rpc,
        "add_track",
        json!({"name": "Bass", "instrument": {"type": "synth", "waveform": "square"}}),
    );
    assert!(out.contains("\"track_id\":\"t1\""), "{out}");
    call(
        &mut rpc,
        "add_clip",
        json!({"track_id": "t1", "start": 0, "length": 4,
        "notes": [{"pitch": "A2", "start": 0, "duration": 2}, {"pitch": "E2", "start": 2, "duration": 2}]}),
    );
    call(
        &mut rpc,
        "apply_ops",
        json!({"ops": [
            {"op": "add_track", "name": "Drums", "instrument": {"type": "drums"}},
            {"op": "add_clip", "track_id": "t2", "start": 0, "length": 4, "notes": [{"pitch": 36, "start": 0, "duration": 0.5}]}
        ]}),
    );

    // A failing op in a batch is reported as a tool error and changes nothing.
    let bad = rpc(
        "tools/call",
        json!({"name": "apply_ops", "arguments": {"ops": [
        {"op": "remove_track", "track_id": "t1"}, {"op": "remove_clip", "clip_id": "missing"}]}}),
    );
    assert_eq!(bad["isError"], true);

    let summary = call(&mut rpc, "get_project", json!({}));
    assert!(
        summary.contains("2 track(s)") && summary.contains("E2..A2"),
        "{summary}"
    );

    call(&mut rpc, "render_wav", json!({"output_path": wav}));
    let reader = hound::WavReader::open(&wav).unwrap();
    assert_eq!(reader.spec().channels, 2);
    assert!(reader.duration() > 44_100 * 2);

    drop(stdin);
    child.wait().unwrap();
    std::fs::remove_dir_all(&dir).ok();
}

/// A minimal MCP client over the real binary.
struct Client {
    child: std::process::Child,
    stdin: std::process::ChildStdin,
    stdout: BufReader<std::process::ChildStdout>,
    next_id: u64,
}

impl Client {
    fn start() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_aidaw-mcp"))
            .env_remove("AIDAW_PROJECT")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Client {
            child,
            stdin,
            stdout,
            next_id: 0,
        }
    }

    fn rpc(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let msg = json!({"jsonrpc": "2.0", "id": self.next_id, "method": method, "params": params});
        writeln!(self.stdin, "{msg}").unwrap();
        let mut line = String::new();
        self.stdout.read_line(&mut line).unwrap();
        let resp: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(resp["id"], self.next_id);
        resp["result"].clone()
    }

    /// Call a tool; returns (is_error, text).
    fn try_call(&mut self, name: &str, args: Value) -> (bool, String) {
        let r = self.rpc("tools/call", json!({"name": name, "arguments": args}));
        (
            r["isError"].as_bool().unwrap(),
            r["content"][0]["text"].as_str().unwrap().to_string(),
        )
    }

    fn call(&mut self, name: &str, args: Value) -> String {
        let (err, text) = self.try_call(name, args);
        assert!(!err, "{name} failed: {text}");
        text
    }

    fn call_err(&mut self, name: &str, args: Value) -> String {
        let (err, text) = self.try_call(name, args);
        assert!(err, "{name} should have failed: {text}");
        text
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.child.kill().ok();
        self.child.wait().ok();
    }
}

fn temp_dir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("aidaw-mcp-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn load(path: &std::path::Path) -> aidaw_core::Project {
    aidaw_core::io::load(path).unwrap()
}

#[test]
fn tool_list_is_simple_and_documented() {
    let mut c = Client::start();
    let init = c.rpc("initialize", json!({"protocolVersion": "2025-06-18"}));
    assert!(init["instructions"]
        .as_str()
        .unwrap()
        .contains("add_chords"));
    let tools = c.rpc("tools/list", json!({}));
    let tools = tools["tools"].as_array().unwrap();
    assert!(tools.len() <= 24, "keep the tool list small");
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    for expected in [
        "help",
        "list_presets",
        "add_chords",
        "add_drum_pattern",
        "add_melody",
        "get_project",
    ] {
        assert!(names.contains(&expected), "missing tool {expected}");
    }
    for t in tools {
        assert!(
            t["description"].as_str().unwrap().contains("Example: {"),
            "{} has no example",
            t["name"]
        );
        let schema = t["inputSchema"].to_string();
        assert!(!schema.contains("oneOf") && !schema.contains("anyOf"));
        assert!(!schema.contains("\"type\":["), "type unions in {schema}");
    }
    assert!(c.call("help", json!({})).contains("QUICK START"));
    assert!(c.call("get_guide", json!({})).contains("QUICK START"));
    assert!(c.call("list_presets", json!({})).contains("warm_pad"));
}

#[test]
fn forgiving_inputs_and_musical_tools() {
    let dir = temp_dir("forgiving");
    let path = dir.join("song.aidaw.json");
    let mut c = Client::start();

    // Before any project: the error says how to start.
    assert!(c
        .call_err("add_track", json!({"name": "x"}))
        .contains("new_project"));

    // Numbers as strings.
    c.call(
        "new_project",
        json!({"path": path, "bpm": "100", "overwrite": "true"}),
    );
    // Instrument from the track name, and by preset display name.
    assert!(c
        .call("add_track", json!({"name": "Drums"}))
        .contains("drum_kit"));
    c.call(
        "add_track",
        json!({"name": "Keys", "instrument": "Warm Pad"}),
    );
    // Arguments sent as a JSON string.
    c.call(
        "add_track",
        json!("{\"name\": \"Bass\", \"instrument\": {\"preset\": \"bass\"}}"),
    );

    let out = c.call(
        "add_drum_pattern",
        json!({"track": "drums", "bars": 2, "pattern": {
            "kick": "x...x...x...x...", "snare": "....x.......x...", "hat": "x."}}),
    );
    assert!(out.contains("bars 1-2"), "{out}");
    c.call(
        "add_chords",
        json!({"track": "Keys", "chords": "Am F", "style": "block"}),
    );
    // Without a bar, music continues after the track's last clip.
    c.call("add_chords", json!({"track": "t2", "chords": "C:2 G:2"}));
    c.call(
        "add_melody",
        json!({"track": "Bass", "bar": "1", "notes": "A2/4 F2/4"}),
    );
    // Same bar again: added into the existing clip, which grows.
    c.call(
        "add_melody",
        json!({"track": "Bass", "bar": 2, "notes": "F2/2 r/2 C2/4"}),
    );
    // A missing track is created with a matching preset.
    let out = c.call(
        "add_melody",
        json!({"track": "Lead", "bar": 2, "notes": "E5/1 D5/0.5 C5/0.5 A4/2"}),
    );
    assert!(
        out.contains("bright_lead") && out.contains("bar 2"),
        "{out}"
    );
    // Low-level tools take names and bars too.
    c.call(
        "add_clip",
        json!({"track": "Lead", "bar": 4, "notes": "G4/1 A4/1"}),
    );
    c.call("transpose_clip", json!({"clip": "c6", "semitones": "12"}));
    c.call(
        "update_track",
        json!({"track": "Lead", "cutoff": "1500", "solo": "no"}),
    );

    let p = load(&path);
    assert_eq!(p.bpm, 100.0);
    let drums = &p.tracks[0];
    assert!(drums.instrument.is_drums());
    assert_eq!(drums.clips[0].length, 8.0);
    let count = |pitch| {
        drums.clips[0]
            .notes
            .iter()
            .filter(|n| n.pitch == pitch)
            .count()
    };
    assert_eq!((count(36), count(38), count(42)), (8, 4, 16));

    let keys = &p.tracks[1];
    assert_eq!(keys.clips.len(), 2);
    assert_eq!((keys.clips[1].start, keys.clips[1].length), (8.0, 4.0));
    let first: Vec<u8> = keys.clips[0]
        .notes
        .iter()
        .take(3)
        .map(|n| n.pitch)
        .collect();
    assert_eq!(first, vec![57, 60, 64]); // A3 C4 E4

    let bass = &p.tracks[2];
    assert_eq!(bass.clips.len(), 1);
    assert_eq!(bass.clips[0].length, 12.0);
    let starts: Vec<f64> = bass.clips[0].notes.iter().map(|n| n.start).collect();
    assert_eq!(starts, vec![0.0, 4.0, 4.0, 8.0]);

    let lead = &p.tracks[3];
    assert_eq!(lead.name, "Lead");
    assert_eq!(lead.clips[0].start, 4.0);
    assert_eq!(lead.clips[1].start, 12.0);
    assert_eq!(lead.clips[1].notes[0].pitch, 79); // G4 + 12

    // get_project: compact by default, notes on request, raw JSON on request.
    let summary = c.call("get_project", json!({}));
    assert!(summary.contains("t3 \"Bass\" [bass]") && !summary.contains("\"pitch\""));
    let notes = c.call("get_project", json!({"detail": "notes", "track": "Bass"}));
    assert!(
        notes.contains("0:A2/4 4:F2/4") && !notes.contains("Drums"),
        "{notes}"
    );
    assert!(c
        .call("get_project", json!({"detail": "json"}))
        .contains("\"beats_per_bar\": 4"));

    // Errors explain themselves.
    let e = c.call_err("add_clip", json!({"track": "Guitar", "bar": 1}));
    assert!(
        e.contains("t1 \"Drums\"") && e.contains("Example add_clip call"),
        "{e}"
    );
    let e = c.call_err("add_track", json!({"name": "X", "instrument": "kazoo"}));
    assert!(e.contains("soft_keys") && e.contains("drum_kit"), "{e}");
    let e = c.call_err("add_melody", json!({"track": "Lead", "notes": "C4/1 H9/1"}));
    assert!(e.contains("\"H9\"") && e.contains("C4/1"), "{e}");
    let e = c.call_err("add_chords", json!({"track": "Keys", "chords": "Am Fxx"}));
    assert!(e.contains("Cmaj7"), "{e}");
    let e = c.call_err("add_clip", json!({"track": "Lead", "bar": 0}));
    assert!(e.contains("numbered from 1"), "{e}");
    let e = c.call_err("remove_clip", json!({"clip": "Nope"}));
    assert!(e.contains("Existing clips: c1"), "{e}");
    let e = c.call_err("make_song", json!({}));
    assert!(e.contains("add_melody"), "{e}");
    let (_, warn) = c.try_call("update_project", json!({"tempo": 90}));
    assert!(warn.contains("unknown argument \"tempo\""), "{warn}");

    // apply_ops takes musical tools and stays atomic.
    let before = load(&path);
    let e = c.call_err(
        "apply_ops",
        json!({"ops": [
            {"op": "add_track", "name": "Pad", "instrument": "warm_pad"},
            {"op": "add_chords", "track": "Pad", "chords": "Am F"},
            {"op": "remove_clip", "clip": "missing"}
        ]}),
    );
    assert!(e.contains("op #2"), "{e}");
    assert_eq!(load(&path), before);
    c.call(
        "apply_ops",
        json!({"ops": [
            {"op": "add_track", "name": "Pad", "instrument": "warm_pad"},
            {"op": "add_chords", "track": "Pad", "chords": "Am F", "style": "arpeggio"}
        ]}),
    );
    assert_eq!(load(&path).tracks.len(), 5);

    drop(c);
    std::fs::remove_dir_all(&dir).ok();
}

fn cli(args: &[&str], project: &std::path::Path) -> (bool, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_aidaw-mcp"))
        .args(args)
        .env("AIDAW_PROJECT", project)
        .output()
        .unwrap();
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn command_line_mode() {
    let dir = temp_dir("cli");
    let path = dir.join("cli.aidaw.json");
    let p = path.to_str().unwrap();

    let (ok, out, _) = cli(
        &[
            "call",
            "new_project",
            &json!({"path": p, "overwrite": true}).to_string(),
        ],
        &path,
    );
    assert!(ok, "{out}");
    // key=value pairs, project from --project.
    let (ok, out, err) = cli(
        &[
            "call",
            "add_chords",
            "track=Piano",
            "chords=Dm7 G7 Cmaj7",
            "--project",
            p,
        ],
        std::path::Path::new("ignored-by-flag.aidaw.json"),
    );
    assert!(ok, "{out}{err}");
    assert!(out.contains("3 chord(s)"), "{out}");
    let (ok, out, _) = cli(&["call", "get_project"], &path);
    assert!(ok && out.contains("\"Piano\" [soft_keys]"), "{out}");

    // Errors go to stderr with a non-zero exit code.
    let (ok, out, err) = cli(&["call", "remove_track", r#"{"track": "Nope"}"#], &path);
    assert!(!ok && out.is_empty());
    assert!(err.contains("Existing tracks: t1 \"Piano\""), "{err}");

    for (format, key) in [
        ("openai", "function"),
        ("anthropic", "input_schema"),
        ("mcp", "inputSchema"),
    ] {
        let (ok, out, _) = cli(&["tools", "--format", format], &path);
        assert!(ok);
        let v: Value = serde_json::from_str(&out).unwrap();
        let tools = v.as_array().unwrap();
        assert!(tools.len() >= 20 && tools[0].get(key).is_some(), "{format}");
    }
    let (ok, out, _) = cli(&["tools", "--format", "gemini"], &path);
    let v: Value = serde_json::from_str(&out).unwrap();
    assert!(ok && v[0]["functionDeclarations"].as_array().unwrap().len() >= 20);
    let (ok, _, err) = cli(&["tools", "--format", "xml"], &path);
    assert!(!ok && err.contains("openai"));
    let (ok, out, _) = cli(&["guide"], &path);
    assert!(ok && out.contains("add_drum_pattern"));

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn bad_inputs_are_rejected_clearly_and_never_kill_the_server() {
    let dir = temp_dir("bad");
    let path = dir.join("bad.aidaw.json");
    let mut c = Client::start();
    c.call("new_project", json!({"path": path, "overwrite": true}));
    c.call(
        "add_track",
        json!({"name": "Keys", "instrument": "soft_keys"}),
    );
    c.call("add_track", json!({"name": "Drums"}));
    c.call("add_melody", json!({"track": "Keys", "notes": "C4/1 E4/1"}));

    // Sample rates that would crash or overflow the renderer.
    for sr in [json!(0), json!("0"), json!(5_000_000_000u64)] {
        let e = c.call_err("render_wav", json!({"sample_rate": sr}));
        assert!(e.contains("8000 to 192000"), "{e}");
    }
    // Absurd lengths.
    let long = [
        ("add_melody", json!({"track": "Keys", "notes": "C4/1e7"})),
        ("add_chords", json!({"track": "Keys", "chords": "Am:1e7"})),
        (
            "add_chords",
            json!({"track": "Keys", "chords": "Am", "beats_per_chord": "1e7"}),
        ),
        ("add_clip", json!({"track": "Keys", "bars": 1e7})),
        ("add_clip", json!({"track": "Keys", "length": 1e9})),
        ("add_clip", json!({"track": "Keys", "bar": 4096, "bars": 2})),
        ("update_clip", json!({"clip": "c1", "start": 1e12})),
        (
            "add_notes",
            json!({"clip": "c1", "notes": [{"pitch": 60, "start": 0, "duration": 1e9}]}),
        ),
    ];
    for (tool, args) in long {
        let e = c.call_err(tool, args.clone());
        assert!(
            e.contains("4096") || e.contains("at most"),
            "{tool} {args}: {e}"
        );
    }
    // Unknown, non-preset track names are not created silently...
    let e = c.call_err("add_melody", json!({"track": "Melody", "notes": "C5/1"}));
    assert!(
        e.contains("Existing tracks: t1 \"Keys\"") && e.contains("instrument"),
        "{e}"
    );
    // ...but are with an explicit instrument, and the result says so up front.
    let out = c.call(
        "add_melody",
        json!({"track": "Melody", "instrument": "pluck", "notes": "C5/1"}),
    );
    assert!(
        out.contains("\"message\":\"created new track t3 \\\"Melody\\\" (pluck)"),
        "{out}"
    );
    let e = c.call_err("add_clip", json!({"track": "  "}));
    assert!(e.contains("empty"), "{e}");
    // Tracks by position.
    c.call("add_clip", json!({"track": "Track 2", "bar": 3}));
    assert_eq!(load(&path).tracks[1].clips.len(), 1);
    c.call("update_track", json!({"track": "3", "mute": true}));
    assert!(load(&path).tracks[2].mute);

    // Drum names only on drum tracks.
    let e = c.call_err("add_melody", json!({"track": "Keys", "notes": "C4/1 o/1"}));
    assert!(e.contains("drum"), "{e}");
    c.call(
        "add_melody",
        json!({"track": "Drums", "notes": "kick/1 o/1"}),
    );

    // Silent coercions are now errors or warnings.
    let e = c.call_err(
        "add_chords",
        json!({"track": "Keys", "chords": "C", "octave": 4.5}),
    );
    assert!(e.contains("whole number"), "{e}");
    let e = c.call_err("transpose_clip", json!({"clip": "c1", "semitones": 60}));
    assert!(e.contains("-48 to 48"), "{e}");
    let e = c.call_err("transpose_clip", json!({"clip": "c1", "semitones": 1.5}));
    assert!(e.contains("whole number"), "{e}");
    c.call(
        "add_melody",
        json!({"track": "Keys", "bar": 1, "notes": "G9/1"}),
    );
    let e = c.call_err("transpose_clip", json!({"clip": "c1", "semitones": 12}));
    assert!(e.contains("0-127"), "{e}");
    let e = c.call_err("update_project", json!({"beats_per_bar": 0}));
    assert!(e.contains("1 to 32"), "{e}");
    let out = c.call("update_track", json!({"track": "Keys", "gain": 5}));
    assert!(out.contains("clamped"), "{out}");

    // Extended chord qualities, including 6/9.
    let out = c.call(
        "add_chords",
        json!({"track": "Keys", "chords": "C6/9 G7b9 Dm11 Cmaj7#11 CmMaj7 A7#9 F13"}),
    );
    assert!(out.contains("7 chord(s)"), "{out}");

    // The server is still alive after all of that.
    assert!(c.call("get_project", json!({})).contains("3 track(s)"));
    drop(c);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn paths_are_relative_to_the_project_and_only_project_and_wav_files_are_written() {
    let dir = temp_dir("paths");
    let path = dir.join("song.aidaw.json");
    let mut c = Client::start();

    // Only *.aidaw.json may be created; a bare name or .json is completed.
    let out = c.call(
        "new_project",
        json!({"path": dir.join("draft"), "overwrite": true}),
    );
    assert!(out.contains("draft.aidaw.json"), "{out}");
    let out = c.call(
        "new_project",
        json!({"path": dir.join("other.json"), "overwrite": true}),
    );
    assert!(out.contains("other.aidaw.json"), "{out}");
    std::fs::write(dir.join("notes.txt"), "precious").unwrap();
    let e = c.call_err(
        "new_project",
        json!({"path": dir.join("notes.txt"), "overwrite": true}),
    );
    assert!(e.contains("aidaw.json"), "{e}");
    assert_eq!(
        std::fs::read_to_string(dir.join("notes.txt")).unwrap(),
        "precious"
    );

    c.call("new_project", json!({"path": path, "overwrite": true}));
    c.call("add_melody", json!({"track": "Lead", "notes": "C4/1"}));

    // Relative paths land next to the project, not in the server's cwd.
    let out = c.call("render_wav", json!({"output_path": "mix"}));
    assert!(
        out.contains(&dir.join("mix.wav").display().to_string()),
        "{out}"
    );
    assert!(dir.join("mix.wav").exists());
    let out = c.call("new_project", json!({"path": "second", "overwrite": true}));
    assert!(dir.join("second.aidaw.json").exists(), "{out}");
    assert!(c
        .call("open_project", json!({"path": "song.aidaw.json"}))
        .contains("Lead"));
    let e = c.call_err("open_project", json!({"path": "nope.aidaw.json"}));
    assert!(e.contains("does not exist"), "{e}");

    // render_wav never overwrites a non-WAV file.
    let e = c.call_err("render_wav", json!({"output_path": dir.join("notes.txt")}));
    assert!(e.contains(".wav"), "{e}");
    assert_eq!(
        std::fs::read_to_string(dir.join("notes.txt")).unwrap(),
        "precious"
    );

    drop(c);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn edits_made_by_the_gui_between_calls_are_kept() {
    let dir = temp_dir("concurrent");
    let path = dir.join("song.aidaw.json");
    let mut c = Client::start();
    c.call("new_project", json!({"path": path, "overwrite": true}));
    c.call("add_track", json!({"name": "Keys"}));

    // The human renames the song in the GUI (an external write)...
    let mut p = load(&path);
    p.name = "Renamed by human".into();
    aidaw_core::io::save(&p, &path).unwrap();
    // ...and the AI's next edit builds on that version.
    c.call("add_melody", json!({"track": "Keys", "notes": "C4/1"}));
    let p = load(&path);
    assert_eq!(p.name, "Renamed by human");
    assert_eq!(p.tracks[0].clips.len(), 1);
    // No temp files are left behind.
    let leftovers: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".tmp"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");

    drop(c);
    std::fs::remove_dir_all(&dir).ok();
}
