//! Compact musical notations that let a caller (typically an AI) write music
//! without doing note arithmetic: chord symbols (`"Am F C G"`), melodies
//! (`"C4/1 D4/0.5 r/0.5 E4/2"`) and drum step patterns (`"x...x...x...x..."`).
//!
//! Everything here produces plain [`Note`]s with starts relative to an
//! arbitrary origin; placing them in a clip is the caller's job.

use anyhow::{anyhow, bail, Result};

use crate::model::Note;
use crate::pitch;

/// Chord qualities understood by [`parse_chord`], as (suffix, intervals).
const QUALITIES: &[(&str, &[u8])] = &[
    ("maj9", &[0, 4, 7, 11, 14]),
    ("maj7", &[0, 4, 7, 11]),
    ("min7b5", &[0, 3, 6, 10]),
    ("m7b5", &[0, 3, 6, 10]),
    ("min7", &[0, 3, 7, 10]),
    ("min9", &[0, 3, 7, 10, 14]),
    ("dim7", &[0, 3, 6, 9]),
    ("add9", &[0, 4, 7, 14]),
    ("sus2", &[0, 2, 7]),
    ("sus4", &[0, 5, 7]),
    ("7sus4", &[0, 5, 7, 10]),
    ("maj", &[0, 4, 7]),
    ("min", &[0, 3, 7]),
    ("dim", &[0, 3, 6]),
    ("aug", &[0, 4, 8]),
    ("sus", &[0, 5, 7]),
    ("m9", &[0, 3, 7, 10, 14]),
    ("m7", &[0, 3, 7, 10]),
    ("m6", &[0, 3, 7, 9]),
    ("M7", &[0, 4, 7, 11]),
    ("ø", &[0, 3, 6, 10]),
    ("°", &[0, 3, 6]),
    ("+", &[0, 4, 8]),
    ("5", &[0, 7]),
    ("6", &[0, 4, 7, 9]),
    ("7", &[0, 4, 7, 10]),
    ("9", &[0, 4, 7, 10, 14]),
    ("m", &[0, 3, 7]),
    ("-", &[0, 3, 7]),
    ("", &[0, 4, 7]),
];

/// Example chord symbols, for help and error messages.
pub const CHORD_EXAMPLES: &str =
    "C, Am, F#m, Bb, G7, Cmaj7, Dm7, Bm7b5, Esus4, Asus2, Cdim, Caug, C6, Cadd9, C5, C/E";

/// A parsed chord: pitch classes relative to C (root first) plus an optional
/// slash bass pitch class.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chord {
    pub root: u8,
    pub intervals: Vec<u8>,
    pub bass: Option<u8>,
}

fn pitch_class(s: &str) -> Option<(u8, &str)> {
    let mut chars = s.char_indices();
    let (_, c) = chars.next()?;
    let base: i32 = match c.to_ascii_uppercase() {
        'C' => 0,
        'D' => 2,
        'E' => 4,
        'F' => 5,
        'G' => 7,
        'A' => 9,
        'B' => 11,
        _ => return None,
    };
    let rest = &s[c.len_utf8()..];
    let (acc, rest) = if let Some(r) = rest.strip_prefix(['#', '♯']) {
        (1, r)
    } else if let Some(r) = rest.strip_prefix(['b', '♭']) {
        (-1, r)
    } else {
        (0, rest)
    };
    Some(((base + acc).rem_euclid(12) as u8, rest))
}

/// Parse a chord symbol such as `"Am"`, `"F#m7"`, `"Bbmaj7"` or `"C/E"`.
pub fn parse_chord(symbol: &str) -> Result<Chord> {
    let bad = || {
        anyhow!(
            "unknown chord symbol {symbol:?}; use a root A-G with optional #/b and a quality, e.g. {CHORD_EXAMPLES}"
        )
    };
    let (main, bass) = match symbol.split_once('/') {
        Some((m, b)) => {
            let (pc, rest) = pitch_class(b).ok_or_else(bad)?;
            if !rest.is_empty() {
                return Err(bad());
            }
            (m, Some(pc))
        }
        None => (symbol, None),
    };
    let (root, quality) = pitch_class(main).ok_or_else(bad)?;
    let intervals = QUALITIES
        .iter()
        .find(|(q, _)| *q == quality)
        .or_else(|| {
            // Be lenient about case for the spelled-out qualities ("Cmin", "CMAJ7").
            let lower = quality.to_ascii_lowercase();
            QUALITIES.iter().find(|(q, _)| q.len() > 1 && *q == lower)
        })
        .map(|(_, iv)| iv.to_vec())
        .ok_or_else(bad)?;
    Ok(Chord {
        root,
        intervals,
        bass,
    })
}

impl Chord {
    /// MIDI pitches in close position with the root placed in the octave
    /// starting at F below `C<octave>` (so octave 4 gives roots F3..E4, which
    /// keeps every chord around middle C). A slash bass goes an octave below.
    pub fn voicing(&self, octave: i32) -> Vec<u8> {
        let low = 12 * (octave + 1) - 7; // F3 = 53 for octave 4
        let root = low + (self.root as i32 - low).rem_euclid(12);
        let mut out: Vec<u8> = self
            .bass
            .map(|b| {
                let below = root - 12;
                below + (b as i32 - below).rem_euclid(12)
            })
            .into_iter()
            .chain(self.intervals.iter().map(|&iv| root + iv as i32))
            .filter_map(|p| u8::try_from(p).ok().filter(|p| *p <= 127))
            .collect();
        out.dedup();
        out
    }
}

/// How [`chords_to_notes`] plays each chord.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChordStyle {
    /// Hold the whole chord for its full length.
    Block,
    /// Play the chord tones one after another, upwards, in eighth notes.
    Arpeggio,
    /// Re-strike the chord on every beat.
    Pulse,
}

impl ChordStyle {
    pub const NAMES: &str = "block, arpeggio, pulse";

    pub fn parse(s: &str) -> Result<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "" | "block" | "hold" | "sustain" | "pad" => Ok(Self::Block),
            "arpeggio" | "arp" | "arpeggiated" | "broken" => Ok(Self::Arpeggio),
            "pulse" | "stab" | "stabs" | "beats" | "quarter" | "strum" => Ok(Self::Pulse),
            other => bail!("unknown chord style {other:?}; use one of {}", Self::NAMES),
        }
    }
}

/// A chord progression, `"Am F C G"`: whitespace, commas or `|` separate
/// chords; `"Am:2"` lasts 2 beats (otherwise `default_beats`); `"r"`, `"-"`
/// or `"N.C."` is a rest. Returns (chord or None for rest, beats).
pub fn parse_progression(text: &str, default_beats: f64) -> Result<Vec<(Option<Chord>, f64)>> {
    let mut out = Vec::new();
    for token in text
        .split(|c: char| c.is_whitespace() || c == ',' || c == '|')
        .filter(|t| !t.is_empty())
    {
        let (sym, beats) = match token.split_once(':') {
            Some((s, b)) => (
                s,
                parse_number(b).ok_or_else(|| {
                    anyhow!("bad duration in {token:?}; write e.g. \"Am:2\" for 2 beats")
                })?,
            ),
            None => (token, default_beats),
        };
        if !(beats.is_finite() && beats > 0.0) {
            bail!("chord duration must be > 0 beats (in {token:?})");
        }
        let chord = match sym.to_ascii_lowercase().as_str() {
            "r" | "rest" | "-" | "n.c." | "nc" | "x" => None,
            _ => Some(parse_chord(sym)?),
        };
        out.push((chord, beats));
    }
    if out.is_empty() {
        bail!("no chords given; write e.g. \"Am F C G\" (one chord per bar) or \"Am:2 F:2\"");
    }
    Ok(out)
}

/// Turn a progression into notes starting at beat 0.
pub fn chords_to_notes(
    progression: &[(Option<Chord>, f64)],
    style: ChordStyle,
    octave: i32,
    velocity: u8,
) -> Vec<Note> {
    let note = |pitch, start, duration| Note {
        pitch,
        start,
        duration,
        velocity,
    };
    let mut notes = Vec::new();
    let mut t = 0.0;
    for (chord, beats) in progression {
        if let Some(chord) = chord {
            let voicing = chord.voicing(octave);
            match style {
                ChordStyle::Block => notes.extend(voicing.iter().map(|&p| note(p, t, *beats))),
                ChordStyle::Pulse => {
                    let mut s = 0.0;
                    while s < beats - 1e-9 {
                        let d = (beats - s).min(1.0);
                        notes.extend(voicing.iter().map(|&p| note(p, t + s, d * 0.9)));
                        s += 1.0;
                    }
                }
                ChordStyle::Arpeggio => {
                    // Up through the chord and on to the root an octave higher.
                    let mut tones = voicing.clone();
                    if let Some(&top) = voicing.iter().find(|&&p| p % 12 == chord.root) {
                        tones.push(top.saturating_add(12).min(127));
                    }
                    let mut s = 0.0;
                    let mut i = 0;
                    while s < beats - 1e-9 {
                        let d = (beats - s).min(0.5);
                        notes.push(note(tones[i % tones.len()], t + s, d));
                        s += 0.5;
                        i += 1;
                    }
                }
            }
        }
        t += beats;
    }
    notes
}

/// Parse `"0.5"`, `"2"` or a fraction like `"1/2"`.
fn parse_number(s: &str) -> Option<f64> {
    let s = s.trim();
    match s.split_once('/') {
        Some((a, b)) => {
            let (a, b): (f64, f64) = (a.trim().parse().ok()?, b.trim().parse().ok()?);
            (b != 0.0).then(|| a / b)
        }
        None => s.parse().ok(),
    }
}

/// Drum names accepted wherever a drum pitch is expected, with their GM note.
pub const DRUMS: &[(&str, u8)] = &[
    ("kick", 36),
    ("snare", 38),
    ("clap", 39),
    ("hat", 42),
    ("open_hat", 46),
    ("tom_low", 41),
    ("tom_mid", 45),
    ("tom_high", 48),
    ("crash", 49),
    ("ride", 51),
];

/// Resolve a drum name (`"kick"`, `"Hi-Hat"`, `"open hat"`, `"bd"`) or a MIDI
/// number/pitch name to a General MIDI drum note.
pub fn drum_note(name: &str) -> Option<u8> {
    let norm: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect();
    let gm = match norm.as_str() {
        "kick" | "bd" | "bassdrum" | "kickdrum" | "k" => 36,
        "snare" | "sd" | "sn" | "snaredrum" | "s" => 38,
        "clap" | "handclap" | "cp" => 39,
        "hat" | "hh" | "hihat" | "hats" | "closedhat" | "closedhihat" | "ch" | "chh" | "h" => 42,
        "openhat" | "openhihat" | "oh" | "ohh" | "o" => 46,
        "tom" | "tomlow" | "lowtom" | "floortom" | "lt" => 41,
        "tommid" | "midtom" | "mt" => 45,
        "tomhigh" | "hightom" | "hitom" | "ht" => 48,
        "crash" | "cymbal" | "cr" => 49,
        "ride" | "rd" => 51,
        _ => return pitch::parse(name),
    };
    Some(gm)
}

fn drum_names() -> String {
    DRUMS
        .iter()
        .map(|(n, p)| format!("{n} ({p})"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Parse one drum lane: `x` hit, `X` accent, `o`/`g` soft (ghost) hit;
/// `.`, `-`, `_` or `0` rest. Spaces and `|` are ignored. Returns the
/// velocity per step (`None` = silent).
pub fn parse_steps(steps: &str, velocity: u8) -> Result<Vec<Option<u8>>> {
    steps
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '|')
        .map(|c| match c {
            'x' | '1' | '*' => Ok(Some(velocity)),
            'X' | '!' => Ok(Some(127)),
            'o' | 'g' => Ok(Some((velocity / 2).max(1))),
            '.' | '-' | '_' | '0' => Ok(None),
            other => bail!(
                "bad step {other:?} in {steps:?}; use x = hit, X = accent, o = soft hit, . = rest, e.g. \"x...x...x...x...\""
            ),
        })
        .collect()
}

/// Build notes for a drum pattern of named lanes. Each lane is tiled to fill
/// `total_steps` steps of `step_beats` each. Returns notes starting at 0.
pub fn drum_pattern_to_notes(
    lanes: &[(String, String)],
    total_steps: usize,
    step_beats: f64,
    velocity: u8,
) -> Result<Vec<Note>> {
    let mut notes = Vec::new();
    for (name, steps) in lanes {
        let pitch = drum_note(name).ok_or_else(|| {
            anyhow!(
                "unknown drum {name:?}; use one of {} or a MIDI note number",
                drum_names()
            )
        })?;
        let lane = parse_steps(steps, velocity)?;
        if lane.is_empty() {
            bail!("drum {name:?} has an empty pattern; write e.g. \"x...x...x...x...\"");
        }
        for i in 0..total_steps {
            if let Some(v) = lane[i % lane.len()] {
                notes.push(Note {
                    pitch,
                    start: i as f64 * step_beats,
                    duration: step_beats,
                    velocity: v,
                });
            }
        }
    }
    Ok(notes)
}

/// Example melody, for help and error messages.
pub const MELODY_EXAMPLE: &str = "C4/1 D4/0.5 E4/0.5 r/1 G4/2 C4+E4+G4/4";

/// Parse a melody: space-separated tokens `PITCH[/BEATS][@VELOCITY]`.
/// PITCH is a name (`C4`, `F#3`, `Bb2`), a MIDI number, a drum name
/// (`kick`), several joined with `+` for a chord (`C4+E4+G4`), or `r` for a
/// rest. BEATS defaults to the previous token's duration (1 at the start)
/// and may be a fraction (`1/2`). Commas and `|` bar lines are ignored.
/// Returns the notes (starting at beat 0) and the total length in beats,
/// including trailing rests.
pub fn parse_melody(text: &str, velocity: u8) -> Result<(Vec<Note>, f64)> {
    let mut notes = Vec::new();
    let mut t = 0.0;
    let mut dur = 1.0;
    for token in text
        .split(|c: char| c.is_whitespace() || c == ',' || c == '|')
        .filter(|t| !t.is_empty())
    {
        let (rest, vel) = match token.split_once('@') {
            Some((r, v)) => (
                r,
                v.parse::<u8>()
                    .ok()
                    .filter(|v| (1..=127).contains(v))
                    .ok_or_else(|| {
                        anyhow!("bad velocity in {token:?}; use 1-127, e.g. \"C4/1@80\"")
                    })?,
            ),
            None => (token, velocity),
        };
        let (pitches, d) = match rest.split_once('/') {
            Some((p, d)) => (
                p,
                parse_number(d)
                    .filter(|d| d.is_finite() && *d > 0.0)
                    .ok_or_else(|| {
                        anyhow!("bad duration in {token:?}; write beats after '/', e.g. \"C4/0.5\" (1 = quarter note)")
                    })?,
            ),
            None => (rest, dur),
        };
        dur = d;
        if !matches!(pitches.to_ascii_lowercase().as_str(), "r" | "rest" | "_") {
            for p in pitches.split('+') {
                let pitch = pitch::parse(p).or_else(|| drum_note(p)).ok_or_else(|| {
                    anyhow!(
                        "bad pitch {p:?} in {token:?}; use a note name with octave like C4, F#3, Bb2 (C4 = middle C = 60), a MIDI number, or r for a rest. Example: \"{MELODY_EXAMPLE}\""
                    )
                })?;
                notes.push(Note {
                    pitch,
                    start: t,
                    duration: d,
                    velocity: vel,
                });
            }
        }
        t += d;
    }
    if notes.is_empty() && t == 0.0 {
        bail!("empty melody; write e.g. \"{MELODY_EXAMPLE}\"");
    }
    Ok((notes, t))
}

/// Format notes in the melody-like notation `start:PITCH/BEATS` used when
/// showing clips to an AI, e.g. `0:C4/1 1:E4/0.5@80`.
pub fn format_notes(notes: &[Note], drums: bool) -> String {
    notes
        .iter()
        .map(|n| {
            let name = if drums {
                DRUMS
                    .iter()
                    .find(|(_, p)| *p == n.pitch)
                    .map(|(name, _)| name.to_string())
                    .unwrap_or_else(|| n.pitch.to_string())
            } else {
                pitch::name(n.pitch)
            };
            let vel = if n.velocity == 100 {
                String::new()
            } else {
                format!("@{}", n.velocity)
            };
            format!("{}:{name}/{}{vel}", n.start, n.duration)
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pitches(notes: &[Note]) -> Vec<u8> {
        notes.iter().map(|n| n.pitch).collect()
    }

    #[test]
    fn chord_symbols() {
        let c = parse_chord("Am").unwrap();
        assert_eq!((c.root, c.intervals.as_slice()), (9, &[0u8, 3, 7][..]));
        assert_eq!(parse_chord("F#m7").unwrap().intervals, vec![0, 3, 7, 10]);
        assert_eq!(parse_chord("Bbmaj7").unwrap().root, 10);
        assert_eq!(parse_chord("Cmin").unwrap().intervals, vec![0, 3, 7]);
        assert_eq!(parse_chord("Bm7b5").unwrap().intervals, vec![0, 3, 6, 10]);
        assert_eq!(parse_chord("C/E").unwrap().bass, Some(4));
        assert!(parse_chord("H7").is_err());
        assert!(parse_chord("Cxyz")
            .unwrap_err()
            .to_string()
            .contains("Cmaj7"));
    }

    #[test]
    fn voicing_stays_near_middle_c() {
        assert_eq!(parse_chord("Am").unwrap().voicing(4), vec![57, 60, 64]); // A3 C4 E4
        assert_eq!(parse_chord("C").unwrap().voicing(4), vec![60, 64, 67]);
        assert_eq!(parse_chord("G").unwrap().voicing(4), vec![55, 59, 62]);
        assert_eq!(parse_chord("C").unwrap().voicing(3), vec![48, 52, 55]);
        assert_eq!(parse_chord("C/E").unwrap().voicing(4), vec![52, 60, 64, 67]);
    }

    #[test]
    fn progression_and_styles() {
        let prog = parse_progression("Am F | C:2 G:2 r", 4.0).unwrap();
        let beats: Vec<f64> = prog.iter().map(|p| p.1).collect();
        assert_eq!(beats, vec![4.0, 4.0, 2.0, 2.0, 4.0]);
        assert!(prog[4].0.is_none());

        let block = chords_to_notes(&prog, ChordStyle::Block, 4, 90);
        assert_eq!(block.len(), 12);
        assert_eq!(block[3].start, 4.0);
        assert_eq!(block[0].duration, 4.0);

        let one = parse_progression("C", 4.0).unwrap();
        let arp = chords_to_notes(&one, ChordStyle::Arpeggio, 4, 90);
        assert_eq!(arp.len(), 8);
        assert_eq!(pitches(&arp[..4]), vec![60, 64, 67, 72]);
        let pulse = chords_to_notes(&one, ChordStyle::Pulse, 4, 90);
        assert_eq!(pulse.len(), 12);
        assert_eq!(pulse[3].start, 1.0);
        assert!(ChordStyle::parse("arp").is_ok() && ChordStyle::parse("jazz").is_err());
        assert!(parse_progression("  ", 4.0).is_err());
        assert!(parse_progression("Am:0", 4.0).is_err());
    }

    #[test]
    fn melody_notation() {
        let (m, len) = parse_melody("C4/1 D4/0.5 E4 r/1 G4/1/2 C4+E4/2@80", 100).unwrap();
        assert_eq!(len, 5.5);
        assert_eq!(pitches(&m), vec![60, 62, 64, 67, 60, 64]);
        let starts: Vec<f64> = m.iter().map(|n| n.start).collect();
        assert_eq!(starts, vec![0.0, 1.0, 1.5, 3.0, 3.5, 3.5]);
        assert_eq!(m[2].duration, 0.5); // inherits previous duration
        assert_eq!(m[3].duration, 0.5);
        assert_eq!(m[5].velocity, 80);
        assert_eq!(parse_melody("60/2 kick", 100).unwrap().0[1].pitch, 36);
        assert_eq!(parse_melody("C4/1 r/3", 100).unwrap().1, 4.0);
        let err = parse_melody("C4/1 Q/1", 100).unwrap_err().to_string();
        assert!(err.contains("\"Q\"") && err.contains("C4/1"), "{err}");
        assert!(parse_melody("C4/0", 100).is_err());
        assert!(parse_melody("", 100).is_err());
    }

    #[test]
    fn drum_patterns() {
        assert_eq!(drum_note("Hi-Hat"), Some(42));
        assert_eq!(drum_note("open hat"), Some(46));
        assert_eq!(drum_note("BD"), Some(36));
        assert_eq!(drum_note("37"), Some(37));
        assert_eq!(drum_note("cowbell"), None);
        let lanes = vec![
            ("kick".to_string(), "x...x...x...x...".to_string()),
            ("hat".to_string(), "x.".to_string()),
            ("snare".to_string(), "....X... ....o...".to_string()),
        ];
        let notes = drum_pattern_to_notes(&lanes, 32, 0.25, 100).unwrap();
        let count = |p| notes.iter().filter(|n| n.pitch == p).count();
        assert_eq!((count(36), count(42), count(38)), (8, 16, 4));
        let snares: Vec<_> = notes.iter().filter(|n| n.pitch == 38).collect();
        assert_eq!((snares[0].start, snares[0].velocity), (1.0, 127));
        assert_eq!(snares[1].velocity, 50);
        let bad = vec![("cowbell".to_string(), "x".to_string())];
        let err = drum_pattern_to_notes(&bad, 16, 0.25, 100)
            .unwrap_err()
            .to_string();
        assert!(err.contains("kick (36)"), "{err}");
        assert!(parse_steps("x.y.", 100).is_err());
    }

    #[test]
    fn format_round_trip() {
        let (m, _) = parse_melody("C4/1 E4/0.5@80", 100).unwrap();
        assert_eq!(format_notes(&m, false), "0:C4/1 1:E4/0.5@80");
        let (d, _) = parse_melody("kick/0.5 snare/0.5", 100).unwrap();
        assert_eq!(format_notes(&d, true), "0:kick/0.5 0.5:snare/0.5");
    }
}
