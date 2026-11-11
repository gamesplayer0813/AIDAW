//! `aidaw-mcp`: lets an AI compose music by editing an AIDAW project file.
//!
//! * As a Model Context Protocol server (stdio, JSON-RPC 2.0) for MCP clients
//!   such as Claude Desktop or Cursor.
//! * As a one-shot command line (`aidaw-mcp call <tool> '<json>'`) for agents
//!   that can only run shell commands, plus `aidaw-mcp tools --format openai`
//!   to hand the same tools to any function-calling API.
//!
//! The GUI watches the same file and reloads it live, so a human sees every
//! change the AI makes and can fine-tune it (and undo it) in the editor.

mod session;
mod tools;

use std::io::{self, BufRead, Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Map, Value};

use session::Session;

/// MCP protocol versions this server speaks, newest first. They all share
/// the `initialize` handshake and the tool methods served here; a client
/// that also knows the handshake-free versions (2026-07-28 on) falls back
/// to `initialize` when `server/discover` is answered "method not found".
const PROTOCOL_VERSIONS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

/// The version to answer `initialize` with: the one the client asked for
/// when we speak it, else our newest, and the client decides whether to go
/// on (the MCP lifecycle rule). Echoing an unknown version would claim
/// support we don't have.
fn negotiate(requested: Option<&str>) -> &'static str {
    PROTOCOL_VERSIONS
        .iter()
        .find(|v| Some(**v) == requested)
        .unwrap_or(&PROTOCOL_VERSIONS[0])
}

const USAGE: &str = "\
usage:
  aidaw-mcp [--project FILE]                    run the MCP server on stdin/stdout
  aidaw-mcp call TOOL [ARGS] [--project FILE]   run one tool and print the result
        ARGS is a JSON object ('{\"name\": \"Bass\"}'), '-' to read it from stdin,
        or key=value pairs (name=Bass instrument=bass). Exit code 1 on error.
  aidaw-mcp tools [--format mcp|openai|anthropic|gemini]
                                                print the tool schemas as JSON
  aidaw-mcp guide                               print the cheat sheet
The project file can also be set with the AIDAW_PROJECT environment variable.";

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<ExitCode> {
    let mut project = std::env::var_os("AIDAW_PROJECT").map(PathBuf::from);
    // Resolved to an absolute path below: MCP clients start us with an
    // arbitrary working directory, and we may be asked for it later.
    let mut format = None;
    let mut positional = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--project" | "-p" => {
                project = Some(PathBuf::from(
                    args.next().context("--project needs a file path")?,
                ))
            }
            "--format" | "-f" => format = Some(args.next().context("--format needs a value")?),
            "--help" | "-h" | "help" if positional.is_empty() => {
                println!("{USAGE}");
                return Ok(ExitCode::SUCCESS);
            }
            _ => positional.push(a),
        }
    }
    let project = project.map(|p| std::path::absolute(&p).unwrap_or(p));
    let mut positional = positional.into_iter();
    match positional.next().as_deref() {
        None | Some("serve") => {
            serve(Session::new(project))?;
            Ok(ExitCode::SUCCESS)
        }
        Some("guide") => {
            println!("{}", tools::GUIDE);
            Ok(ExitCode::SUCCESS)
        }
        Some("tools") => {
            let format = format.as_deref().unwrap_or("mcp");
            let schemas = tools::export(format).ok_or_else(|| {
                anyhow!("unknown format {format:?}; use one of {}", tools::FORMATS)
            })?;
            println!("{}", serde_json::to_string_pretty(&schemas)?);
            Ok(ExitCode::SUCCESS)
        }
        Some("call") => {
            let tool = positional
                .next()
                .ok_or_else(|| anyhow!("call needs a tool name\n{USAGE}"))?;
            let args = cli_args(positional.collect())?;
            let mut session = Session::new(project);
            match call_guarded(&mut session, &tool, args) {
                Ok(text) => {
                    println!("{text}");
                    Ok(ExitCode::SUCCESS)
                }
                Err(e) => {
                    eprintln!("error: {e:#}");
                    Ok(ExitCode::FAILURE)
                }
            }
        }
        Some(other) => bail!("unknown command {other:?}\n{USAGE}"),
    }
}

/// Parse the arguments of `aidaw-mcp call`: a JSON object, `-` for stdin,
/// or `key=value` pairs (values starting with `{` or `[` are parsed as JSON).
fn cli_args(raw: Vec<String>) -> Result<Value> {
    match raw.as_slice() {
        [] => Ok(json!({})),
        [one] if one == "-" => {
            let mut s = String::new();
            io::stdin().read_to_string(&mut s)?;
            serde_json::from_str(&s).context("stdin must contain a JSON object")
        }
        [one] if one.trim_start().starts_with('{') => serde_json::from_str(one).with_context(|| {
            "arguments are not valid JSON; quote them for the shell, e.g. '{\"name\": \"Bass\"}'"
        }),
        pairs => {
            let mut map = Map::new();
            for pair in pairs {
                let (k, v) = pair.split_once('=').ok_or_else(|| {
                    anyhow!("expected a JSON object or key=value pairs, got {pair:?}")
                })?;
                let v = if v.starts_with(['{', '[']) {
                    serde_json::from_str(v).with_context(|| format!("value of {k} is not valid JSON"))?
                } else {
                    Value::String(v.to_string())
                };
                map.insert(k.to_string(), v);
            }
            Ok(Value::Object(map))
        }
    }
}

fn serve(mut session: Session) -> Result<()> {
    let stdin = io::stdin();
    let mut stdout = io::stdout().lock();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<Value>(&line) {
            Ok(Value::Array(batch)) if batch.is_empty() => Some(error_response(
                Value::Null,
                -32600,
                "invalid request: empty batch",
            )),
            // A JSON-RPC batch (MCP 2025-03-26): one array with the answers
            // to its requests, nothing when it held only notifications.
            Ok(Value::Array(batch)) => {
                let replies: Vec<Value> = batch
                    .into_iter()
                    .filter_map(|msg| handle(&mut session, msg))
                    .collect();
                (!replies.is_empty()).then_some(Value::Array(replies))
            }
            Ok(msg) => handle(&mut session, msg),
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

/// Run a tool, turning a panic (a bug) into an ordinary error so that one bad
/// call can never take the server down.
fn call_guarded(session: &mut Session, name: &str, args: Value) -> Result<String> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| session.call(name, args)))
        .unwrap_or_else(|panic| {
            let msg = panic
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| panic.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "unknown panic".into());
            Err(anyhow!(
                "internal error in {name}: {msg}. The project file was not changed by this call; please report this bug"
            ))
        })
}

fn error_response(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

fn handle(session: &mut Session, msg: Value) -> Option<Value> {
    if !msg.is_object() {
        return Some(error_response(
            Value::Null,
            -32600,
            "invalid request: expected a JSON-RPC object",
        ));
    }
    // Notifications (no id) never get a response.
    let id = msg.get("id")?.clone();
    let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
    let params = msg.get("params").cloned().unwrap_or(Value::Null);
    let result = match method {
        "initialize" => {
            let version = negotiate(params.get("protocolVersion").and_then(Value::as_str));
            json!({
                "protocolVersion": version,
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "aidaw", "version": env!("CARGO_PKG_VERSION")},
                "instructions": tools::INSTRUCTIONS,
            })
        }
        "ping" => json!({}),
        "tools/list" => json!({"tools": tools::export("mcp").expect("mcp format")}),
        // Some clients probe these even though only tools are advertised.
        "resources/list" => json!({"resources": []}),
        "prompts/list" => json!({"prompts": []}),
        "tools/call" => {
            let name = params
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let args = params.get("arguments").cloned().unwrap_or(Value::Null);
            let (text, is_error) = match call_guarded(session, &name, args) {
                Ok(text) => (text, false),
                Err(e) => (format!("error: {e:#}"), true),
            };
            json!({"content": [{"type": "text", "text": text}], "isError": is_error})
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_versions_are_echoed_and_unknown_ones_get_the_newest() {
        for v in PROTOCOL_VERSIONS {
            assert_eq!(negotiate(Some(v)), *v);
        }
        assert_eq!(negotiate(Some("2099-01-01")), PROTOCOL_VERSIONS[0]);
        assert_eq!(negotiate(Some("")), PROTOCOL_VERSIONS[0]);
        assert_eq!(negotiate(None), PROTOCOL_VERSIONS[0]);
    }
}
