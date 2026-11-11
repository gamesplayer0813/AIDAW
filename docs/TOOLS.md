# AIDAW tools

<!-- Generated from crates/aidaw-mcp/src/tools.rs. Don't edit it here: change that file, then run `cargo run -p aidaw-mcp -- docs docs/TOOLS.md`. -->

All 24 tools of `aidaw-mcp`, with every parameter. They are the same over MCP and on the command line (`aidaw-mcp tools --format ...`, `aidaw-mcp call`). How to connect a model and compose with them: [LLM_GUIDE.md](LLM_GUIDE.md).

| Tool | What it does |
|---|---|
| [`help`](#help) | Return the AIDAW cheat sheet: time, pitch, notations, placement rules and a complete quick-start song. |
| [`new_project`](#new_project) | Create a new empty project file and make it the current project. |
| [`open_project`](#open_project) | Open an existing .aidaw.json project and make it current. |
| [`get_project`](#get_project) | Show the current project. |
| [`list_presets`](#list_presets) | List the instrument presets (ids like "e_piano", "bass_808", "strings", "drums_808") usable as `instrument` in add_track/update_track, grouped by category. |
| [`update_project`](#update_project) | Change the project name, bpm (20-400), time signature, master_gain (0-2) or the shared reverb room every track can send to (reverb_size, reverb_tone). |
| [`add_track`](#add_track) | Add a track with an instrument preset. |
| [`update_track`](#update_track) | Change a track's name, instrument preset, drum kit, volume, pan, reverb, mute/solo, or tweak its synth sound. |
| [`remove_track`](#remove_track) | Delete a track and all its clips. |
| [`add_clip`](#add_clip) | Add a clip (a block of notes) to a track at a bar or beat, optionally with notes. |
| [`update_clip`](#update_clip) | Move, resize or rename a clip. |
| [`remove_clip`](#remove_clip) | Delete a clip. |
| [`add_notes`](#add_notes) | Add notes to an existing clip. |
| [`remove_notes`](#remove_notes) | Remove notes from a clip whose start is in [from, to) beats and that match pitch. |
| [`transpose_clip`](#transpose_clip) | Shift every note in a clip up or down by semitones (12 = one octave). |
| [`copy_bars`](#copy_bars) | Copy bars to another place, like copy and paste: the notes that start in those bars go to to_bar, repeated `times` times in a row. |
| [`quantize`](#quantize) | Snap note starts to a grid so the timing is tight, optionally with swing (a shuffle feel). |
| [`humanize`](#humanize) | Make programmed parts sound played: nudge note timing and velocity by small random amounts (the same seed gives the same result). |
| [`add_chords`](#add_chords) | Write a chord progression from chord symbols; one chord per bar unless "Am:2" gives beats. |
| [`add_drum_pattern`](#add_drum_pattern) | Write a drum beat from step strings, 4 steps per beat (16 per bar in 4/4): x = hit, X = accent, o = soft, . = rest. |
| [`add_melody`](#add_melody) | Write a melody or bass line from compact text: PITCH/BEATS tokens, r = rest, C4+E4/2 = chord. |
| [`apply_ops`](#apply_ops) | Run many edits atomically (all or nothing). |
| [`analyze`](#analyze) | Hear the mix as numbers: renders the song (nothing is written) and reports loudness in LUFS, peak, how hard the limiter works, the low/mid/high balance, each track's level alone, loudness along the song, and tips with calls that fix what stands out. |
| [`render_wav`](#render_wav) | Render the song to a 16-bit stereo WAV file (defaults to &lt;project>.wav) so it can be heard. |

## `help`

Return the AIDAW cheat sheet: time, pitch, notations, placement rules and a complete quick-start song. Call this first if unsure.

No parameters.

Example:

```json
{}
```

Also answers to `get_guide`, `guide`, `cheat_sheet`.

## `new_project`

Create a new empty project file and make it the current project.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `path` | string | yes | file path ending in .aidaw.json (relative = next to the current project, else the Music folder) |
| `name` | string | no | song title |
| `bpm` | number | no | tempo, 20-400 (default 120) |
| `time_signature` | string | no | meter, default "4/4"; e.g. "3/4", "6/8", "12/8" |
| `overwrite` | boolean | no | replace the file if it exists |

Example:

```json
{"path": "song.aidaw.json", "name": "My Song", "bpm": 100}
```

Also answers to `create_project`.

## `open_project`

Open an existing .aidaw.json project and make it current. Returns a summary.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `path` | string | yes | file path |

Example:

```json
{"path": "song.aidaw.json"}
```

Also answers to `load_project`.

## `get_project`

Show the current project. detail: "summary" (default: tracks, clips, bars), "notes" (also every note) or "json" (raw file).

| Parameter | Type | Required | Description |
|---|---|---|---|
| `detail` | string | no | summary \| notes \| json |
| `track` | string | no | only show this track (name or id) |

Example:

```json
{"detail": "notes", "track": "Bass"}
```

Also answers to `show_project`.

## `list_presets`

List the instrument presets (ids like "e_piano", "bass_808", "strings", "drums_808") usable as `instrument` in add_track/update_track, grouped by category.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `category` | string | no | only this group: keys \| bass \| lead \| pad \| pluck \| bells \| orchestra \| retro \| fx \| drums |

Example:

```json
{"category": "bass"}
```

Also answers to `list_instruments`, `presets`.

## `update_project`

Change the project name, bpm (20-400), time signature, master_gain (0-2) or the shared reverb room every track can send to (reverb_size, reverb_tone).

| Parameter | Type | Required | Description |
|---|---|---|---|
| `name` | string | no | song title |
| `bpm` | number | no | tempo, 20-400 |
| `time_signature` | string | no | meter such as "4/4", "3/4", "6/8" or "12/8"; sets the bar length (6/8 = 3 beats) |
| `beats_per_bar` | integer | no | bar length in beats (quarter notes), 1-32: 4 for 4/4, 3 for 3/4. Prefer time_signature |
| `master_gain` | number | no | 0-2, 1 = unity |
| `reverb_size` | number | no | 0-1 shared reverb room: 0 small room (short tail) .. 1 big hall (long tail); default 0.5 |
| `reverb_tone` | number | no | 0-1 shared reverb tone: 0 dark .. 1 bright; default 0.5 |

Example:

```json
{"bpm": 92, "time_signature": "6/8", "reverb_size": 0.7}
```

## `add_track`

Add a track with an instrument preset. Returns its track_id. Without instrument, a preset matching the name is used ("Drums" -> drum_kit), else a saw synth.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `name` | string | yes | track name, e.g. "Bass" |
| `instrument` | string | no | preset id (78 of them, see list_presets), e.g. soft_keys, e_piano, organ, bass, bass_808, acid_bass, bright_lead, supersaw, warm_pad, lush_pad, choir, pluck, nylon_guitar, rock_guitar, marimba, bell, strings, brass, flute, saxophone, chiptune, riser, drum_kit, drums_808, drums_909, drums_acoustic, drums_lofi, drums_trap. Loose names work too ("rhodes", "violin") |
| `kit` | string | no | drum tracks only: standard \| 808 \| 909 \| acoustic \| lofi \| trap \| electro |
| `gain` | number | no | volume 0-2 (default 0.8) |
| `pan` | number | no | -1 left .. 1 right |
| `reverb` | number | no | 0-1 how much of this track goes to the shared reverb: 0 dry, 0.2-0.35 typical for keys, pads and leads, 0.1 drums, 0 bass. New tracks start with their preset's usual amount |

Example:

```json
{"name": "Bass", "instrument": "bass"}
```

Also answers to `create_track`.

## `update_track`

Change a track's name, instrument preset, drum kit, volume, pan, reverb, mute/solo, or tweak its synth sound.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `track` | string | yes | track name or id, e.g. "Bass" or "t1" |
| `name` | string | no | new name |
| `instrument` | string | no | preset id (78 of them, see list_presets), e.g. soft_keys, e_piano, organ, bass, bass_808, acid_bass, bright_lead, supersaw, warm_pad, lush_pad, choir, pluck, nylon_guitar, rock_guitar, marimba, bell, strings, brass, flute, saxophone, chiptune, riser, drum_kit, drums_808, drums_909, drums_acoustic, drums_lofi, drums_trap. Loose names work too ("rhodes", "violin") |
| `gain` | number | no | volume 0-2, 1 = unity |
| `pan` | number | no | -1 left .. 1 right |
| `reverb` | number | no | 0-1 how much of this track goes to the shared reverb: 0 dry, 0.2-0.35 typical for keys, pads and leads, 0.1 drums, 0 bass. New tracks start with their preset's usual amount |
| `mute` | boolean | no | silence the track |
| `solo` | boolean | no | play only soloed tracks |
| `kit` | string | no | drum tracks only: standard \| 808 \| 909 \| acoustic \| lofi \| trap \| electro |
| `waveform` | string | no | synth only: sine \| triangle \| saw \| square \| pulse \| noise |
| `attack` | number | no | synth only: seconds |
| `decay` | number | no | synth only: seconds |
| `sustain` | number | no | synth only: level 0-1 |
| `release` | number | no | synth only: seconds |
| `cutoff` | number | no | synth only: low-pass cutoff in Hz (lower = darker) |
| `resonance` | number | no | synth only: 0-1, whistling peak at the cutoff (acid, wah) |
| `filter_env` | number | no | synth only: octaves the cutoff jumps up at each note, 0-8 (plucks, brass) |
| `filter_decay` | number | no | synth only: seconds for that filter jump to fall back |
| `unison` | integer | no | synth only: 1-5 stacked detuned voices (wider, fatter) |
| `detune` | number | no | synth only: unison spread in cents, e.g. 20 |
| `sub` | number | no | synth only: 0-1 sine one octave below |
| `noise` | number | no | synth only: 0-1 noise mixed in (breath, air) |
| `fm` | number | no | synth only: 0-10 FM amount for sine/triangle (electric piano, bells) |
| `fm_ratio` | number | no | synth only: FM modulator ratio; 1 warm, 2 hollow, 3.5 metallic |
| `fm_decay` | number | no | synth only: seconds the FM tone takes to fade (0 = never) |
| `vibrato` | number | no | synth only: pitch wobble depth in cents, e.g. 15 |
| `vibrato_rate` | number | no | synth only: vibrato speed in Hz (default 5) |
| `pitch_env` | number | no | synth only: semitones above the note each note starts at, sliding down |
| `pitch_decay` | number | no | synth only: seconds for that pitch slide |
| `drive` | number | no | synth only: 0-1 saturation / distortion |

Example:

```json
{"track": "Bass", "instrument": "pluck", "gain": 0.6, "cutoff": 1200}
```

## `remove_track`

Delete a track and all its clips.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `track` | string | yes | track name or id, e.g. "Bass" or "t1" |

Example:

```json
{"track": "Bass"}
```

## `add_clip`

Add a clip (a block of notes) to a track at a bar or beat, optionally with notes. Returns clip_id. Length defaults to fit the notes.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `track` | string | yes | track name or id, e.g. "Bass" or "t1" |
| `bar` | number | no | 1-based bar to start at (bar 1 = beginning of the song) |
| `start` | number | no | absolute start in beats (alternative to bar) |
| `bars` | number | no | length in bars |
| `length` | number | no | length in beats (alternative to bars) |
| `name` | string | no | clip name |
| `notes` | string or array | no | notes as a melody string like "C4/1 E4/0.5 G4/0.5" (PITCH/BEATS, r = rest), or an array of {pitch, start, duration, velocity} with start in beats from the clip start |

Example:

```json
{"track": "Lead", "bar": 1, "bars": 2, "notes": "C5/1 E5/1 G5/2"}
```

Also answers to `create_clip`.

## `update_clip`

Move, resize or rename a clip.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `clip` | string | yes | clip name or id, e.g. "c1" (ids are shown by get_project) |
| `name` | string | no | new name |
| `bar` | number | no | move to this 1-based bar |
| `start` | number | no | move to this absolute beat |
| `bars` | number | no | new length in bars |
| `length` | number | no | new length in beats |

Example:

```json
{"clip": "c2", "bar": 5, "bars": 4}
```

## `remove_clip`

Delete a clip.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `clip` | string | yes | clip name or id, e.g. "c1" (ids are shown by get_project) |

Example:

```json
{"clip": "c2"}
```

## `add_notes`

Add notes to an existing clip. Note starts are beats from the clip start.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `clip` | string | yes | clip name or id, e.g. "c1" (ids are shown by get_project) |
| `notes` | string or array | yes | notes as a melody string like "C4/1 E4/0.5 G4/0.5" (PITCH/BEATS, r = rest), or an array of {pitch, start, duration, velocity} with start in beats from the clip start |

Example:

```json
{"clip": "c1", "notes": [{"pitch": "E4", "start": 2, "duration": 1}]}
```

## `remove_notes`

Remove notes from a clip whose start is in [from, to) beats and that match pitch. All filters optional; none clears the clip.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `clip` | string | yes | clip name or id, e.g. "c1" (ids are shown by get_project) |
| `from` | number | no | beat (relative to clip), inclusive |
| `to` | number | no | beat (relative to clip), exclusive |
| `pitch` | string | no | only this pitch, e.g. "C4" or 60 |

Example:

```json
{"clip": "c1", "from": 4, "to": 8, "pitch": "C4"}
```

## `transpose_clip`

Shift every note in a clip up or down by semitones (12 = one octave).

| Parameter | Type | Required | Description |
|---|---|---|---|
| `clip` | string | yes | clip name or id, e.g. "c1" (ids are shown by get_project) |
| `semitones` | integer | yes | e.g. 2, -5, 12 |

Example:

```json
{"clip": "c1", "semitones": -12}
```

## `copy_bars`

Copy bars to another place, like copy and paste: the notes that start in those bars go to to_bar, repeated `times` times in a row. Works on every track with notes there, or only on `track`; what those tracks had in the destination bars is replaced. Use it to repeat a chorus or extend a loop.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `from_bar` | number | yes | first bar to copy (1 = the start of the song) |
| `bars` | number | yes | how many bars to copy |
| `to_bar` | number | yes | bar where the first copy starts |
| `times` | integer | no | copies in a row, 1-256 (default 1) |
| `track` | string | no | only this track (default: every track with notes in those bars) |

Example:

```json
{"from_bar": 1, "bars": 4, "to_bar": 5, "times": 3}
```

Also answers to `duplicate_bars`, `repeat_bars`, `copy_section`.

## `quantize`

Snap note starts to a grid so the timing is tight, optionally with swing (a shuffle feel). Works on one clip, every clip of a track, or the whole song. Lengths are kept.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `clip` | string | no | clip name or id, e.g. "c1" (ids are shown by get_project) |
| `track` | string | no | every clip on this track (default: the whole song) |
| `grid` | number | no | grid in beats: 0.25 = sixteenth notes (default), 0.5 = eighths, 1 = quarter notes |
| `strength` | number | no | 0-1 how far notes move to the grid (default 1 = all the way) |
| `swing` | number | no | 0.5 straight (default) .. 0.75 heavy shuffle; 0.6 is a light swing. Delays every second grid step, so grid 0.5 swings eighth notes |

Example:

```json
{"track": "Drums", "grid": 0.25, "swing": 0.6}
```

Also answers to `quantize_clip`.

## `humanize`

Make programmed parts sound played: nudge note timing and velocity by small random amounts (the same seed gives the same result). Works on one clip, every clip of a track, or the whole song. Apply it once; each call adds more.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `clip` | string | no | clip name or id, e.g. "c1" (ids are shown by get_project) |
| `track` | string | no | every clip on this track (default: the whole song) |
| `timing` | number | no | largest shift in beats, 0-0.25 (default 0.02, about 10 ms at 120 BPM) |
| `velocity` | integer | no | largest velocity change, 0-40 (default 8) |
| `seed` | integer | no | another number gives another random result |

Example:

```json
{"track": "Keys", "timing": 0.02, "velocity": 10}
```

Also answers to `humanize_clip`.

## `add_chords`

Write a chord progression from chord symbols; one chord per bar unless "Am:2" gives beats. Voiced around middle C. Creates the clip (and track) if needed.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `track` | string | yes | track name, id or position; a missing track is created if the name is a preset (Piano, Pad...) or instrument is given |
| `chords` | string | yes | e.g. "Am F C G", "Dm7:2 G7:2 Cmaj7", "C/E"; r = rest |
| `bar` | number | no | 1-based bar to start at (default: after the track's last clip) |
| `beats_per_chord` | number | no | default: one bar |
| `style` | string | no | block (held, default) \| arpeggio (eighth notes) \| pulse (every beat) |
| `octave` | integer | no | 4 = around middle C (default), 3 lower, 5 higher |
| `velocity` | integer | no | 1-127 (default 90) |
| `clip` | string | no | add into this existing clip instead |
| `instrument` | string | no | preset id used if the track has to be created, e.g. "warm_pad" |

Example:

```json
{"track": "Keys", "chords": "Am F C G", "bar": 1, "style": "block"}
```

Also answers to `add_chord_progression`, `add_progression`.

## `add_drum_pattern`

Write a drum beat from step strings, 4 steps per beat (16 per bar in 4/4): x = hit, X = accent, o = soft, . = rest. Shorter strings repeat. Creates the clip (and a drum track) if needed.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `track` | string | no | drum track name or id (default "Drums", created if missing) |
| `pattern` | object | yes | drum name -> steps |
| `bar` | number | no | 1-based bar to start at (default: after the track's last clip) |
| `bars` | integer | no | how many bars to fill by repeating the pattern (default 1) |
| `steps_per_beat` | integer | no | 4 = sixteenth notes (default), 2 = eighths, 3 = triplets |
| `velocity` | integer | no | 1-127 for x hits (default 100; X = 127, o = half) |
| `clip` | string | no | add into this existing clip instead |

Example:

```json
{"track": "Drums", "bars": 4, "pattern": {"kick": "x...x...x...x...", "snare": "....x.......x...", "hat": "x.x.x.x.x.x.x.x."}}
```

Also answers to `add_drums`, `add_beat`, `drum_pattern`.

## `add_melody`

Write a melody or bass line from compact text: PITCH/BEATS tokens, r = rest, C4+E4/2 = chord. Creates the clip (and track) if needed.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `track` | string | yes | track name, id or position; a missing track is created if the name is a preset (Bass, Lead...) or instrument is given |
| `notes` | string | yes | e.g. "C4/1 D4/0.5 E4/0.5 r/1 G4/2" (1 = quarter note) |
| `bar` | number | no | 1-based bar to start at (default: after the track's last clip) |
| `velocity` | integer | no | 1-127 (default 100) |
| `clip` | string | no | add into this existing clip instead |
| `instrument` | string | no | preset id used if the track has to be created, e.g. "pluck" |

Example:

```json
{"track": "Lead", "bar": 1, "notes": "C4/1 D4/0.5 E4/0.5 r/1 G4/2"}
```

Also answers to `add_bassline`, `add_bass_line`.

## `apply_ops`

Run many edits atomically (all or nothing). Each op is {"op": &lt;tool name>, ...that tool's arguments}; any editing tool works, including add_chords/add_melody/add_drum_pattern.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `ops` | array | yes | list of {"op": tool name, ...arguments} |

Example:

```json
{"ops": [{"op": "add_track", "name": "Pad", "instrument": "warm_pad"}, {"op": "add_chords", "track": "Pad", "chords": "Am F C G"}]}
```

Also answers to `batch`.

## `analyze`

Hear the mix as numbers: renders the song (nothing is written) and reports loudness in LUFS, peak, how hard the limiter works, the low/mid/high balance, each track's level alone, loudness along the song, and tips with calls that fix what stands out. Use it after big changes.

No parameters.

Example:

```json
{}
```

Also answers to `analyse`, `analyze_mix`, `check_mix`, `listen`.

## `render_wav`

Render the song to a 16-bit stereo WAV file (defaults to &lt;project>.wav) so it can be heard.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `output_path` | string | no | where to write the .wav (relative = next to the project) |
| `sample_rate` | integer | no | default 44100 |

Example:

```json
{"output_path": "song.wav"}
```

Also answers to `render`, `export_wav`.
