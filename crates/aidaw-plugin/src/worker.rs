//! Plugin work in a separate process. Loading an unknown plugin can crash,
//! and some plugins print to stdout, which would corrupt the MCP server's
//! messages; so scanning, and every plugin job of the MCP server, runs in a
//! child process: the AIDAW program itself, started with [`FLAG`].
//!
//! The parent writes one [`Job`] as JSON to the child's stdin. The child
//! answers on stdout with lines starting with [`PREFIX`]: progress
//! (`{"progress":0.5}`), then the result (`{"done":…}`) or an error
//! (`{"error":"…"}`). Anything else on stdout is a plugin talking and is
//! ignored. A child that exits without a result has crashed.

use std::ffi::OsString;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::Duration;

use aidaw_core::analysis;
use aidaw_core::render;
use aidaw_core::{PluginRef, Project};
use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::instance::{Plugin, Setup};
use crate::module::Module;
use crate::params;
use crate::rack;

/// The command-line flag that turns AIDAW into a plugin worker.
pub const FLAG: &str = "--aidaw-plugin-worker";
/// Marks the worker's own lines on stdout.
pub const PREFIX: &str = "AIDAW-WORKER:";

/// Work for a worker process.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "job", rename_all = "snake_case")]
pub enum Job {
    /// List the plugins in one module.
    Scan { path: String },
    /// Render the song to a WAV file. Each plugin's `path` must say where
    /// it is installed.
    Render {
        project: Box<Project>,
        out: String,
        sample_rate: u32,
    },
    /// Measure the song (see [`analysis::analyze`]).
    Analyze { project: Box<Project> },
    /// List a plugin's parameters, with its settings applied.
    Params { plugin: Box<PluginRef> },
    /// Work out parameter values from what was asked for (see
    /// [`params::resolve`]): parameter name or id, and a value as a knob
    /// position, an option name or text such as "-6 dB".
    SetParams {
        plugin: Box<PluginRef>,
        wanted: Vec<(String, Value)>,
    },
}

/// How to start a worker.
#[derive(Debug, Clone)]
pub struct Worker {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub env: Vec<(OsString, OsString)>,
}

impl Worker {
    /// This program, started again as a worker.
    pub fn current_exe() -> Result<Worker> {
        Ok(Worker {
            program: std::env::current_exe().context("cannot find the AIDAW program")?,
            args: vec![FLAG.into()],
            env: Vec::new(),
        })
    }

    /// Run `job`, waiting at most `quiet` between two signs of life.
    /// `progress` hears values from 0 to 1.
    pub fn run(&self, job: &Job, quiet: Duration, progress: &mut dyn FnMut(f64)) -> Result<Value> {
        let mut cmd = Command::new(&self.program);
        cmd.args(&self.args)
            .envs(self.env.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = cmd
            .spawn()
            .with_context(|| format!("could not start {}", self.program.display()))?;
        let input = serde_json::to_vec(job)?;
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow!("no worker stdin"))?;
        let writer = std::thread::spawn(move || {
            let _ = stdin.write_all(&input);
        });
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("no worker stdout"))?;
        let (tx, rx) = mpsc::channel();
        let reader = std::thread::spawn(move || {
            for line in BufReader::new(stdout).split(b'\n') {
                let Ok(line) = line else { break };
                let line = String::from_utf8_lossy(&line);
                if let Some(at) = line.find(PREFIX) {
                    if tx
                        .send(line[at + PREFIX.len()..].trim().to_string())
                        .is_err()
                    {
                        break;
                    }
                }
            }
        });
        let outcome = loop {
            match rx.recv_timeout(quiet) {
                Ok(msg) => {
                    let Ok(v) = serde_json::from_str::<Value>(&msg) else {
                        continue;
                    };
                    if let Some(p) = v.get("progress").and_then(Value::as_f64) {
                        progress(p.clamp(0.0, 1.0));
                    } else if let Some(done) = v.get("done") {
                        break Ok(done.clone());
                    } else if let Some(e) = v.get("error").and_then(Value::as_str) {
                        break Err(anyhow!("{e}"));
                    }
                }
                Err(RecvTimeoutError::Timeout) => {
                    let _ = child.kill();
                    break Err(anyhow!(
                        "a plugin stopped responding (no word for {} s), so it was stopped",
                        quiet.as_secs()
                    ));
                }
                Err(RecvTimeoutError::Disconnected) => {
                    let status = child.wait();
                    break Err(anyhow!(
                        "a plugin crashed{}",
                        match status {
                            Ok(s) => format!(" ({s})"),
                            Err(_) => String::new(),
                        }
                    ));
                }
            }
        };
        let _ = child.kill();
        let _ = child.wait();
        let _ = writer.join();
        let _ = reader.join();
        outcome
    }
}

/// Run as a worker if this program was started as one: call first thing in
/// `main`. Returns the exit code to end with.
pub fn run_if_worker() -> Option<i32> {
    (std::env::args_os().nth(1).as_deref() == Some(FLAG.as_ref())).then(serve)
}

/// Serve one job from stdin (see the module documentation).
pub fn serve() -> i32 {
    quiet_crashes();
    let mut input = String::new();
    if let Err(e) = std::io::stdin().read_to_string(&mut input) {
        say(&json!({ "error": format!("could not read the job: {e}") }));
        return 2;
    }
    let result = serde_json::from_str::<Job>(&input)
        .map_err(|e| anyhow!("bad job: {e}"))
        .and_then(|job| work(job, &mut |p| say(&json!({ "progress": p }))));
    match result {
        Ok(done) => {
            say(&json!({ "done": done }));
            0
        }
        Err(e) => {
            say(&json!({ "error": format!("{e:#}") }));
            1
        }
    }
}

/// Write one protocol line. The newline first ends any half line a plugin
/// may have printed.
fn say(v: &Value) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "\n{PREFIX}{v}");
    let _ = out.flush();
}

/// Do a job here, in this process.
pub fn work(job: Job, progress: &mut dyn FnMut(f64)) -> Result<Value> {
    match job {
        Job::Scan { path } => {
            let module = Module::load(Path::new(&path))?;
            Ok(json!({ "classes": module.classes() }))
        }
        Job::Render {
            project,
            out,
            sample_rate,
        } => {
            let (mut rack, problems) = rack::offline(&project, f64::from(sample_rate), &by_path);
            let mut last = 0.0;
            let seconds = render::render_to_wav_with(
                &project,
                Some(&mut rack),
                Path::new(&out),
                sample_rate,
                |p| {
                    if p - last >= 0.01 || p >= 1.0 {
                        last = p;
                        progress(p);
                    }
                },
            )?;
            Ok(json!({ "seconds": seconds, "problems": problems }))
        }
        Job::Analyze { project } => {
            let mut problems = Vec::new();
            // The song is played once for the mix and once per track; each
            // pass is a sign of life.
            let passes = 1 + project.tracks.len();
            let mut pass = 0;
            let measured = analysis::analyze_with(&project, |p| {
                progress(pass as f64 / passes as f64);
                pass += 1;
                let (rack, mut trouble) =
                    rack::offline(p, f64::from(analysis::ANALYSIS_SAMPLE_RATE), &by_path);
                trouble.retain(|t| !problems.contains(t));
                problems.extend(trouble);
                Some(Box::new(rack) as Box<dyn aidaw_core::rack::Rack>)
            })?;
            Ok(json!({ "analysis": measured, "problems": problems }))
        }
        Job::Params { plugin } => {
            let p = set_up(&plugin)?;
            Ok(json!({ "name": p.name, "params": p.params() }))
        }
        Job::SetParams { plugin, wanted } => {
            let p = set_up(&plugin)?;
            let set = params::resolve(&p, &wanted)?;
            Ok(json!({ "name": p.name, "set": set }))
        }
    }
}

/// Load `plugin` as the song has it: its saved settings, then the
/// parameters set on top.
fn set_up(plugin: &PluginRef) -> Result<Plugin> {
    let path = by_path(plugin)?;
    let (p, _processor) = Plugin::load(
        &path,
        &plugin.uid,
        Setup {
            sample_rate: 48_000.0,
            offline: true,
        },
        plugin.state.as_ref().map(|b| b.as_bytes()),
        plugin.controller_state.as_ref().map(|b| b.as_bytes()),
    )?;
    for (id, v) in plugin.param_values() {
        p.set_param(id, v);
    }
    Ok(p)
}

/// In a worker, a plugin's `path` says where it is: the parent found it,
/// or left it empty when it could not.
fn by_path(p: &PluginRef) -> Result<PathBuf> {
    if p.path.is_empty() {
        bail!(
            "{} is not installed on this computer (if it is, scan for plugins again)",
            p.name
        );
    }
    Ok(PathBuf::from(&p.path))
}

/// A crashing plugin should end the worker, not show a dialog nobody
/// will click (which would hang it until the parent gives up).
fn quiet_crashes() {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::System::Diagnostics::Debug::{
            SetErrorMode, SEM_FAILCRITICALERRORS, SEM_NOGPFAULTERRORBOX, SEM_NOOPENFILEERRORBOX,
        };
        SetErrorMode(SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX | SEM_NOOPENFILEERRORBOX);
    }
}
