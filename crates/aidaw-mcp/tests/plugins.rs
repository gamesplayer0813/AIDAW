//! The plugin tools end to end: the real server, its plugin workers and
//! the test plugin from `aidaw-plugin` (an instrument and an effect),
//! installed into a scratch folder.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use serde_json::{json, Value};

/// The example plugin, which `cargo test` builds with the workspace. A
/// test of this crate alone may run before it exists: then there is
/// nothing to test against.
fn built_plugin() -> Option<PathBuf> {
    let exe = std::env::current_exe().unwrap();
    let dir = exe.parent()?.parent()?.join("examples");
    let lib = dir.join(if cfg!(windows) {
        "test_plugin.dll"
    } else {
        "libtest_plugin.so"
    });
    lib.exists().then_some(lib)
}

fn install(lib: &Path, folder: &Path) -> PathBuf {
    let bundle = folder.join("AIDAW Test.vst3");
    let (arch, ext) = if cfg!(windows) {
        ("x86_64-win", "vst3")
    } else {
        ("x86_64-linux", "so")
    };
    let dir = bundle.join("Contents").join(arch);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::copy(lib, dir.join(format!("AIDAW Test.{ext}"))).unwrap();
    bundle
}

struct Server {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    id: u64,
}

impl Server {
    fn start(config: &Path, plugins: &Path) -> Server {
        let mut child = Command::new(env!("CARGO_BIN_EXE_aidaw-mcp"))
            .env("AIDAW_CONFIG_DIR", config)
            .env("AIDAW_VST3_PATH", plugins)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Server {
            child,
            stdin,
            stdout,
            id: 0,
        }
    }

    fn call_raw(&mut self, name: &str, args: Value) -> (bool, String) {
        self.id += 1;
        let msg = json!({"jsonrpc": "2.0", "id": self.id, "method": "tools/call",
                         "params": {"name": name, "arguments": args}});
        writeln!(self.stdin, "{msg}").unwrap();
        let mut line = String::new();
        self.stdout.read_line(&mut line).unwrap();
        let resp: Value = serde_json::from_str(&line).unwrap_or_else(|e| panic!("{e}: {line}"));
        assert_eq!(resp["id"], self.id);
        let r = &resp["result"];
        (
            r["isError"] == true,
            r["content"][0]["text"].as_str().unwrap().to_string(),
        )
    }

    fn call(&mut self, name: &str, args: Value) -> String {
        let (err, text) = self.call_raw(name, args);
        assert!(!err, "{name} failed: {text}");
        text
    }

    fn call_err(&mut self, name: &str, args: Value) -> String {
        let (err, text) = self.call_raw(name, args);
        assert!(err, "{name} should have failed: {text}");
        text
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn peak(wav: &Path) -> f32 {
    hound::WavReader::open(wav)
        .unwrap()
        .samples::<i16>()
        .map(|s| f32::from(s.unwrap()).abs() / f32::from(i16::MAX))
        .fold(0.0, f32::max)
}

#[test]
fn the_ai_plays_and_shapes_plugins() {
    let Some(lib) = built_plugin() else {
        eprintln!("the test plugin is not built yet (cargo test --workspace builds it); skipped");
        return;
    };
    let dir = std::env::temp_dir().join(format!("aidaw-mcp-plugins-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let plugins = dir.join("plugins");
    let bundle = install(&lib, &plugins);
    let config = dir.join("config");
    let mut s = Server::start(&config, &plugins);
    s.call(
        "new_project",
        json!({"path": dir.join("song.aidaw.json"), "bpm": 120}),
    );

    let list = s.call("list_plugins", json!({}));
    assert!(
        list.contains("AIDAW Test Synth (AIDAW, Instrument|Synth)"),
        "{list}"
    );
    assert!(list.contains("AIDAW Test Gain"), "{list}");
    assert!(list.contains("looked at 1 new"), "{list}");
    let effects = s.call("list_plugins", json!({"kind": "effect"}));
    assert!(!effects.contains("Test Synth"), "{effects}");
    assert!(config.join("plugins.json").exists());

    let added = s.call("add_track", json!({"name": "Lead", "plugin": "test synth"}));
    assert!(added.contains("plugin AIDAW Test Synth"), "{added}");
    s.call("add_melody", json!({"track": "Lead", "notes": "A4/2 r/2"}));
    let fx = s.call("add_effect", json!({"track": "Lead", "plugin": "gain"}));
    assert!(fx.contains("fx1"), "{fx}");

    let knobs = s.call("plugin_params", json!({"track": "Lead"}));
    assert!(
        knobs.contains("1 \"Octave\" = 0.5 (Middle) options: Low | Middle | High"),
        "{knobs}"
    );
    let set = s.call(
        "set_plugin_params",
        json!({"track": "Lead", "param": "octave", "value": "High"}),
    );
    assert!(set.contains("Octave = 1 (High)"), "{set}");
    let set = s.call(
        "set_plugin_params",
        json!({"track": "Lead", "effect": "fx1", "params": [{"param": "Gain", "value": "1.5"}]}),
    );
    assert!(set.contains("Gain = 0.75 (1.50)"), "{set}");
    let knobs = s.call("plugin_params", json!({"track": "Lead", "effect": "gain"}));
    assert!(
        knobs.contains("= 0.75 (1.50 x) [set with set_plugin_params]"),
        "{knobs}"
    );
    let song = s.call("get_project", json!({}));
    assert!(
        song.contains("[plugin AIDAW Test Synth, 1 param set]"),
        "{song}"
    );
    assert!(
        song.contains("effects: fx1 \"AIDAW Test Gain\" (1 param set)"),
        "{song}"
    );

    // Rendered and measured through the plugins, in a worker process.
    let wav = dir.join("song.wav");
    let out = s.call("render_wav", json!({}));
    assert!(
        out.starts_with("rendered") && !out.contains("warning"),
        "{out}"
    );
    assert!(peak(&wav) > 0.05, "the synth is heard");
    let mix = s.call("analyze", json!({}));
    assert!(mix.contains("LUFS integrated"), "{mix}");

    let err = s.call_err("add_track", json!({"name": "X", "plugin": "Nonexistent"}));
    assert!(err.contains("AIDAW Test Synth"), "{err}");
    let err = s.call_err(
        "add_effect",
        json!({"track": "Lead", "plugin": "test synth"}),
    );
    assert!(err.contains("is an instrument, not an effect"), "{err}");
    let err = s.call_err(
        "set_plugin_params",
        json!({"track": "Lead", "param": "Volume", "value": 1}),
    );
    assert!(err.contains("no parameter"), "{err}");

    // Uninstalled: the song still renders, and says what is missing.
    std::fs::remove_dir_all(&bundle).unwrap();
    let out = s.call("render_wav", json!({}));
    assert!(out.contains("AIDAW Test Synth is not installed"), "{out}");
    assert!(peak(&wav) < 1e-3, "nothing left to hear");
    let _ = std::fs::remove_dir_all(&dir);
}
