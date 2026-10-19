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

All three expose the same tools with the same arguments. A chat AI with no tools at all can
still write notes for you: see [4. Plain chat](#4-plain-chat-paste-the-answer).

## The tools

| Tool | What it does |
|---|---|
| `help` | Cheat sheet: time, pitch, notations, and a complete quick-start song |
| `new_project` / `open_project` | Create or open a project file |
| `get_project` | Overview of tracks and clips; `detail: "notes"` lists every note, `"json"` gives the raw file |
| `list_presets` | The 78 instrument presets, grouped by category (see [Sounds](#sounds)); `{"category": "bass"}` lists one group |
| `add_chords` | Chord symbols, e.g. `"Am F C G"`, one per bar. Styles: block, arpeggio, pulse |
| `add_drum_pattern` | Step strings, e.g. `{"kick": "x...x...x...x...", "snare": "....x.......x..."}` |
| `add_melody` | Compact text, e.g. `"C4/1 D4/0.5 E4/0.5 r/1 G4/2"` (pitch/beats, `r` = rest) |
| `add_track`, `update_track`, `remove_track` | Tracks, instruments, volume, pan, reverb, mute and solo, plus synth tweaks |
| `add_clip`, `update_clip`, `remove_clip` | Clips (blocks of notes on a track) |
| `add_notes`, `remove_notes`, `transpose_clip` | Note-level edits |
| `update_project` | Name, tempo, time signature, master volume, reverb room |
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
- an instrument can be a preset name or a loose word (`"Warm Pad"`, `"piano"`, `"rhodes"`,
  `"violin"`, `"808"`, `"drums"`)
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

## Sounds

`list_presets` prints every preset with a one-line hint. Pass the id as `instrument`.

| Group | Presets |
|---|---|
| keys | `soft_keys`, `bright_piano`, `e_piano`, `wurly`, `organ`, `church_organ`, `clavinet`, `harpsichord`, `lofi_keys` |
| bass | `bass`, `sub_bass`, `bass_808`, `synth_bass`, `acid_bass`, `reese_bass`, `pluck_bass`, `finger_bass`, `growl_bass` |
| lead | `bright_lead`, `supersaw`, `analog_lead`, `square_lead`, `pulse_lead`, `soft_lead`, `hard_lead`, `pure_tone` |
| pad | `warm_pad`, `lush_pad`, `glass_pad`, `dark_pad`, `choir`, `airy_pad`, `sweep_pad` |
| pluck | `pluck`, `synth_pluck`, `nylon_guitar`, `clean_guitar`, `rock_guitar`, `harp`, `koto` |
| bells | `bell`, `music_box`, `marimba`, `xylophone`, `vibraphone`, `glockenspiel`, `kalimba`, `tubular_bells`, `steel_drum` |
| orchestra | `strings`, `cello`, `pizzicato`, `brass`, `trumpet`, `french_horn`, `saxophone`, `flute`, `pan_flute`, `clarinet`, `oboe` |
| retro | `chiptune`, `chip_pulse`, `chip_bass`, `game_lead`, `retro_synth` |
| fx | `riser`, `zap`, `boom`, `wind`, `theremin`, `magic` |
| drums | `drum_kit`, `drums_808`, `drums_909`, `drums_acoustic`, `drums_lofi`, `drums_trap`, `drums_electro` |

**Drum kits.** Every kit plays the same drum names, so a pattern works on any kit. Switch kit
with `update_track {"track": "Drums", "kit": "909"}` (`standard`, `808`, `909`, `acoustic`,
`lofi`, `trap`, `electro`). Besides the drum set, every kit has percussion: `rim`, `ride_bell`,
`tambourine`, `splash`, `cowbell`, `bongo_high`, `bongo_low`, `conga_mute`, `conga_high`,
`conga_low`, `timbale_high`, `timbale_low`, `agogo_high`, `agogo_low`, `shaker`, `claves`,
`block_high`, `block_low`, `triangle`.

**Shaping a synth.** `update_track` (and `add_track`) take these on any synth track; anything
left out keeps its value:

| Setting | Range | What it does |
|---|---|---|
| `waveform` | `sine`, `triangle`, `saw`, `square`, `pulse`, `noise` | Basic tone |
| `attack`, `decay`, `sustain`, `release` | seconds, sustain 0-1 | Volume envelope |
| `cutoff` | 40-20000 Hz | Brightness (low-pass filter) |
| `resonance` | 0-1 | Whistling peak at the cutoff (acid, wah) |
| `filter_env`, `filter_decay` | 0-8 octaves, seconds | Each note starts brighter and closes (plucks, brass) |
| `unison`, `detune` | 1-5 voices, cents | Stacked detuned copies: wider and fatter |
| `sub` | 0-1 | Sine one octave below |
| `noise` | 0-1 | Breath, air, hiss |
| `fm`, `fm_ratio`, `fm_decay` | 0-10, ratio, seconds | FM tone for sine/triangle: electric piano (ratio 1), bells (3.5) |
| `vibrato`, `vibrato_rate` | cents, Hz | Pitch wobble that eases in |
| `pitch_env`, `pitch_decay` | semitones, seconds | Notes start higher and slide down (808 bass, zaps); negative slides up |
| `drive` | 0-1 | Warmth to distortion |

Example: `update_track {"track": "Lead", "instrument": "analog_lead", "resonance": 0.7, "vibrato": 25}`.

**Reverb.** All tracks share one reverb room. A track's `reverb` (0-1) says how much of it goes
there: 0 is dry, 0.2-0.35 suits keys, pads and leads, about 0.1 suits drums, and basses usually
stay at 0. New tracks start with their preset's usual amount (pads, bells, orchestra and fx 0.3,
plucks 0.25, keys and leads 0.2, drums and retro 0.1, basses 0); custom sounds start dry. Set it
with `update_track {"track": "Lead", "reverb": 0.35}`. `update_project {"reverb_size": 0.7,
"reverb_tone": 0.4}` changes the room itself: size 0 is a small room, 1 a big hall (longer
tail); tone 0 is dark, 1 bright. `get_project` shows every track's reverb and the room settings.

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

## 4. Plain chat: paste the answer

No setup: ask any chat AI for a JSON list of notes, copy its whole answer, click a clip in the
AIDAW app and press `Ctrl+V`. The app finds the list in the answer (words and a ```` ``` ````
fence around it are fine), keeps each note's place in the bar and pastes it into the bar where
the playhead is. The **Ask AI** window has a ready-made request to copy. A note looks like:

```json
[{"pitch": "A4", "start": 0, "duration": 1, "velocity": 100}]
```

`start` and `duration` are in beats, `pitch` is a note name or a MIDI number, and `velocity`
(1-127) is optional. `{"notes": [...]}` works too. Notes copied in the app with `Ctrl+C` are
plain JSON as well, so they can be pasted into a chat for the AI to vary.

## Music in one minute

- **Time** is counted in beats; 1 beat is a quarter note. In 4/4 a bar has 4 beats, so bar 5
  starts at beat 16.
- **Pitch** is written as a note name with an octave. `C4` is middle C (MIDI 60). Bass sits
  around `C2`, chords around `C4`, melodies between `C4` and `C6`.
- **Drums** are written by name: `kick`, `snare`, `clap`, `hat`, `open_hat`, `tom_low`,
  `tom_mid`, `tom_high`, `crash`, `ride`, plus percussion such as `cowbell`, `shaker`,
  `tambourine` and `conga_high`. Each pattern has 16 steps per bar.
- **Chords** are written as symbols: `C Am F#m Bb G7 Cmaj7 Dm7 Bm7b5 Esus4 Cdim Caug C/E`.
  `C6/9 C9 C13 G7b9 Dm11 Cmaj7#11` also work. `Am:2` lasts 2 beats.
