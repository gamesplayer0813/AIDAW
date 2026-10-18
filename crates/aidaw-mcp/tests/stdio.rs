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
