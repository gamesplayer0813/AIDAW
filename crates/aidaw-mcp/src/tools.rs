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
add_drum_pattern (they create clips for you) -> copy_bars to repeat parts -> get_project to check -> \
analyze to check the mix -> render_wav to listen. To show the human what you changed, set_loop or \
set_playhead points the app at it; undo takes back your last edit.
Every error message says what was wrong and shows a valid example; read it and retry. Call help for the \
full cheat sheet.";

/// Returned by the `help` tool and `aidaw-mcp guide`.
pub const GUIDE: &str = r##"AIDAW cheat sheet
=================
TIME   Beats (1 = quarter note). 4/4 => 4 beats per bar. bar 1 = beat 0, bar 2 = beat 4, bar 5 = beat 16.
       Other meters: update_project {"time_signature": "3/4"} (3 beats), "6/8" (3 beats = six eighths),
       "12/8" (6 beats). Bars must last a whole number of beats, so 7/8 and 5/8 are not possible.
       Clip positions are absolute; note starts are relative to their clip.
PITCH  "C4" = 60 = middle C. Sharps "F#3", flats "Bb2". Bass lives around C2, chords around C4, melody C4-C6.
NAMES  Tracks/clips can be given by name or id: {"track": "Bass"} or {"track": "t1"}.
DRUMS  kick 36, snare 38, clap 39, hat 42, open_hat 46, tom_low 41, tom_mid 45, tom_high 48, crash 49, ride 51.
       Percussion: rim 37, ride_bell 53, tambourine 54, splash 55, cowbell 56, bongo_high 60, bongo_low 61,
       conga_mute 62, conga_high 63, conga_low 64, timbale_high 65, timbale_low 66, agogo_high 67,
       agogo_low 68, shaker 70, claves 75, block_high 76, block_low 77, triangle 81.
SOUNDS 78 presets in 10 groups (list_presets): keys, bass, lead, pad, pluck, bells, orchestra, retro,
       fx, drums. Drum kits: drum_kit, drums_808, drums_909, drums_acoustic, drums_lofi, drums_trap,
       drums_electro.

MIX    Every track has volume (gain), pan and a reverb send (0 dry .. 1 very wet) into one shared reverb
       room. New tracks start with a sensible amount for their preset (pads 0.3, keys 0.2, drums 0.1,
       bass 0). Room: update_project {"reverb_size": 0.7, "reverb_tone": 0.4} (size 0 small room ..
       1 big hall, tone 0 dark .. 1 bright).

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
  update_track     {"track": "Lead", "reverb": 0.35}
  render_wav       {}

NOTATIONS
  chords   "Am F C G" = one chord per bar. "Am:2 F:2" = 2 beats each. "r" = rest.
           Qualities: C Am F#m Bb G7 Cmaj7 Dm7 Bm7b5 Esus4 Asus2 Cdim Caug C6 C6/9 Cadd9 C9 C11 C13
           G7b9 G7#9 Dm11 Cmaj7#11 CmMaj7 C5, slash C/E.
           style: block (held), arpeggio (eighth notes), pulse (every beat). octave: 4 = around middle C.
  melody   "PITCH/BEATS" tokens: "C4/1 D4/0.5 E4/0.5 r/1 G4/2". r = rest. "C4+E4+G4/2" = chord.
           Duration may be omitted (repeats the previous one). "@80" sets velocity: "C4/1@80".
  drums    4 steps per beat (sixteenth notes; 16 per bar in 4/4, 12 in 3/4 and 6/8): x = hit,
           X = accent, o = soft, . = rest.
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

ARRANGE AND POLISH
  copy_bars {"from_bar": 1, "bars": 4, "to_bar": 5, "times": 3}
           Repeats bars 1-4 three times (bars 5-16) on every track with notes there; add
           {"track": "Drums"} for one track. What was in the destination bars is replaced.
  quantize {"track": "Drums", "grid": 0.25, "swing": 0.6}
           Snaps note starts to the grid. strength 0-1 (1 = all the way), swing 0.5 straight ..
           0.75 heavy shuffle. Give "clip", "track" or neither (the whole song).
  humanize {"track": "Keys", "timing": 0.02, "velocity": 8}
           Small random timing and velocity changes so programmed parts sound played. Once is
           enough; the same "seed" gives the same result.
  split_clip {"clip": "c1", "bar": 3}
           Cuts a clip in two at the start of bar 3 (no clip: every clip crossing bar 3). Notes
           starting before the cut stay, shortened to end there; the rest go to the new clip.
  update_track {"track": "Bass", "position": 1, "color": "orange"}
           Moves a track to the top row and gives it a colour (names, "#ff8a3d" or "auto").

SHOW THE HUMAN (in the AIDAW app, when it has this song open)
  set_loop {"from_bar": 5, "bars": 4, "play": true}
           The app repeats bars 5-8 and starts playing them. {"clip": "Chorus"} loops a clip;
           {"on": false} plays the whole song again. render_wav and analyze ignore the loop.
  set_playhead {"bar": 9}
           Puts the app's playhead at bar 9; "play": true starts playing there, false stops.

UNDO
  undo {}  takes back your last edit ({"steps": 3}: the last three). Only edits made through these
           tools, newest first; if the human changed the song in the app since, undo refuses
           rather than lose their work.

CHECK YOUR WORK
  get_project {}                    compact overview (tracks, clips, bars)
  get_project {"detail": "notes"}   every note as start:PITCH/BEATS
  get_project {"detail": "json"}    raw project JSON
  analyze {}                        hear the mix as numbers: loudness (LUFS), limiter, low/mid/high
                                    balance, each track alone, loudness along the song, and tips
"##;

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
const INSTRUMENT: &str = "preset id (78 of them, see list_presets), e.g. soft_keys, e_piano, organ, bass, \
                          bass_808, acid_bass, bright_lead, supersaw, warm_pad, lush_pad, choir, pluck, \
                          nylon_guitar, rock_guitar, marimba, bell, strings, brass, flute, saxophone, \
                          chiptune, riser, drum_kit, drums_808, drums_909, drums_acoustic, drums_lofi, \
                          drums_trap. Loose names work too (\"rhodes\", \"violin\")";
const KIT: &str = "drum tracks only: standard | 808 | 909 | acoustic | lofi | trap | electro";
const REVERB: &str = "0-1 how much of this track goes to the shared reverb: 0 dry, 0.2-0.35 typical for keys, \
                      pads and leads, 0.1 drums, 0 bass. New tracks start with their preset's usual amount";
const COLOR: &str = "colour of the track in the app: blue, orange, green, purple, yellow, cyan, pink, violet, \
                     red, teal, white, gray, a hex code like \"#ff8a3d\", or \"auto\" for its own colour";
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
            p("time_signature", Str, "meter, default \"4/4\"; e.g. \"3/4\", \"6/8\", \"12/8\""),
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
        description: "Change the project name, bpm (20-400), time signature, master_gain (0-2) or the shared reverb room every track can send to (reverb_size, reverb_tone).",
        example: r#"{"bpm": 92, "time_signature": "6/8", "reverb_size": 0.7}"#,
        params: &[
            p("name", Str, "song title"),
            p("bpm", Num, "tempo, 20-400"),
            p("time_signature", Str, "meter such as \"4/4\", \"3/4\", \"6/8\" or \"12/8\"; sets the bar length (6/8 = 3 beats)"),
            p("beats_per_bar", Int, "bar length in beats (quarter notes), 1-32: 4 for 4/4, 3 for 3/4. Prefer time_signature"),
            p("master_gain", Num, "0-2, 1 = unity"),
            p("reverb_size", Num, "0-1 shared reverb room: 0 small room (short tail) .. 1 big hall (long tail); default 0.5"),
            p("reverb_tone", Num, "0-1 shared reverb tone: 0 dark .. 1 bright; default 0.5"),
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
            p("reverb", Num, REVERB),
            p("color", Str, COLOR),
        ],
    },
    Tool {
        name: "update_track",
        description: "Change a track's name, instrument preset, drum kit, volume, pan, reverb, mute/solo, its row (position) and colour in the app, or tweak its synth sound.",
        example: r#"{"track": "Bass", "instrument": "pluck", "gain": 0.6, "cutoff": 1200}"#,
        params: &[
            req("track", Str, TRACK),
            p("name", Str, "new name"),
            p("position", Int, "move the track to this row in the app, 1 = the top"),
            p("color", Str, COLOR),
            p("instrument", Str, INSTRUMENT),
            p("gain", Num, "volume 0-2, 1 = unity"),
            p("pan", Num, "-1 left .. 1 right"),
            p("reverb", Num, REVERB),
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
        name: "copy_bars",
        description: "Copy bars to another place, like copy and paste: the notes that start in those bars go to to_bar, repeated `times` times in a row. Works on every track with notes there, or only on `track`; what those tracks had in the destination bars is replaced. Use it to repeat a chorus or extend a loop.",
        example: r#"{"from_bar": 1, "bars": 4, "to_bar": 5, "times": 3}"#,
        params: &[
            req("from_bar", Num, "first bar to copy (1 = the start of the song)"),
            req("bars", Num, "how many bars to copy"),
            req("to_bar", Num, "bar where the first copy starts"),
            p("times", Int, "copies in a row, 1-256 (default 1)"),
            p("track", Str, "only this track (default: every track with notes in those bars)"),
        ],
    },
    Tool {
        name: "quantize",
        description: "Snap note starts to a grid so the timing is tight, optionally with swing (a shuffle feel). Works on one clip, every clip of a track, or the whole song. Lengths are kept.",
        example: r#"{"track": "Drums", "grid": 0.25, "swing": 0.6}"#,
        params: &[
            p("clip", Str, CLIP),
            p("track", Str, "every clip on this track (default: the whole song)"),
            p("grid", Num, "grid in beats: 0.25 = sixteenth notes (default), 0.5 = eighths, 1 = quarter notes"),
            p("strength", Num, "0-1 how far notes move to the grid (default 1 = all the way)"),
            p("swing", Num, "0.5 straight (default) .. 0.75 heavy shuffle; 0.6 is a light swing. Delays every second grid step, so grid 0.5 swings eighth notes"),
        ],
    },
    Tool {
        name: "humanize",
        description: "Make programmed parts sound played: nudge note timing and velocity by small random amounts (the same seed gives the same result). Works on one clip, every clip of a track, or the whole song. Apply it once; each call adds more.",
        example: r#"{"track": "Keys", "timing": 0.02, "velocity": 10}"#,
        params: &[
            p("clip", Str, CLIP),
            p("track", Str, "every clip on this track (default: the whole song)"),
            p("timing", Num, "largest shift in beats, 0-0.25 (default 0.02, about 10 ms at 120 BPM)"),
            p("velocity", Int, "largest velocity change, 0-40 (default 8)"),
            p("seed", Int, "another number gives another random result"),
        ],
    },
    Tool {
        name: "split_clip",
        description: "Cut clips in two at a bar, like scissors, so the parts can be moved, copied or removed on their own. Notes that start before the cut stay (shortened to end there); the rest go to a new clip that starts at the cut. Give clip, or track (its clips crossing the cut), or neither (every clip crossing the cut).",
        example: r#"{"clip": "c1", "bar": 3}"#,
        params: &[
            p("bar", Num, "1-based bar to cut at: 3 cuts between bars 2 and 3, 3.5 half way through bar 3"),
            p("beat", Num, "absolute beat to cut at (alternative to bar)"),
            p("clip", Str, CLIP),
            p("track", Str, "cut every clip on this track that crosses the cut"),
        ],
    },
    Tool {
        name: "set_loop",
        description: "Make the AIDAW app repeat part of the song while the human listens (an A/B loop), e.g. the bars you just changed. Give from_bar and bars, or a clip; on: false plays the whole song again (the loop is kept for later). play: true also starts playing at the loop. Only live playback in the app loops; render_wav and analyze always use the whole song.",
        example: r#"{"from_bar": 5, "bars": 4, "play": true}"#,
        params: &[
            p("from_bar", Num, "first bar of the loop (1 = the start of the song)"),
            p("bars", Num, "how many bars to loop (default 1)"),
            p("clip", Str, "loop exactly this clip instead (name or id)"),
            p("on", Bool, "false: stop looping; true (default): loop"),
            p("play", Bool, "true: start playing at the loop's start; false: stop playing"),
        ],
    },
    Tool {
        name: "set_playhead",
        description: "Put the AIDAW app's playhead at a bar, so the human hears that spot when they press Play. play: true starts playing there at once, false stops. Use it to point the human at what you changed.",
        example: r#"{"bar": 9, "play": true}"#,
        params: &[
            p("bar", Num, "1-based bar: 1 = the start of the song, 9.5 = half way through bar 9"),
            p("beat", Num, "absolute beat instead of bar (0 = the start)"),
            p("play", Bool, "true: start playing there; false: stop"),
        ],
    },
    Tool {
        name: "undo",
        description: "Take back your last edit (or several), e.g. when analyze or the human says it was a mistake. Only edits made through these tools are undone, newest first; if the human changed the song in the app since, undo refuses rather than lose their work.",
        example: r#"{"steps": 1}"#,
        params: &[p("steps", Int, "how many of your edits to take back, 1-30 (default 1)")],
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
        description: "Write a drum beat from step strings, 4 steps per beat (16 per bar in 4/4): x = hit, X = accent, o = soft, . = rest. Shorter strings repeat. Creates the clip (and a drum track) if needed.",
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
        name: "analyze",
        description: "Hear the mix as numbers: renders the song (nothing is written) and reports loudness in LUFS, peak, how hard the limiter works, the low/mid/high balance, each track's level alone, loudness along the song, and tips with calls that fix what stands out. Use it after big changes.",
        example: "{}",
        params: &[],
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
    ("duplicate_bars", "copy_bars"),
    ("repeat_bars", "copy_bars"),
    ("copy_section", "copy_bars"),
    ("quantize_clip", "quantize"),
    ("humanize_clip", "humanize"),
    ("analyse", "analyze"),
    ("analyze_mix", "analyze"),
    ("check_mix", "analyze"),
    ("listen", "analyze"),
    ("split", "split_clip"),
    ("split_clips", "split_clip"),
    ("cut_clip", "split_clip"),
    ("loop", "set_loop"),
    ("set_loop_region", "set_loop"),
    ("loop_bars", "set_loop"),
    ("seek", "set_playhead"),
    ("move_playhead", "set_playhead"),
    ("go_to_bar", "set_playhead"),
    ("play_from", "set_playhead"),
    ("undo_last", "undo"),
    ("undo_edit", "undo"),
    ("revert", "undo"),
    ("move_track", "update_track"),
    ("reorder_track", "update_track"),
    ("set_track_color", "update_track"),
    ("color_track", "update_track"),
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
                "description": "drum name -> step string, 4 steps per beat (16 per bar in 4/4). x = hit, X = accent, o = soft, . = rest",
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

/// Every tool with every parameter, in Markdown: what `aidaw-mcp docs`
/// prints and docs/TOOLS.md holds (a test keeps that file equal to this).
pub fn markdown() -> String {
    let mut md = format!(
        "# AIDAW tools\n\n\
         <!-- Generated from crates/aidaw-mcp/src/tools.rs. Don't edit it here: change that file, \
         then run `cargo run -p aidaw-mcp -- docs docs/TOOLS.md`. -->\n\n\
         All {} tools of `aidaw-mcp`, with every parameter. They are the same over MCP and on the \
         command line (`aidaw-mcp tools --format ...`, `aidaw-mcp call`). How to connect a model \
         and compose with them: [LLM_GUIDE.md](LLM_GUIDE.md).\n\n\
         | Tool | What it does |\n|---|---|\n",
        TOOLS.len()
    );
    for t in TOOLS {
        md += &format!(
            "| [`{0}`](#{0}) | {1} |\n",
            t.name,
            cell(first_sentence(t.description))
        );
    }
    for t in TOOLS {
        md += &format!("\n## `{}`\n\n{}\n\n", t.name, text(t.description));
        if t.params.is_empty() {
            md += "No parameters.\n";
        } else {
            md += "| Parameter | Type | Required | Description |\n|---|---|---|---|\n";
            for p in t.params {
                md += &format!(
                    "| `{}` | {} | {} | {} |\n",
                    p.name,
                    type_name(p.ty),
                    if p.required { "yes" } else { "no" },
                    cell(p.desc)
                );
            }
        }
        md += &format!("\nExample:\n\n```json\n{}\n```\n", t.example);
        let aliases: Vec<String> = ALIASES
            .iter()
            .filter(|(_, real)| *real == t.name)
            .map(|(alias, _)| format!("`{alias}`"))
            .collect();
        if !aliases.is_empty() {
            md += &format!("\nAlso answers to {}.\n", aliases.join(", "));
        }
    }
    md
}

fn type_name(ty: Ty) -> &'static str {
    match ty {
        Str => "string",
        Num => "number",
        Int => "integer",
        Bool => "boolean",
        Notes => "string or array",
        Drums => "object",
        Ops => "array",
    }
}

/// Up to the first full stop: the summary of a description. Not a dot
/// after a space (". = rest" in a drum legend) or in "e.g.".
fn first_sentence(s: &str) -> &str {
    let mut from = 0;
    while let Some(i) = s[from..].find(". ").map(|i| from + i) {
        let before = &s[..i];
        let symbol = before.is_empty() || before.ends_with(char::is_whitespace);
        if !symbol && !before.ends_with("e.g") && !before.ends_with("i.e") {
            return &s[..=i];
        }
        from = i + 2;
    }
    s
}

/// Prose for Markdown: a `<` outside code would start an HTML tag and hide
/// the text after it (as in "<project>.wav").
fn text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut code = false;
    for c in s.chars() {
        match c {
            '`' => code = !code,
            '<' if !code => {
                out += "&lt;";
                continue;
            }
            _ => {}
        }
        out.push(c);
    }
    out
}

/// Prose for a table cell, where `|` (even inside code) ends the cell.
fn cell(s: &str) -> String {
    text(s).replace('|', "\\|").replace('\n', " ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// A file of this repository, with Windows line ends made plain (a
    /// checkout on Windows may convert them).
    fn repo_file(path: &str) -> String {
        let full = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(path);
        std::fs::read_to_string(&full)
            .unwrap_or_else(|e| panic!("{}: {e}", full.display()))
            .replace("\r\n", "\n")
    }

    #[test]
    fn the_tool_reference_matches_the_tools() {
        let fresh = markdown();
        let saved = repo_file("docs/TOOLS.md");
        if saved != fresh {
            let line = saved
                .lines()
                .zip(fresh.lines())
                .position(|(a, b)| a != b)
                .unwrap_or_else(|| saved.lines().count().min(fresh.lines().count()))
                + 1;
            panic!(
                "docs/TOOLS.md is out of date (from line {line}): run \
                 `cargo run -p aidaw-mcp -- docs docs/TOOLS.md`"
            );
        }
    }

    #[test]
    fn the_readme_and_the_guide_name_every_tool_and_count_them_right() {
        for doc in ["README.md", "docs/LLM_GUIDE.md"] {
            let text = repo_file(doc);
            let missing: Vec<&str> = TOOLS
                .iter()
                .map(|t| t.name)
                .filter(|name| !text.contains(&format!("`{name}`")))
                .collect();
            assert!(
                missing.is_empty(),
                "{doc} never names {missing:?}: add them where it lists the tools"
            );
        }
        let readme = repo_file("README.md");
        let counts: Vec<&str> = readme
            .match_indices(" 个 MCP 工具")
            .map(|(i, _)| {
                let before = &readme[..i];
                &before[before.trim_end_matches(|c: char| c.is_ascii_digit()).len()..]
            })
            .collect();
        assert!(
            !counts.is_empty(),
            "README.md no longer says how many MCP tools there are"
        );
        for count in counts {
            assert_eq!(
                count,
                TOOLS.len().to_string(),
                "README.md says \"{count} 个 MCP 工具\", but there are {}",
                TOOLS.len()
            );
        }
    }

    #[test]
    fn markdown_escapes_what_would_break_a_table_or_hide_text() {
        assert_eq!(
            first_sentence("Delete a clip. Then more."),
            "Delete a clip."
        );
        assert_eq!(first_sentence("No full stop"), "No full stop");
        assert_eq!(
            first_sentence("Steps: x = hit, . = rest. Shorter ones repeat."),
            "Steps: x = hit, . = rest."
        );
        assert_eq!(
            first_sentence("Loose, e.g. a name. More."),
            "Loose, e.g. a name."
        );
        assert_eq!(
            text("defaults to <project>.wav"),
            "defaults to &lt;project>.wav"
        );
        assert_eq!(text("keep `a<b` as code"), "keep `a<b` as code");
        assert_eq!(cell("standard | 808\nnext"), "standard \\| 808 next");
    }
}
