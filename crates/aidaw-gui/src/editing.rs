//! The note and clip commands behind the editing shortcuts: copy, paste,
//! duplicate, delete, move and quantize. They work on plain data (a clip
//! and the indexes of the selected notes), so they are tested without a
//! window; `App` wires them to the keyboard and the selection.

use std::collections::BTreeSet;

use aidaw_core::transform;
use aidaw_core::{Clip, Note};
use serde::{Deserialize, Serialize};

/// What Ctrl+C puts on the system clipboard (as JSON), so Ctrl+V can bring
/// it back, even in another AIDAW window.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Copied {
    /// Notes, moved by whole bars so the earliest is in the first bar.
    #[serde(rename = "aidaw_notes")]
    Notes(Vec<Note>),
    #[serde(rename = "aidaw_clip")]
    Clip(Clip),
}

/// Everything Ctrl+V understands: what Ctrl+C copied, or notes written
/// somewhere else, e.g. by an AI in a chat window: a plain list, or an
/// object with a `notes` list (the shape `add_notes` takes).
#[derive(Deserialize)]
#[serde(untagged)]
enum Pasted {
    Ours(Copied),
    List(Vec<Note>),
    Wrapped { notes: Vec<Note> },
}

/// The JSON value at the start of `text`, ignoring whatever follows it.
fn first_value(text: &str) -> Option<Pasted> {
    serde_json::Deserializer::from_str(text)
        .into_iter()
        .next()?
        .ok()
}

impl Copied {
    pub fn to_text(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    /// `None` when the clipboard holds something else (ordinary text).
    /// Notes that could not be played (negative or missing times) are
    /// dropped.
    pub fn from_text(text: &str) -> Option<Copied> {
        // A whole chat answer has words, links or a ``` fence around the
        // list, so try where each bracket opens until notes turn up. Only
        // the first few: a huge paste of code shouldn't freeze the window.
        let starts = text.match_indices(['[', '{']).map(|(i, _)| i).take(64);
        std::iter::once(0)
            .chain(starts)
            .find_map(|i| match first_value(&text[i..])? {
                Pasted::Ours(Copied::Notes(notes))
                | Pasted::List(notes)
                | Pasted::Wrapped { notes } => {
                    let notes = playable(notes);
                    (!notes.is_empty()).then_some(Copied::Notes(notes))
                }
                Pasted::Ours(Copied::Clip(mut clip)) => {
                    clip.notes = playable(std::mem::take(&mut clip.notes));
                    (clip.length.is_finite() && clip.length > 0.0).then_some(Copied::Clip(clip))
                }
            })
    }
}

fn playable(notes: Vec<Note>) -> Vec<Note> {
    notes
        .into_iter()
        .filter(|n| {
            n.start.is_finite() && n.start >= 0.0 && n.duration.is_finite() && n.duration > 0.0
        })
        .map(|n| Note {
            velocity: n.velocity.clamp(1, 127),
            ..n
        })
        .collect()
}

/// Start of the earliest and end of the latest selected note.
pub fn span(clip: &Clip, sel: &BTreeSet<usize>) -> Option<(f64, f64)> {
    sel.iter()
        .filter_map(|&i| clip.notes.get(i))
        .map(|n| (n.start, n.start + n.duration))
        .reduce(|(a0, a1), (b0, b1)| (a0.min(b0), a1.max(b1)))
}

/// The start of the bar `beat` is in, counting bars from beat 0.
pub fn bar_start(beat: f64, bar: f64) -> f64 {
    if bar > 0.0 {
        (beat / bar).floor() * bar
    } else {
        beat
    }
}

/// Move notes by whole bars so the earliest one is in the first bar. Each
/// note keeps its place in the bar, so an off-beat pattern stays off-beat.
pub fn to_first_bar(notes: &mut [Note], bar: f64) {
    let first = notes.iter().map(|n| n.start).fold(f64::INFINITY, f64::min);
    if first.is_finite() {
        let shift = bar_start(first, bar);
        for n in notes {
            n.start -= shift;
        }
    }
}

/// The selected notes, ready for the clipboard (see [`to_first_bar`]).
pub fn copy_notes(clip: &Clip, sel: &BTreeSet<usize>, bar: f64) -> Vec<Note> {
    let mut notes: Vec<Note> = sel
        .iter()
        .filter_map(|&i| clip.notes.get(i))
        .cloned()
        .collect();
    to_first_bar(&mut notes, bar);
    notes
}

/// Add `notes` shifted by `at` beats, making the clip longer (in whole
/// bars, but at most `room` beats long, where the next clip on its track
/// starts) when they run past its end. Returns the new notes' indexes.
pub fn paste_notes(
    clip: &mut Clip,
    notes: &[Note],
    at: f64,
    bar: f64,
    room: f64,
) -> BTreeSet<usize> {
    let first = clip.notes.len();
    clip.notes.extend(notes.iter().map(|n| Note {
        start: (n.start + at).max(0.0),
        ..n.clone()
    }));
    fit_length(clip, bar, room);
    (first..clip.notes.len()).collect()
}

/// Whether any of these notes starts past the end of the clip, where it
/// won't be heard.
pub fn any_silent(clip: &Clip, notes: &BTreeSet<usize>) -> bool {
    notes
        .iter()
        .filter_map(|&i| clip.notes.get(i))
        .any(|n| n.start >= clip.length - 1e-9)
}

/// How far a duplicate goes after the original, for notes spanning `span`
/// beats: whole bars for phrases, whole beats for short figures and grid
/// steps for single short notes.
pub fn repeat_offset(span: f64, grid: f64, bar: f64) -> f64 {
    let unit = if span > bar / 2.0 {
        bar
    } else if span >= 1.0 {
        1.0
    } else {
        grid.max(1.0 / 64.0)
    };
    ((span - 1e-9) / unit).ceil().max(1.0) * unit
}

/// Copy the selected notes to right after themselves. Returns the copies'
/// indexes, which become the selection, so pressing it again keeps going.
pub fn duplicate_notes(
    clip: &mut Clip,
    sel: &BTreeSet<usize>,
    grid: f64,
    bar: f64,
    room: f64,
) -> BTreeSet<usize> {
    let Some((start, end)) = span(clip, sel) else {
        return BTreeSet::new();
    };
    let copies: Vec<Note> = sel
        .iter()
        .filter_map(|&i| clip.notes.get(i))
        .cloned()
        .collect();
    let offset = repeat_offset(end - start, grid, bar);
    paste_notes(clip, &copies, offset, bar, room)
}

pub fn delete_notes(clip: &mut Clip, sel: &BTreeSet<usize>) {
    let mut i = 0;
    clip.notes.retain(|_| {
        i += 1;
        !sel.contains(&(i - 1))
    });
}

/// Where the indexes in `sel` point once the notes in `gone` are deleted.
pub fn after_delete(sel: &BTreeSet<usize>, gone: &BTreeSet<usize>) -> BTreeSet<usize> {
    sel.iter()
        .filter(|i| !gone.contains(i))
        .map(|&i| i - gone.range(..i).count())
        .collect()
}

/// Move the selected notes `beats` later (earlier when negative). Changes
/// nothing and returns false if one would start before the clip does.
pub fn move_notes(clip: &mut Clip, sel: &BTreeSet<usize>, beats: f64) -> bool {
    let notes = sel.iter().filter_map(|&i| clip.notes.get(i));
    if notes.clone().any(|n| n.start + beats < -1e-9) || sel.is_empty() {
        return false;
    }
    for &i in sel {
        if let Some(n) = clip.notes.get_mut(i) {
            n.start = (n.start + beats).max(0.0);
        }
    }
    true
}

/// Give every selected note the pitch `step` returns for it. Changes
/// nothing and returns false if `step` has no answer for one of them
/// (it would leave the keyboard).
pub fn transpose_notes(
    clip: &mut Clip,
    sel: &BTreeSet<usize>,
    step: impl Fn(u8) -> Option<u8>,
) -> bool {
    let Some(pitches) = sel
        .iter()
        .filter_map(|&i| clip.notes.get(i))
        .map(|n| step(n.pitch))
        .collect::<Option<Vec<u8>>>()
    else {
        return false;
    };
    if pitches.is_empty() {
        return false;
    }
    let len = clip.notes.len();
    for (&i, p) in sel.iter().filter(|&&i| i < len).zip(pitches) {
        clip.notes[i].pitch = p;
    }
    true
}

/// Snap note starts to the grid: the selected notes, or every note when
/// nothing is selected. Lengths are kept. Returns how many notes moved.
pub fn quantize_notes(clip: &mut Clip, sel: &BTreeSet<usize>, grid: f64) -> usize {
    let origin = clip.start;
    transform::quantize(chosen(clip, sel), origin, grid, 1.0, 0.5)
}

/// Nudge the timing and velocity of the selected notes (or every note when
/// nothing is selected) a little at random, the way a player would vary
/// them. Returns how many notes changed.
pub fn humanize_notes(clip: &mut Clip, sel: &BTreeSet<usize>, seed: u64) -> usize {
    transform::humanize(chosen(clip, sel), 0.02, 8, seed)
}

/// The selected notes, or all of them when nothing is selected.
fn chosen<'a>(clip: &'a mut Clip, sel: &'a BTreeSet<usize>) -> impl Iterator<Item = &'a mut Note> {
    clip.notes
        .iter_mut()
        .enumerate()
        .filter(move |(i, _)| sel.is_empty() || sel.contains(i))
        .map(|(_, n)| n)
}

/// The notes touching the box between beats `b0` and `b1` and key rows
/// `r0` and `r1` (row 0 is the top key, pitch 127; row 127 is pitch 0).
pub fn in_box(notes: &[Note], (b0, b1): (f64, f64), (r0, r1): (f32, f32)) -> BTreeSet<usize> {
    let (blo, bhi) = (b0.min(b1), b0.max(b1));
    let (rlo, rhi) = (r0.min(r1), r0.max(r1));
    notes
        .iter()
        .enumerate()
        .filter(|(_, n)| {
            let row = 127.0 - n.pitch.min(127) as f32;
            n.start < bhi && n.start + n.duration > blo && row < rhi && row + 1.0 > rlo
        })
        .map(|(i, _)| i)
        .collect()
}

/// The first place at or after `from` where a clip `length` beats long fits
/// between the clips already on a track, so a copy never plays on top of
/// another part.
pub fn free_spot(clips: &[Clip], from: f64, length: f64) -> f64 {
    let mut at = from;
    while let Some(end) = clips
        .iter()
        .filter(|c| c.start < at + length - 1e-9 && c.end() > at + 1e-9)
        .map(|c| c.end())
        .reduce(f64::max)
    {
        at = end;
    }
    at
}

/// Make the clip long enough for all its notes, in whole bars, but no
/// longer than `room`, so it doesn't grow over the next clip.
fn fit_length(clip: &mut Clip, bar: f64, room: f64) {
    let end = clip
        .notes
        .iter()
        .map(|n| n.start + n.duration)
        .fold(0.0, f64::max);
    if end > clip.length + 1e-9 && bar > 0.0 {
        let whole_bars = ((end - 1e-9) / bar).ceil() * bar;
        clip.length = whole_bars.min(room).max(clip.length);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(pitch: u8, start: f64, duration: f64) -> Note {
        Note {
            pitch,
            start,
            duration,
            velocity: 100,
        }
    }

    /// A one-bar clip: C4 E4 G4 on beats 0, 1, 2.
    fn clip() -> Clip {
        Clip {
            id: "c1".into(),
            name: String::new(),
            start: 0.0,
            length: 4.0,
            notes: vec![note(60, 0.0, 1.0), note(64, 1.0, 1.0), note(67, 2.0, 1.5)],
        }
    }

    fn sel(i: &[usize]) -> BTreeSet<usize> {
        i.iter().copied().collect()
    }

    #[test]
    fn copy_and_paste_keep_the_place_in_the_bar_and_grow_the_clip() {
        let mut c = clip();
        let copied = copy_notes(&c, &sel(&[1, 2]), 4.0);
        assert_eq!(copied, [note(64, 1.0, 1.0), note(67, 2.0, 1.5)]);
        // Through the clipboard and back.
        let text = Copied::Notes(copied.clone()).to_text();
        assert_eq!(
            Copied::from_text(&text),
            Some(Copied::Notes(copied.clone()))
        );
        assert_eq!(Copied::from_text("just some text"), None);

        // Into the second bar: same beats of the bar, clip grows to 2 bars.
        let new = paste_notes(&mut c, &copied, 4.0, 4.0, f64::INFINITY);
        assert_eq!(new, sel(&[3, 4]));
        assert_eq!(c.notes[3], note(64, 5.0, 1.0));
        assert_eq!(c.notes[4], note(67, 6.0, 1.5));
        assert_eq!(c.length, 8.0, "grows to whole bars to fit the paste");

        // Notes from bar 3 are copied as if they were in bar 1.
        let mut late = vec![note(42, 8.5, 0.25), note(42, 9.5, 0.25)];
        to_first_bar(&mut late, 4.0);
        assert_eq!((late[0].start, late[1].start), (0.5, 1.5));
    }

    #[test]
    fn notes_pasted_from_a_chat_are_understood() {
        let chat = "```json\n[{\"pitch\": \"C4\", \"start\": 0, \"duration\": 1},\n {\"pitch\": 64, \"start\": 1, \"duration\": 0.5, \"velocity\": 0}]\n```";
        let Some(Copied::Notes(notes)) = Copied::from_text(chat) else {
            panic!("not understood");
        };
        assert_eq!(notes.len(), 2);
        assert_eq!((notes[0].pitch, notes[0].velocity), (60, 100));
        assert_eq!(notes[1].velocity, 1, "clamped");

        let answer = "Sure! Here it is:\n[{\"pitch\": \"E4\", \"start\": 0.5, \"duration\": 0.5}]\nHave fun.";
        assert!(matches!(Copied::from_text(answer), Some(Copied::Notes(n)) if n[0].pitch == 64));
        // Brackets that aren't notes, before and after the list.
        let links = "**[Verse]** in [A minor](https://example.com):\n```json\n[{\"pitch\": \"A4\", \"start\": 0, \"duration\": 1}]\n```\n[1] a footnote";
        assert!(
            matches!(Copied::from_text(links), Some(Copied::Notes(n)) if n.len() == 1 && n[0].pitch == 69)
        );
        // Notes copied here, sent through a chat and pasted back.
        let ours = format!(
            "Varied it:\n```\n{}\n```",
            Copied::Notes(vec![note(62, 0.0, 1.0)]).to_text()
        );
        assert!(matches!(Copied::from_text(&ours), Some(Copied::Notes(n)) if n[0].pitch == 62));
        let wrapped = r#"{"notes": [{"pitch": "A3", "start": 2, "duration": 2}]}"#;
        assert!(matches!(Copied::from_text(wrapped), Some(Copied::Notes(n)) if n[0].pitch == 57));
        // Unplayable notes are dropped; nothing playable means nothing to paste.
        let bad = r#"[{"pitch": 60, "start": -1, "duration": 1}, {"pitch": 60, "start": 0, "duration": 0}]"#;
        assert_eq!(Copied::from_text(bad), None);
        assert_eq!(Copied::from_text("[]"), None);
        assert_eq!(Copied::from_text(r#"{"pitch": 60}"#), None);
    }

    #[test]
    fn duplicate_repeats_phrases_by_the_bar_and_short_figures_by_the_beat() {
        assert_eq!(repeat_offset(3.5, 0.25, 4.0), 4.0);
        assert_eq!(repeat_offset(6.0, 0.25, 4.0), 8.0);
        assert_eq!(repeat_offset(1.5, 0.25, 4.0), 2.0);
        assert_eq!(repeat_offset(0.5, 0.25, 4.0), 0.5);
        assert_eq!(repeat_offset(0.1, 0.25, 4.0), 0.25);
        assert_eq!(repeat_offset(3.0, 0.5, 3.0), 3.0);

        let mut c = clip();
        let all = sel(&[0, 1, 2]);
        let copies = duplicate_notes(&mut c, &all, 0.25, 4.0, f64::INFINITY);
        assert_eq!(copies, sel(&[3, 4, 5]));
        let starts: Vec<f64> = c.notes.iter().map(|n| n.start).collect();
        assert_eq!(starts, [0.0, 1.0, 2.0, 4.0, 5.0, 6.0]);
        assert_eq!(c.length, 8.0);
        // Again, from the copies: the next bar.
        duplicate_notes(&mut c, &copies, 0.25, 4.0, f64::INFINITY);
        assert_eq!(c.notes[6].start, 8.0);
        assert!(duplicate_notes(&mut c, &BTreeSet::new(), 0.25, 4.0, f64::INFINITY).is_empty());
    }

    #[test]
    fn a_clip_does_not_grow_over_the_next_one() {
        // The next clip on the track starts 6 beats after this one.
        let mut c = clip();
        let copies = duplicate_notes(&mut c, &sel(&[0, 1, 2]), 0.25, 4.0, 6.0);
        assert_eq!(c.length, 6.0, "grew only up to the next clip");
        assert!(any_silent(&c, &copies), "the copy of G4 starts at beat 6");
        assert!(!any_silent(&c, &sel(&[0, 1, 2])));
        // Never shrinks a clip that already overlaps the next one.
        let mut c = clip();
        paste_notes(&mut c, &[note(60, 5.0, 1.0)], 0.0, 4.0, 2.0);
        assert_eq!(c.length, 4.0);
    }

    #[test]
    fn delete_move_and_transpose_selected_notes() {
        let mut c = clip();
        delete_notes(&mut c, &sel(&[0, 2]));
        assert_eq!(c.notes, [note(64, 1.0, 1.0)]);
        // The selection follows the notes that are left.
        assert_eq!(after_delete(&sel(&[1, 2]), &sel(&[0])), sel(&[0, 1]));
        assert_eq!(after_delete(&sel(&[0, 3, 5]), &sel(&[1, 3])), sel(&[0, 3]));

        let mut c = clip();
        assert!(move_notes(&mut c, &sel(&[1, 2]), 0.5));
        assert_eq!((c.notes[1].start, c.notes[2].start), (1.5, 2.5));
        assert!(
            !move_notes(&mut c, &sel(&[0, 1]), -0.5),
            "C4 would start before the clip"
        );
        assert_eq!(c.notes[1].start, 1.5, "nothing moved");

        assert!(transpose_notes(&mut c, &sel(&[0, 1]), |p| Some(p + 12)));
        assert_eq!((c.notes[0].pitch, c.notes[1].pitch), (72, 76));
        let up = |p: u8| p.checked_add(60).filter(|p| *p <= 127);
        assert!(!transpose_notes(&mut c, &sel(&[0, 2]), up));
        assert_eq!(c.notes[2].pitch, 67, "nothing moved");
    }

    #[test]
    fn quantize_snaps_starts_of_the_selection_or_everything() {
        let mut c = clip();
        c.notes[0].start = 0.1;
        c.notes[1].start = 1.2;
        c.notes[2].start = 2.4;
        assert_eq!(quantize_notes(&mut c, &sel(&[1]), 0.5), 1);
        assert_eq!(c.notes[1].start, 1.0);
        assert_eq!(c.notes[0].start, 0.1, "not selected");
        assert_eq!(quantize_notes(&mut c, &BTreeSet::new(), 0.5), 2);
        let starts: Vec<f64> = c.notes.iter().map(|n| n.start).collect();
        assert_eq!(starts, [0.0, 1.0, 2.5]);
        assert_eq!(c.notes[2].duration, 1.5, "lengths are kept");
    }

    #[test]
    fn a_box_selects_the_notes_it_touches() {
        let c = clip();
        // Rows: C4 (60) is row 67, E4 row 63, G4 row 60.
        assert_eq!(in_box(&c.notes, (0.5, 1.5), (62.5, 67.5)), sel(&[0, 1]));
        assert_eq!(
            in_box(&c.notes, (1.5, 0.5), (67.5, 62.5)),
            sel(&[0, 1]),
            "any corner"
        );
        assert_eq!(in_box(&c.notes, (0.0, 4.0), (60.2, 60.8)), sel(&[2]));
        assert!(in_box(&c.notes, (3.6, 4.0), (0.0, 127.0)).is_empty());
    }

    #[test]
    fn a_copied_clip_survives_the_clipboard() {
        let c = clip();
        let text = Copied::Clip(c.clone()).to_text();
        assert!(text.starts_with(r#"{"aidaw_clip":"#), "{text}");
        assert_eq!(Copied::from_text(&text), Some(Copied::Clip(c)));
    }

    #[test]
    fn a_copied_clip_goes_to_the_first_free_place() {
        let at = |start, length| Clip {
            start,
            length,
            ..clip()
        };
        // Bars 1-4 and 5-8 taken, a one-bar gap at bar 9, then bars 10-11.
        let track = [at(0.0, 16.0), at(16.0, 16.0), at(36.0, 8.0)];
        assert_eq!(free_spot(&track, 48.0, 16.0), 48.0, "free already");
        assert_eq!(free_spot(&track, 16.0, 16.0), 44.0, "the gap is too small");
        assert_eq!(free_spot(&track, 16.0, 4.0), 32.0, "fits the gap");
        assert_eq!(
            free_spot(&track, 4.0, 8.0),
            44.0,
            "past every clip in the way"
        );
        assert_eq!(free_spot(&[], 8.0, 4.0), 8.0);
    }
}
