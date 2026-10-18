//! The tool catalogue: names, descriptions with an inline example, and flat
//! JSON schemas. Schemas deliberately avoid `oneOf`, type unions and nesting
//! where possible so that small models and strict function-calling APIs
//! (OpenAI, Gemini) accept them; the server is more forgiving than the schema.

use serde_json::{json, Map, Value};

/// Sent in the MCP `initialize` response; most clients put it in the system prompt.
pub const INSTRUCTIONS: &str = "AIDAW is a music editor (DAW). You compose by calling tools that edit a \
project file; a human may watch and edit the same project in the AIDAW app.
Basics: time is in beats (1 = quarter note); in 4/4 a bar is 4 beats and `bar: 1` is the start of the song. \
Pitches are names like \"C4\" (middle C = 60), \"F#3\", \"Bb2\" or MIDI numbers. Tracks and clips can be \
referred to by name (\"Bass\") or id (\"t1\", \"c1\").
Easiest workflow: new_project -> add_track with a preset (list_presets) -> add_chords / add_melody / \
add_drum_pattern (they create clips for you) -> get_project to check -> render_wav to listen.
Every error message says what was wrong and shows a valid example; read it and retry. Call help for the \
full cheat sheet.";

/// Returned by the `help` tool and `aidaw-mcp guide`.
pub const GUIDE: &str = r#"AIDAW cheat sheet
=================
TIME   Beats (1 = quarter note). 4/4 => 4 beats per bar. bar 1 = beat 0, bar 2 = beat 4, bar 5 = beat 16.
       Clip positions are absolute; note starts are relative to their clip.
PITCH  "C4" = 60 = middle C. Sharps "F#3", flats "Bb2". Bass lives around C2, chords around C4, melody C4-C6.
NAMES  Tracks/clips can be given by name or id: {"track": "Bass"} or {"track": "t1"}.
DRUMS  kick 36, snare 38, clap 39, hat 42, open_hat 46, tom_low 41, tom_mid 45, tom_high 48, crash 49, ride 51.
       Percussion: rim 37, ride_bell 53, tambourine 54, splash 55, cowbell 56, bongo_high 60, bongo_low 61,
       conga_mute 62, conga_high 63, conga_low 64, timbale_high 65, timbale_low 66, agogo_high 67,
       agogo_low 68, shaker 70, claves 75, block_high 76, block_low 77, triangle 81.
SOUNDS 77 presets in 10 groups (list_presets): keys, bass, lead, pad, pluck, bells, orchestra, retro,
       fx, drums. Drum kits: drum_kit, drums_808, drums_909, drums_acoustic, drums_lofi, drums_trap,
       drums_electro.

PATHS  Relative paths are taken relative to the open project's folder (with no project open: the
       user's Music folder). Results print the absolute path. Only *.aidaw.json (projects) and
       *.wav (renders) are ever written.

QUICK START (4-bar loop)
  new_project      {"path": "song.aidaw.json", "bpm": 100}
  add_track        {"name": "Drums", "instrument": "drum_kit"}
  add_drum_pattern {"track": "Drums", "bars": 4, "pattern": {"kick": "x...x...x...x...", "snare": "....x.......x...", "hat": "x.x.x.x.x.x.x.x."}}
  add_track        {"name": "Keys", "instrument": "soft_keys"}
  add_chords       {"track": "Keys", "chords": "Am F C G"}
  add_track        {"name": "Bass", "instrument": "bass"}
  add_melody       {"track": "Bass", "notes": "A2/4 F2/4 C2/4 G2/4"}
  add_track        {"name": "Lead", "instrument": "bright_lead"}
  add_melody       {"track": "Lead", "bar": 1, "notes": "E5/1 D5/0.5 C5/0.5 A4/2 r/4 C5/1 D5/1 E5/2 G5/4"}
  render_wav       {}

NOTATIONS
  chords   "Am F C G" = one chord per bar. "Am:2 F:2" = 2 beats each. "r" = rest.
           Qualities: C Am F#m Bb G7 Cmaj7 Dm7 Bm7b5 Esus4 Asus2 Cdim Caug C6 C6/9 Cadd9 C9 C11 C13
           G7b9 G7#9 Dm11 Cmaj7#11 CmMaj7 C5, slash C/E.
           style: block (held), arpeggio (eighth notes), pulse (every beat). octave: 4 = around middle C.
  melody   "PITCH/BEATS" tokens: "C4/1 D4/0.5 E4/0.5 r/1 G4/2". r = rest. "C4+E4+G4/2" = chord.
           Duration may be omitted (repeats the previous one). "@80" sets velocity: "C4/1@80".
  drums    16 steps per bar (sixteenth notes): x = hit, X = accent, o = soft, . = rest.
           A shorter pattern repeats: "x." = every eighth note. bars = how many bars to fill.

PLACEMENT (add_chords, add_melody, add_drum_pattern)
  Give "track" and optionally "bar". Without "bar" the music goes after the track's last clip
  (bar 1 on an empty track). If a clip already covers that bar, notes are added into it; otherwise
  a new clip is created. Give "clip" to target a specific clip. A missing track is created only
  if its name is a preset ("Drums", "Bass", "Piano", "Pad", "Lead"...) or you pass "instrument".
  Tracks can also be given by position: "2" or "Track 2". Songs are at most 4096 bars.

LOW-LEVEL TOOLS
  add_clip {"track": "Lead", "bar": 5, "bars": 2, "notes": "C5/1 E5/1"}  (notes may also be objects:
           [{"pitch": "C5", "start": 0, "duration": 1, "velocity": 100}])
  add_notes, remove_notes, transpose_clip, update_clip, update_track, remove_*, update_project.
  apply_ops {"ops": [{"op": "add_track", "name": "Pad", "instrument": "warm_pad"},
                     {"op": "add_chords", "track": "Pad", "chords": "Am F"}]}  -- all or nothing.

CHECK YOUR WORK
  get_project {}                    compact overview (tracks, clips, bars)
  get_project {"detail": "notes"}   every note as start:PITCH/BEATS
  get_project {"detail": "json"}    raw project JSON
"#;

#[derive(Clone, Copy)]
pub enum Ty {
    Str,
    Num,
    Int,
    Bool,
    /// Array of note objects (a melody string is also accepted).
    Notes,
    /// Drum lanes: object of drum name -> step string.
    Drums,
    /// Array of operation objects for `apply_ops`.
    Ops,
}

pub struct Param {
    pub name: &'static str,
    pub ty: Ty,
    pub desc: &'static str,
    pub required: bool,
}

pub struct Tool {
    pub name: &'static str,
    pub description: &'static str,
    /// Example arguments, shown in the description and in error messages.
    pub example: &'static str,
    pub params: &'static [Param],
}

const fn p(name: &'static str, ty: Ty, desc: &'static str) -> Param {
    Param {
        name,
        ty,
        desc,
        required: false,
    }
}

const fn req(name: &'static str, ty: Ty, desc: &'static str) -> Param {
    Param {
        name,
        ty,
        desc,
        required: true,
    }
}

const TRACK: &str = "track name or id, e.g. \"Bass\" or \"t1\"";
const CLIP: &str = "clip name or id, e.g. \"c1\" (ids are shown by get_project)";
const BAR: &str = "1-based bar to start at (bar 1 = beginning of the song)";
const INSTRUMENT: &str = "preset id (77 of them, see list_presets), e.g. soft_keys, e_piano, organ, bass, \
                          bass_808, acid_bass, bright_lead, supersaw, warm_pad, lush_pad, choir, pluck, \
                          nylon_guitar, rock_guitar, marimba, bell, strings, brass, flute, saxophone, \
                          chiptune, riser, drum_kit, drums_808, drums_909, drums_acoustic, drums_lofi, \
                          drums_trap. Loose names work too (\"rhodes\", \"violin\")";
const KIT: &str = "drum tracks only: standard | 808 | 909 | acoustic | lofi | trap | electro";
const NOTES: &str = "notes as a melody string like \"C4/1 E4/0.5 G4/0.5\" (PITCH/BEATS, r = rest), or an \
                     array of {pitch, start, duration, velocity} with start in beats from the clip start";

use Ty::*;

pub const TOOLS: &[Tool] = &[
    Tool {
        name: "help",
        description: "Return the AIDAW cheat sheet: time, pitch, notations, placement rules and a complete quick-start song. Call this first if unsure.",
        example: "{}",
        params: &[],
    },
    Tool {
        name: "new_project",
        description: "Create a new empty project file and make it the current project.",
        example: r#"{"path": "song.aidaw.json", "name": "My Song", "bpm": 100}"#,
        params: &[
            req("path", Str, "file path ending in .aidaw.json (relative = next to the current project, else the Music folder)"),
            p("name", Str, "song title"),
            p("bpm", Num, "tempo, 20-400 (default 120)"),
            p("overwrite", Bool, "replace the file if it exists"),
        ],
    },
    Tool {
        name: "open_project",
        description: "Open an existing .aidaw.json project and make it current. Returns a summary.",
        example: r#"{"path": "song.aidaw.json"}"#,
        params: &[req("path", Str, "file path")],
    },
    Tool {
        name: "get_project",
        description: "Show the current project. detail: \"summary\" (default: tracks, clips, bars), \"notes\" (also every note) or \"json\" (raw file).",
        example: r#"{"detail": "notes", "track": "Bass"}"#,
        params: &[
            p("detail", Str, "summary | notes | json"),
            p("track", Str, "only show this track (name or id)"),
        ],
    },
    Tool {
        name: "list_presets",
        description: "List the instrument presets (ids like \"e_piano\", \"bass_808\", \"strings\", \"drums_808\") usable as `instrument` in add_track/update_track, grouped by category.",
        example: r#"{"category": "bass"}"#,
        params: &[p(
            "category",
            Str,
            "only this group: keys | bass | lead | pad | pluck | bells | orchestra | retro | fx | drums",
        )],
    },
    Tool {
        name: "update_project",
        description: "Change the project name, bpm (20-400), beats_per_bar or master_gain (0-2).",
        example: r#"{"bpm": 92}"#,
        params: &[
            p("name", Str, "song title"),
            p("bpm", Num, "tempo, 20-400"),
            p("beats_per_bar", Int, "time signature numerator, e.g. 4 or 3"),
            p("master_gain", Num, "0-2, 1 = unity"),
        ],
    },
    Tool {
        name: "add_track",
        description: "Add a track with an instrument preset. Returns its track_id. Without instrument, a preset matching the name is used (\"Drums\" -> drum_kit), else a saw synth.",
        example: r#"{"name": "Bass", "instrument": "bass"}"#,
        params: &[
            req("name", Str, "track name, e.g. \"Bass\""),
            p("instrument", Str, INSTRUMENT),
            p("kit", Str, KIT),
            p("gain", Num, "volume 0-2 (default 0.8)"),
            p("pan", Num, "-1 left .. 1 right"),
        ],
    },
    Tool {
        name: "update_track",
        description: "Change a track's name, instrument preset, drum kit, volume, pan, mute/solo, or tweak its synth sound.",
        example: r#"{"track": "Bass", "instrument": "pluck", "gain": 0.6, "cutoff": 1200}"#,
        params: &[
            req("track", Str, TRACK),
            p("name", Str, "new name"),
            p("instrument", Str, INSTRUMENT),
            p("gain", Num, "volume 0-2, 1 = unity"),
            p("pan", Num, "-1 left .. 1 right"),
            p("mute", Bool, "silence the track"),
            p("solo", Bool, "play only soloed tracks"),
            p("kit", Str, KIT),
            p("waveform", Str, "synth only: sine | triangle | saw | square | pulse | noise"),
            p("attack", Num, "synth only: seconds"),
            p("decay", Num, "synth only: seconds"),
            p("sustain", Num, "synth only: level 0-1"),
            p("release", Num, "synth only: seconds"),
            p("cutoff", Num, "synth only: low-pass cutoff in Hz (lower = darker)"),
            p("resonance", Num, "synth only: 0-1, whistling peak at the cutoff (acid, wah)"),
            p("filter_env", Num, "synth only: octaves the cutoff jumps up at each note, 0-8 (plucks, brass)"),
            p("filter_decay", Num, "synth only: seconds for that filter jump to fall back"),
            p("unison", Int, "synth only: 1-5 stacked detuned voices (wider, fatter)"),
            p("detune", Num, "synth only: unison spread in cents, e.g. 20"),
            p("sub", Num, "synth only: 0-1 sine one octave below"),
            p("noise", Num, "synth only: 0-1 noise mixed in (breath, air)"),
            p("fm", Num, "synth only: 0-10 FM amount for sine/triangle (electric piano, bells)"),
            p("fm_ratio", Num, "synth only: FM modulator ratio; 1 warm, 2 hollow, 3.5 metallic"),
            p("fm_decay", Num, "synth only: seconds the FM tone takes to fade (0 = never)"),
            p("vibrato", Num, "synth only: pitch wobble depth in cents, e.g. 15"),
            p("vibrato_rate", Num, "synth only: vibrato speed in Hz (default 5)"),
            p("pitch_env", Num, "synth only: semitones above the note each note starts at, sliding down"),
            p("pitch_decay", Num, "synth only: seconds for that pitch slide"),
            p("drive", Num, "synth only: 0-1 saturation / distortion"),
        ],
    },
    Tool {
        name: "remove_track",
        description: "Delete a track and all its clips.",
        example: r#"{"track": "Bass"}"#,
        params: &[req("track", Str, TRACK)],
    },
    Tool {
        name: "add_clip",
        description: "Add a clip (a block of notes) to a track at a bar or beat, optionally with notes. Returns clip_id. Length defaults to fit the notes.",
        example: r#"{"track": "Lead", "bar": 1, "bars": 2, "notes": "C5/1 E5/1 G5/2"}"#,
        params: &[
            req("track", Str, TRACK),
            p("bar", Num, BAR),
            p("start", Num, "absolute start in beats (alternative to bar)"),
            p("bars", Num, "length in bars"),
            p("length", Num, "length in beats (alternative to bars)"),
            p("name", Str, "clip name"),
            p("notes", Notes, NOTES),
        ],
    },
    Tool {
        name: "update_clip",
        description: "Move, resize or rename a clip.",
        example: r#"{"clip": "c2", "bar": 5, "bars": 4}"#,
        params: &[
            req("clip", Str, CLIP),
            p("name", Str, "new name"),
            p("bar", Num, "move to this 1-based bar"),
            p("start", Num, "move to this absolute beat"),
            p("bars", Num, "new length in bars"),
            p("length", Num, "new length in beats"),
        ],
    },
    Tool {
        name: "remove_clip",
        description: "Delete a clip.",
        example: r#"{"clip": "c2"}"#,
        params: &[req("clip", Str, CLIP)],
    },
    Tool {
        name: "add_notes",
        description: "Add notes to an existing clip. Note starts are beats from the clip start.",
        example: r#"{"clip": "c1", "notes": [{"pitch": "E4", "start": 2, "duration": 1}]}"#,
        params: &[req("clip", Str, CLIP), req("notes", Notes, NOTES)],
    },
    Tool {
        name: "remove_notes",
        description: "Remove notes from a clip whose start is in [from, to) beats and that match pitch. All filters optional; none clears the clip.",
        example: r#"{"clip": "c1", "from": 4, "to": 8, "pitch": "C4"}"#,
        params: &[
            req("clip", Str, CLIP),
            p("from", Num, "beat (relative to clip), inclusive"),
            p("to", Num, "beat (relative to clip), exclusive"),
            p("pitch", Str, "only this pitch, e.g. \"C4\" or 60"),
        ],
    },
    Tool {
        name: "transpose_clip",
        description: "Shift every note in a clip up or down by semitones (12 = one octave).",
        example: r#"{"clip": "c1", "semitones": -12}"#,
        params: &[
            req("clip", Str, CLIP),
            req("semitones", Int, "e.g. 2, -5, 12"),
        ],
    },
    Tool {
        name: "add_chords",
        description: "Write a chord progression from chord symbols; one chord per bar unless \"Am:2\" gives beats. Voiced around middle C. Creates the clip (and track) if needed.",
        example: r#"{"track": "Keys", "chords": "Am F C G", "bar": 1, "style": "block"}"#,
        params: &[
            req("track", Str, "track name, id or position; a missing track is created if the name is a preset (Piano, Pad...) or instrument is given"),
            req("chords", Str, "e.g. \"Am F C G\", \"Dm7:2 G7:2 Cmaj7\", \"C/E\"; r = rest"),
            p("bar", Num, "1-based bar to start at (default: after the track's last clip)"),
            p("beats_per_chord", Num, "default: one bar"),
            p("style", Str, "block (held, default) | arpeggio (eighth notes) | pulse (every beat)"),
            p("octave", Int, "4 = around middle C (default), 3 lower, 5 higher"),
            p("velocity", Int, "1-127 (default 90)"),
            p("clip", Str, "add into this existing clip instead"),
            p("instrument", Str, "preset id used if the track has to be created, e.g. \"warm_pad\""),
        ],
    },
    Tool {
        name: "add_drum_pattern",
        description: "Write a drum beat from step strings, 16 steps per bar: x = hit, X = accent, o = soft, . = rest. Shorter strings repeat. Creates the clip (and a drum track) if needed.",
        example: r#"{"track": "Drums", "bars": 4, "pattern": {"kick": "x...x...x...x...", "snare": "....x.......x...", "hat": "x.x.x.x.x.x.x.x."}}"#,
        params: &[
            p("track", Str, "drum track name or id (default \"Drums\", created if missing)"),
            req("pattern", Drums, "drum name -> steps"),
            p("bar", Num, "1-based bar to start at (default: after the track's last clip)"),
            p("bars", Int, "how many bars to fill by repeating the pattern (default 1)"),
            p("steps_per_beat", Int, "4 = sixteenth notes (default), 2 = eighths, 3 = triplets"),
            p("velocity", Int, "1-127 for x hits (default 100; X = 127, o = half)"),
            p("clip", Str, "add into this existing clip instead"),
        ],
    },
    Tool {
        name: "add_melody",
        description: "Write a melody or bass line from compact text: PITCH/BEATS tokens, r = rest, C4+E4/2 = chord. Creates the clip (and track) if needed.",
        example: r#"{"track": "Lead", "bar": 1, "notes": "C4/1 D4/0.5 E4/0.5 r/1 G4/2"}"#,
        params: &[
            req("track", Str, "track name, id or position; a missing track is created if the name is a preset (Bass, Lead...) or instrument is given"),
            req("notes", Str, "e.g. \"C4/1 D4/0.5 E4/0.5 r/1 G4/2\" (1 = quarter note)"),
            p("bar", Num, "1-based bar to start at (default: after the track's last clip)"),
            p("velocity", Int, "1-127 (default 100)"),
            p("clip", Str, "add into this existing clip instead"),
            p("instrument", Str, "preset id used if the track has to be created, e.g. \"pluck\""),
        ],
    },
    Tool {
        name: "apply_ops",
        description: "Run many edits atomically (all or nothing). Each op is {\"op\": <tool name>, ...that tool's arguments}; any editing tool works, including add_chords/add_melody/add_drum_pattern.",
        example: r#"{"ops": [{"op": "add_track", "name": "Pad", "instrument": "warm_pad"}, {"op": "add_chords", "track": "Pad", "chords": "Am F C G"}]}"#,
        params: &[req("ops", Ops, "list of {\"op\": tool name, ...arguments}")],
    },
    Tool {
        name: "render_wav",
        description: "Render the song to a 16-bit stereo WAV file (defaults to <project>.wav) so it can be heard.",
        example: r#"{"output_path": "song.wav"}"#,
        params: &[
            p("output_path", Str, "where to write the .wav (relative = next to the project)"),
            p("sample_rate", Int, "default 44100"),
        ],
    },
];

/// Tool names the server also accepts, mapped to the real tool.
pub const ALIASES: &[(&str, &str)] = &[
    ("get_guide", "help"),
    ("guide", "help"),
    ("cheat_sheet", "help"),
    ("list_instruments", "list_presets"),
    ("presets", "list_presets"),
    ("create_project", "new_project"),
    ("load_project", "open_project"),
    ("show_project", "get_project"),
    ("create_track", "add_track"),
    ("create_clip", "add_clip"),
    ("add_chord_progression", "add_chords"),
    ("add_progression", "add_chords"),
    ("add_drums", "add_drum_pattern"),
    ("add_beat", "add_drum_pattern"),
    ("drum_pattern", "add_drum_pattern"),
    ("add_bassline", "add_melody"),
    ("add_bass_line", "add_melody"),
    ("render", "render_wav"),
    ("export_wav", "render_wav"),
    ("batch", "apply_ops"),
];

pub fn find(name: &str) -> Option<&'static Tool> {
    let name = name
        .trim()
        .trim_start_matches("aidaw.")
        .trim_start_matches("aidaw_");
    let name = ALIASES
        .iter()
        .find(|(a, _)| *a == name)
        .map_or(name, |(_, real)| real);
    TOOLS.iter().find(|t| t.name == name)
}

pub fn names() -> String {
    TOOLS.iter().map(|t| t.name).collect::<Vec<_>>().join(", ")
}

fn param_schema(p: &Param) -> Value {
    let simple = |ty: &str| json!({"type": ty, "description": p.desc});
    match p.ty {
        Str => simple("string"),
        Num => simple("number"),
        Int => simple("integer"),
        Bool => simple("boolean"),
        Notes => json!({
            "type": "array",
            "description": p.desc,
            "items": {"type": "object", "properties": {
                "pitch": {"type": "string", "description": "\"C4\" (= 60), \"F#3\", or a MIDI number"},
                "start": {"type": "number", "description": "beats from clip start"},
                "duration": {"type": "number", "description": "beats"},
                "velocity": {"type": "integer", "description": "1-127, default 100"}
            }, "required": ["pitch", "start", "duration"]}
        }),
        Drums => {
            let lane = |d: &str| json!({"type": "string", "description": d});
            json!({
                "type": "object",
                "description": "drum name -> step string, 16 steps per bar. x = hit, X = accent, o = soft, . = rest",
                "properties": {
                    "kick": lane("e.g. \"x...x...x...x...\" (four on the floor)"),
                    "snare": lane("e.g. \"....x.......x...\" (beats 2 and 4)"),
                    "clap": lane("same format"),
                    "hat": lane("closed hi-hat, e.g. \"x.x.x.x.x.x.x.x.\""),
                    "open_hat": lane("same format"),
                    "tom_low": lane("same format"),
                    "tom_mid": lane("same format"),
                    "tom_high": lane("same format"),
                    "crash": lane("e.g. \"x...............\""),
                    "ride": lane("same format"),
                    "rim": lane("same format"),
                    "cowbell": lane("same format"),
                    "tambourine": lane("same format"),
                    "shaker": lane("e.g. \"xxxxxxxxxxxxxxxx\""),
                    "conga_high": lane("same format; also conga_low, bongo_high, bongo_low, claves, triangle")
                }
            })
        }
        Ops => json!({
            "type": "array",
            "description": p.desc,
            "items": {"type": "object", "properties": {
                "op": {"type": "string", "description": "tool name, e.g. add_track"}
            }, "required": ["op"]}
        }),
    }
}

pub fn input_schema(tool: &Tool) -> Value {
    let props: Map<String, Value> = tool
        .params
        .iter()
        .map(|p| (p.name.to_string(), param_schema(p)))
        .collect();
    let required: Vec<&str> = tool
        .params
        .iter()
        .filter(|p| p.required)
        .map(|p| p.name)
        .collect();
    json!({"type": "object", "properties": props, "required": required})
}

pub fn description(tool: &Tool) -> String {
    format!("{} Example: {}", tool.description, tool.example)
}

/// Tool definitions in the shape a given API expects.
pub fn export(format: &str) -> Option<Value> {
    let each = |f: &dyn Fn(&Tool) -> Value| TOOLS.iter().map(f).collect::<Vec<_>>();
    Some(match format {
        "mcp" => json!(each(&|t| json!({
            "name": t.name, "description": description(t), "inputSchema": input_schema(t)
        }))),
        "openai" => json!(each(&|t| json!({
            "type": "function",
            "function": {"name": t.name, "description": description(t), "parameters": input_schema(t)}
        }))),
        "anthropic" | "claude" => json!(each(&|t| json!({
            "name": t.name, "description": description(t), "input_schema": input_schema(t)
        }))),
        "gemini" | "google" => json!([{"functionDeclarations": each(&|t| {
            let mut decl = json!({"name": t.name, "description": description(t)});
            // Gemini rejects an empty `properties` object.
            if !t.params.is_empty() {
                decl["parameters"] = input_schema(t);
            }
            decl
        })}]),
        _ => return None,
    })
}

pub const FORMATS: &str = "mcp, openai, anthropic, gemini";
