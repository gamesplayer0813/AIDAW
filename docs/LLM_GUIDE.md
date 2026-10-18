# Using AIDAW with any LLM

AIDAW's AI interface is one program, `aidaw-mcp` (`aidaw-mcp.exe` on Windows). It edits a
`.aidaw.json` project file; the AIDAW app watches the same file, so you see every change live
and can undo it with `Ctrl+Z`.

It works three ways:

| Your setup | Use |
|---|---|
| An MCP client (Claude Desktop, Claude Code, Cursor, Windsurf, Cline, Zed, ...) | `aidaw-mcp` as an MCP server |
| A model behind a function-calling API (OpenAI/ChatGPT, Anthropic, Gemini, Ollama, LM Studio, vLLM, ...) | `aidaw-mcp tools --format ...` + `aidaw-mcp call ...` |
| An agent that can only run shell commands | `aidaw-mcp call ...` |

All three expose the same tools with the same arguments.

## The tools

| Tool | What it does |
|---|---|
| `help` | Cheat sheet: time, pitch, notations, and a complete quick-start song |
| `new_project` / `open_project` | Create or open a project file |
| `get_project` | Overview of tracks and clips; `detail: "notes"` lists every note, `"json"` gives the raw file |
| `list_presets` | Instrument presets: `soft_keys`, `bass`, `warm_pad`, `bright_lead`, `pluck`, `bell`, `chiptune`, `pure_tone`, `drum_kit` |
| `add_chords` | Chord symbols, e.g. `"Am F C G"`, one per bar. Styles: block, arpeggio, pulse |
| `add_drum_pattern` | Step strings, e.g. `{"kick": "x...x...x...x...", "snare": "....x.......x..."}` |
| `add_melody` | Compact text, e.g. `"C4/1 D4/0.5 E4/0.5 r/1 G4/2"` (pitch/beats, `r` = rest) |
| `add_track`, `update_track`, `remove_track` | Tracks, instruments, volume, pan, mute and solo, plus synth tweaks |
| `add_clip`, `update_clip`, `remove_clip` | Clips (blocks of notes on a track) |
| `add_notes`, `remove_notes`, `transpose_clip` | Note-level edits |
| `update_project` | Name, tempo, time signature, master volume |
| `apply_ops` | Several edits as one all-or-nothing step |
| `render_wav` | Render the song to a WAV file |

The musical tools (`add_chords`, `add_drum_pattern`, `add_melody`) do the note maths for you.
Give them a `track` (a name such as `"Bass"`) and optionally a `bar` (1 = start of the song).
If the track does not exist, they create it when its name matches a preset ("Drums", "Bass",
"Piano", "Pad", "Lead", ...) or when you pass `instrument`. They create or extend the clip as needed.

The server accepts more than the schemas strictly require, because smaller models make small
mistakes:

- tracks and clips can be named by name or id
- numbers can be sent as strings
- an instrument can be a preset name (`"Warm Pad"`, `"piano"`, `"drums"`)
- positions can be given in bars
- notes can be a melody string

Every error says what went wrong, lists the valid values (existing tracks, presets, drum names)
and shows an example call, so the model can fix the call and try again.

Paths: a relative `path` or `output_path` is taken relative to the open project's folder (or the
user's Music folder when no project is open), never the server's working directory, and results
report the absolute path. The server only ever writes `*.aidaw.json` project files and `*.wav`
renders; `new_project` completes `"song"` to `song.aidaw.json`, and asking it to overwrite any other
kind of file is refused. If the human saves the project in the AIDAW app while a tool call is
running, the call is replayed on the fresh file instead of overwriting their change.

## 1. MCP clients

Build it (`cargo build --release`) and point your client at the binary. Adding
`--project` sets the file to work on; without it, the model calls `new_project` or
`open_project` first.

**Claude Desktop**: edit `claude_desktop_config.json` (Settings → Developer → Edit Config):

```json
{
  "mcpServers": {
    "aidaw": {
      "command": "C:\\path\\to\\aidaw-mcp.exe",
      "args": ["--project", "C:\\Music\\song.aidaw.json"]
    }
  }
}
```

**Claude Code**:

```sh
claude mcp add aidaw -- /path/to/aidaw-mcp --project /path/to/song.aidaw.json
```

**Cursor**: in `.cursor/mcp.json` (project) or `~/.cursor/mcp.json` (global), use the same
`mcpServers` block as Claude Desktop. **Windsurf, Cline, Zed and other MCP clients** take the
same command and args in their own MCP settings.

The server sends usage instructions when it starts. Most clients add these to the model's context
automatically. If the model seems lost, ask it to call the `help` tool.

## 2. Function-calling APIs (ChatGPT-style)

Print the tool definitions in the format your API wants:

```sh
aidaw-mcp tools --format openai     > tools.json   # OpenAI, Ollama, LM Studio, vLLM, Groq, OpenRouter...
aidaw-mcp tools --format anthropic  > tools.json   # Anthropic Messages API
aidaw-mcp tools --format gemini     > tools.json   # Google Gemini (function_declarations)
aidaw-mcp tools --format mcp        > tools.json   # raw MCP shape
```

When the model calls a tool, run it and send back whatever the command prints. Here is a complete
loop with the OpenAI Python SDK, which works with any OpenAI-compatible endpoint:

```python
import json, subprocess
from openai import OpenAI

PROJECT = "song.aidaw.json"
tools = json.loads(subprocess.check_output(["aidaw-mcp", "tools", "--format", "openai"]))
guide = subprocess.check_output(["aidaw-mcp", "guide"], text=True)

def run_tool(name, args_json):
    p = subprocess.run(["aidaw-mcp", "call", name, args_json, "--project", PROJECT],
                       capture_output=True, text=True)
    return p.stdout if p.returncode == 0 else p.stderr   # errors teach the model how to retry

client = OpenAI()  # local model: OpenAI(base_url="http://localhost:11434/v1", api_key="ollama")
messages = [{"role": "system", "content": guide},
            {"role": "user", "content": f"Create {PROJECT} and write a 4-bar lo-fi loop, then render it."}]
while True:
    msg = client.chat.completions.create(model="gpt-4o-mini", messages=messages, tools=tools).choices[0].message
    messages.append(msg)
    if not msg.tool_calls:
        print(msg.content)
        break
    for call in msg.tool_calls:
        messages.append({"role": "tool", "tool_call_id": call.id,
                         "content": run_tool(call.function.name, call.function.arguments)})
```

Use the output of `aidaw-mcp guide` as the system prompt. It is short and includes a full
example song, which helps small models (Llama, Qwen, GPT-4o-mini) a lot.

## 3. Shell-only agents

Each `call` runs one tool and exits. It prints the result to stdout and exits with `0`, or prints
an error to stderr and exits with `1`. Set the project file once with `AIDAW_PROJECT` or pass
`--project FILE` on every call. Arguments can be JSON or `key=value` pairs:

```sh
export AIDAW_PROJECT=song.aidaw.json
aidaw-mcp call new_project '{"path": "song.aidaw.json", "bpm": 90}'
aidaw-mcp call add_drum_pattern '{"track": "Drums", "bars": 4, "pattern": {"kick": "x...x...x...x...", "snare": "....x.......x...", "hat": "x.x.x.x.x.x.x.x."}}'
aidaw-mcp call add_chords track=Keys chords="Am F C G" style=arpeggio
aidaw-mcp call add_melody track=Bass notes="A2/4 F2/4 C2/4 G2/4"
aidaw-mcp call get_project detail=notes
aidaw-mcp call render_wav
```

On Windows `cmd.exe`, write JSON with escaped double quotes (`"{\"track\": \"Bass\"}"`), or use
`key=value` pairs, or pipe the JSON in with `-`: `echo {"bpm": 90} | aidaw-mcp call update_project -`.

## Music in one minute

- **Time** is counted in beats; 1 beat is a quarter note. In 4/4 a bar has 4 beats, so bar 5
  starts at beat 16.
- **Pitch** is written as a note name with an octave. `C4` is middle C (MIDI 60). Bass sits
  around `C2`, chords around `C4`, melodies between `C4` and `C6`.
- **Drums** are written by name: `kick`, `snare`, `clap`, `hat`, `open_hat`, `tom_low`,
  `tom_mid`, `tom_high`, `crash`, `ride`. Each pattern has 16 steps per bar.
- **Chords** are written as symbols: `C Am F#m Bb G7 Cmaj7 Dm7 Bm7b5 Esus4 Cdim Caug C/E`.
  `C6/9 C9 C13 G7b9 Dm11 Cmaj7#11` also work. `Am:2` lasts 2 beats.
