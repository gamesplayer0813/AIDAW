//! Edits that rework music already in the song, behind the AI's arranging
//! tools: copy bars to another place, quantize (with swing) and humanize.

use anyhow::{bail, Result};

use crate::model::{Clip, Note, Project, Track};

/// Tolerance for comparing beat positions.
const EPS: f64 = 1e-9;

/// What [`copy_bars`] copied on one track.
#[derive(Debug, Clone, PartialEq)]
pub struct CopiedTrack {
    pub track_id: String,
    /// Notes in one copy.
    pub notes: usize,
}

/// The notes of `track` that start in `[from, to)` beats and are heard
/// (inside their clip), as (beats after `from`, note, clip name).
fn notes_in(track: &Track, from: f64, to: f64) -> Vec<(f64, Note, String)> {
    let mut found = Vec::new();
    for clip in &track.clips {
        for n in &clip.notes {
            let at = clip.start + n.start;
            if n.start >= 0.0 && n.start < clip.length && at >= from - EPS && at < to - EPS {
                found.push((at - from, n.clone(), clip.name.clone()));
            }
        }
    }
    found.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.pitch.cmp(&b.1.pitch)));
    found
}

/// The stretches of `[from, to)` that no clip on the track covers.
fn gaps(clips: &[Clip], from: f64, to: f64) -> Vec<(f64, f64)> {
    let mut taken: Vec<(f64, f64)> = clips
        .iter()
        .filter(|c| c.start < to - EPS && c.end() > from + EPS)
        .map(|c| (c.start, c.end()))
        .collect();
    taken.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut free = Vec::new();
    let mut at = from;
    for (start, end) in taken {
        if start > at + EPS {
            free.push((at, start));
        }
        at = at.max(end);
    }
    if at < to - EPS {
        free.push((at, to));
    }
    free
}

fn sort_notes(clip: &mut Clip) {
    clip.notes
        .sort_by(|a, b| a.start.total_cmp(&b.start).then(a.pitch.cmp(&b.pitch)));
}

/// Copy the notes that start in `[from, from + length)` beats to `to`, and
/// `times - 1` more times right after that, like copy and paste in a DAW.
///
/// Works on the tracks in `track_ids`, or, when that is empty, on every
/// track that has notes there. On each of those tracks the notes that
/// started in a destination span are replaced by the copy. Copies go into
/// the clip that covers them; parts of a span without a clip get a new
/// clip, so clips never end up on top of each other.
pub fn copy_bars(
    project: &mut Project,
    track_ids: &[String],
    from: f64,
    length: f64,
    to: f64,
    times: u32,
) -> Result<Vec<CopiedTrack>> {
    if !(from.is_finite() && from >= 0.0 && to.is_finite() && to >= 0.0) {
        bail!("from and to must be beats >= 0");
    }
    if !(length.is_finite() && length > 0.0) {
        bail!("length must be > 0 beats");
    }
    if times == 0 {
        bail!("times must be at least 1");
    }
    for id in track_ids {
        if project.track(id).is_none() {
            bail!("no track {id}");
        }
    }
    // Read every source first: a destination may overlap the source.
    let mut sources = Vec::new();
    for track in &project.tracks {
        let asked = track_ids.contains(&track.id);
        if !track_ids.is_empty() && !asked {
            continue;
        }
        let notes = notes_in(track, from, from + length);
        if notes.is_empty() {
            if asked {
                bail!("track {} has no notes there to copy", track.id);
            }
            continue;
        }
        sources.push((track.id.clone(), notes));
    }
    if sources.is_empty() {
        bail!("no notes start there, so there is nothing to copy");
    }

    let mut next_clip = project
        .next_id("c")
        .trim_start_matches('c')
        .parse::<u64>()
        .unwrap_or(1);
    let mut copied = Vec::new();
    for (track_id, notes) in sources {
        let track = project.track_mut(&track_id).expect("checked above");
        for k in 0..times {
            let d0 = to + k as f64 * length;
            let d1 = d0 + length;
            // Replace whatever started in the destination.
            for clip in &mut track.clips {
                let start = clip.start;
                clip.notes.retain(|n| {
                    let at = start + n.start;
                    !(at >= d0 - EPS && at < d1 - EPS)
                });
            }
            let free = gaps(&track.clips, d0, d1);
            let mut homeless: Vec<Vec<Note>> = vec![Vec::new(); free.len()];
            let mut name = String::new();
            for (offset, note, clip_name) in &notes {
                let at = d0 + offset;
                let home = track
                    .clips
                    .iter_mut()
                    .find(|c| c.start <= at + EPS && at < c.end() - EPS);
                match home {
                    Some(c) => c.notes.push(Note {
                        start: (at - c.start).max(0.0),
                        ..note.clone()
                    }),
                    None => {
                        if let Some(g) = free
                            .iter()
                            .position(|&(g0, g1)| at >= g0 - EPS && at < g1 - EPS)
                        {
                            homeless[g].push(Note {
                                start: (at - free[g].0).max(0.0),
                                ..note.clone()
                            });
                            if name.is_empty() {
                                name.clone_from(clip_name);
                            }
                        }
                    }
                }
            }
            for ((g0, g1), notes) in free.into_iter().zip(homeless) {
                if notes.is_empty() {
                    continue;
                }
                track.clips.push(Clip {
                    id: format!("c{next_clip}"),
                    name: name.clone(),
                    start: g0,
                    length: g1 - g0,
                    notes,
                });
                next_clip += 1;
            }
            for clip in &mut track.clips {
                sort_notes(clip);
            }
            track.clips.sort_by(|a, b| a.start.total_cmp(&b.start));
        }
        copied.push(CopiedTrack {
            track_id,
            notes: notes.len(),
        });
    }
    Ok(copied)
}

/// Snap note starts to a grid of `grid` beats, counted from the start of
/// the song (`origin` is where the notes' clip starts). `strength` (0..1)
/// moves each note that part of the way. `swing` (0.5 = straight .. 0.75)
/// delays every second grid step, giving straight eighths a shuffle.
/// Lengths are kept. Returns how many notes moved.
pub fn quantize<'a>(
    notes: impl IntoIterator<Item = &'a mut Note>,
    origin: f64,
    grid: f64,
    strength: f64,
    swing: f64,
) -> usize {
    if !(grid.is_finite() && grid > 0.0) {
        return 0;
    }
    let strength = if strength.is_finite() {
        strength.clamp(0.0, 1.0)
    } else {
        1.0
    };
    let late = if swing.is_finite() {
        (swing.clamp(0.5, 0.75) - 0.5) * 2.0 * grid
    } else {
        0.0
    };
    // Where grid step `s` lands, counted from the clip's start.
    let spot = |s: f64| s * grid + if s.rem_euclid(2.0) == 1.0 { late } else { 0.0 } - origin;
    let mut moved = 0;
    for n in notes {
        // The nearest swung spot (the later one on a tie): with swing up to
        // 0.75 that is the step before, at or after the note's position.
        let base = ((origin + n.start) / grid).floor();
        let target = [base + 1.0, base, base - 1.0]
            .map(spot)
            .into_iter()
            .min_by(|a, b| (a - n.start).abs().total_cmp(&(b - n.start).abs()))
            .unwrap_or(n.start);
        let new = (n.start + (target - n.start) * strength).max(0.0);
        if (new - n.start).abs() > EPS {
            n.start = new;
            moved += 1;
        }
    }
    moved
}

/// SplitMix64: a small, fast generator; good enough for musical jitter and
/// the same on every machine, so a seed always gives the same result.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// From -1 to 1, most often near 0 (the sum of two uniform numbers), as
    /// a player's small timing and touch differences are.
    fn wobble(&mut self) -> f64 {
        let mut unit = || (self.next() >> 11) as f64 / (1u64 << 53) as f64;
        unit() + unit() - 1.0
    }
}

/// A seed of its own for each clip as it is now (FNV-1a over its id and
/// notes): two clips humanized together don't get the very same "mistakes",
/// and humanizing a clip again varies it anew rather than repeating, and so
/// doubling, the first changes.
pub fn clip_seed(seed: u64, clip: &Clip) -> u64 {
    let fnv = |h: u64, b: u8| (h ^ b as u64).wrapping_mul(0x0100_0000_01B3);
    let mut h = clip.id.bytes().fold(seed ^ 0xCBF2_9CE4_8422_2325, fnv);
    for n in &clip.notes {
        h = n
            .start
            .to_bits()
            .to_le_bytes()
            .into_iter()
            .chain([n.pitch, n.velocity])
            .fold(h, fnv);
    }
    h
}

/// Move note starts by up to `timing` beats either way and change their
/// velocity by up to `velocity`, at random but repeatably (the same `seed`
/// gives the same result), so programmed parts sound played. No note moves
/// before the start of its clip. Returns how many notes changed.
pub fn humanize<'a>(
    notes: impl IntoIterator<Item = &'a mut Note>,
    timing: f64,
    velocity: u8,
    seed: u64,
) -> usize {
    let timing = if timing.is_finite() {
        timing.max(0.0)
    } else {
        0.0
    };
    let mut rng = Rng(seed);
    let mut changed = 0;
    for n in notes {
        let shift = rng.wobble() * timing;
        let touch = (rng.wobble() * velocity as f64).round() as i32;
        let start = (n.start + shift).max(0.0);
        let vel = (n.velocity as i32 + touch).clamp(1, 127) as u8;
        if (start - n.start).abs() > EPS || vel != n.velocity {
            changed += 1;
        }
        n.start = start;
        n.velocity = vel;
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edit::{apply_all, Op};
    use serde_json::json;

    fn note(pitch: u8, start: f64, duration: f64) -> Note {
        Note {
            pitch,
            start,
            duration,
            velocity: 100,
        }
    }

    /// Bass with a 4-bar clip (a note on each bar), drums with one bar of
    /// kick and snare, and an empty lead track.
    fn song() -> Project {
        let mut p = Project::new("t", 120.0);
        let ops: Vec<Op> = serde_json::from_value(json!([
            {"op": "add_track", "name": "Bass"},
            {"op": "add_track", "name": "Drums", "instrument": {"type": "drums"}},
            {"op": "add_track", "name": "Lead"},
            {"op": "add_clip", "track_id": "t1", "start": 0, "length": 16, "name": "Line",
             "notes": [{"pitch": 40, "start": 0, "duration": 4}, {"pitch": 43, "start": 4, "duration": 4},
                       {"pitch": 45, "start": 8, "duration": 4}, {"pitch": 47, "start": 12, "duration": 4}]},
            {"op": "add_clip", "track_id": "t2", "start": 0, "length": 4, "name": "Beat",
             "notes": [{"pitch": 36, "start": 0, "duration": 0.25}, {"pitch": 38, "start": 1, "duration": 0.25},
                       {"pitch": 36, "start": 2, "duration": 0.25}, {"pitch": 38, "start": 3, "duration": 0.25}]}
        ]))
        .unwrap();
        apply_all(&mut p, ops).unwrap();
        p
    }

    fn starts(track: &Track) -> Vec<(f64, u8)> {
        let mut all: Vec<(f64, u8)> = track
            .clips
            .iter()
            .flat_map(|c| c.notes.iter().map(move |n| (c.start + n.start, n.pitch)))
            .collect();
        all.sort_by(|a, b| a.0.total_cmp(&b.0));
        all
    }

    #[test]
    fn copying_a_bar_of_drums_three_times_makes_new_clips() {
        let mut p = song();
        let done = copy_bars(&mut p, &["t2".into()], 0.0, 4.0, 4.0, 3).unwrap();
        assert_eq!(
            done,
            [CopiedTrack {
                track_id: "t2".into(),
                notes: 4
            }]
        );
        let drums = p.track("t2").unwrap();
        assert_eq!(drums.clips.len(), 4, "the original and three copies");
        assert_eq!(drums.clips[3].start, 12.0);
        assert_eq!(drums.clips[3].length, 4.0);
        assert_eq!(drums.clips[3].name, "Beat");
        assert_eq!(starts(drums).len(), 16);
        assert_eq!(starts(drums)[13], (13.0, 38));
        // Fresh ids that don't clash with anything.
        let mut ids: Vec<&str> = p
            .tracks
            .iter()
            .flat_map(|t| &t.clips)
            .map(|c| c.id.as_str())
            .collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), 5, "bass, drums and three copies");
    }

    #[test]
    fn copying_into_a_clip_replaces_what_was_there() {
        let mut p = song();
        // Bar 1 of the bass line over bar 3, inside the same clip.
        copy_bars(&mut p, &[], 0.0, 4.0, 8.0, 1).unwrap();
        let bass = p.track("t1").unwrap();
        assert_eq!(bass.clips.len(), 1, "it went into the clip covering bar 3");
        assert_eq!(starts(bass), [(0.0, 40), (4.0, 43), (8.0, 40), (12.0, 47)]);
        // Every track with notes there took part: the drums got a new clip;
        // the lead, with nothing to copy, was left alone.
        assert_eq!(starts(p.track("t2").unwrap()).len(), 8);
        assert!(p.track("t3").unwrap().clips.is_empty());
    }

    #[test]
    fn a_copy_may_overlap_its_source() {
        let mut p = song();
        // Bars 1-2 of the bass over bars 2-3: the copy is of the original.
        copy_bars(&mut p, &["t1".into()], 0.0, 8.0, 4.0, 1).unwrap();
        assert_eq!(
            starts(p.track("t1").unwrap()),
            [(0.0, 40), (4.0, 40), (8.0, 43), (12.0, 47)]
        );
    }

    #[test]
    fn copies_fill_clips_and_the_gaps_between_them() {
        let mut p = song();
        // A drum fill in bars 3-4 with a crash on beats 9 and 13.
        p.tracks[1].clips.push(Clip {
            id: "c9".into(),
            name: "Fill".into(),
            start: 8.0,
            length: 8.0,
            notes: vec![note(49, 0.0, 1.0), note(49, 4.0, 1.0)],
        });
        // Bar 1 copied to the middle of bar 2: half lands in the empty
        // stretch before the fill, half in the fill, replacing its crash.
        copy_bars(&mut p, &["t2".into()], 0.0, 4.0, 6.0, 1).unwrap();
        let drums = p.track("t2").unwrap();
        let spans: Vec<(f64, f64)> = drums.clips.iter().map(|c| (c.start, c.end())).collect();
        assert_eq!(
            spans,
            [(0.0, 4.0), (6.0, 8.0), (8.0, 16.0)],
            "no clips overlap"
        );
        let beats: Vec<(f64, u8)> = [0.0, 1.0, 2.0, 3.0, 6.0, 7.0, 8.0, 9.0]
            .iter()
            .zip([36, 38, 36, 38, 36, 38, 36, 38])
            .map(|(&b, p)| (b, p))
            .chain([(12.0, 49)])
            .collect();
        assert_eq!(starts(drums), beats);
    }

    #[test]
    fn copy_bars_explains_what_is_wrong() {
        let mut p = song();
        let err = |r: Result<Vec<CopiedTrack>>| r.unwrap_err().to_string();
        assert!(err(copy_bars(&mut p, &["t3".into()], 0.0, 4.0, 4.0, 1)).contains("no notes"));
        assert!(err(copy_bars(&mut p, &[], 64.0, 4.0, 4.0, 1)).contains("nothing to copy"));
        assert!(err(copy_bars(&mut p, &["t9".into()], 0.0, 4.0, 4.0, 1)).contains("no track"));
        assert!(copy_bars(&mut p, &[], 0.0, 0.0, 4.0, 1).is_err());
        assert!(copy_bars(&mut p, &[], 0.0, 4.0, 4.0, 0).is_err());
        assert_eq!(p, song(), "failed calls change nothing");
    }

    #[test]
    fn quantize_snaps_with_strength_and_swing() {
        let mut notes = vec![
            note(60, 0.1, 1.0),
            note(62, 0.45, 1.0),
            note(64, 1.0, 1.0),
            note(65, 1.25, 1.0),
        ];
        assert_eq!(quantize(&mut notes, 0.0, 0.5, 1.0, 0.5), 3);
        let at: Vec<f64> = notes.iter().map(|n| n.start).collect();
        assert_eq!(at, [0.0, 0.5, 1.0, 1.5], "half way goes to the later step");
        assert_eq!(notes[0].duration, 1.0, "lengths are kept");

        // Half strength goes half way.
        let mut notes = vec![note(60, 0.2, 1.0)];
        quantize(&mut notes, 0.0, 0.5, 0.5, 0.5);
        assert!((notes[0].start - 0.1).abs() < 1e-9);

        // Swing 2/3 on eighths: the off-beat eighth lands on the last
        // triplet. The grid counts from the song's start, not the clip's.
        let mut notes = vec![note(60, 0.0, 0.5), note(60, 0.5, 0.5), note(60, 1.0, 0.5)];
        quantize(&mut notes, 0.0, 0.5, 1.0, 2.0 / 3.0);
        let at: Vec<f64> = notes.iter().map(|n| n.start).collect();
        assert!(
            (at[1] - 2.0 / 3.0).abs() < 1e-9 && at[0] == 0.0 && at[2] == 1.0,
            "{at:?}"
        );
        let mut late_clip = vec![note(60, 0.0, 0.5)];
        quantize(&mut late_clip, 0.5, 0.5, 1.0, 2.0 / 3.0);
        assert!((late_clip[0].start - 1.0 / 6.0).abs() < 1e-9);
        assert_eq!(quantize(&mut late_clip, 0.0, 0.0, 1.0, 0.5), 0, "no grid");

        // Heavy swing puts the off-beat half way to the next beat. Notes
        // there, or played a little late, stay on their own step.
        let mut heavy = vec![
            note(60, 0.75, 0.25),
            note(60, 0.8, 0.25),
            note(60, 1.3, 0.25),
        ];
        quantize(&mut heavy, 0.0, 0.5, 1.0, 0.75);
        let at: Vec<f64> = heavy.iter().map(|n| n.start).collect();
        assert_eq!(at, [0.75, 0.75, 1.0]);
        assert_eq!(
            quantize(&mut heavy, 0.0, 0.5, 1.0, 0.75),
            0,
            "already there"
        );
    }

    #[test]
    fn humanize_is_small_varied_and_repeatable() {
        let line = || {
            (0..64)
                .map(|i| note(60, i as f64 * 0.5, 0.5))
                .collect::<Vec<_>>()
        };
        let mut a = line();
        let mut b = line();
        assert_eq!(humanize(&mut a, 0.02, 8, 7), 64);
        humanize(&mut b, 0.02, 8, 7);
        assert_eq!(a, b, "same seed, same result");
        let mut c = line();
        humanize(&mut c, 0.02, 8, 8);
        assert_ne!(a, c, "another seed, another result");

        let orig = line();
        assert!(a
            .iter()
            .zip(&orig)
            .all(|(n, o)| (n.start - o.start).abs() <= 0.02 + 1e-12));
        assert!(a.iter().all(|n| (92..=108).contains(&n.velocity)));
        assert!(
            a.iter()
                .map(|n| n.velocity)
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                > 5
        );
        assert!(a[0].start >= 0.0, "never before the clip");

        let clip = |id: &str| Clip {
            id: id.into(),
            name: String::new(),
            start: 0.0,
            length: 32.0,
            notes: line(),
        };
        assert_eq!(clip_seed(1, &clip("c1")), clip_seed(1, &clip("c1")));
        assert_ne!(clip_seed(1, &clip("c1")), clip_seed(1, &clip("c2")));
        // Humanized again, a clip gets new changes, not the same ones twice.
        let mut c1 = clip("c1");
        let first = clip_seed(1, &c1);
        humanize(&mut c1.notes, 0.02, 8, first);
        assert_ne!(clip_seed(1, &c1), first);

        let mut quiet = vec![Note {
            velocity: 2,
            ..note(60, 0.0, 1.0)
        }];
        humanize(&mut quiet, 0.0, 40, 3);
        assert!(quiet[0].velocity >= 1);
    }
}
