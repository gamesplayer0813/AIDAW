//! Editing operations. Every change the AI makes goes through [`Op`], which is
//! also the JSON shape of the MCP tool arguments, so the vocabulary the AI
//! uses and the code that validates it live in one place.

use anyhow::{anyhow, bail, Result};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::model::{Clip, Instrument, Note, Project, Track};

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    UpdateProject {
        name: Option<String>,
        bpm: Option<f64>,
        beats_per_bar: Option<u32>,
        master_gain: Option<f32>,
    },
    AddTrack {
        name: String,
        instrument: Option<Instrument>,
        gain: Option<f32>,
        pan: Option<f32>,
    },
    UpdateTrack {
        track_id: String,
        name: Option<String>,
        instrument: Option<Instrument>,
        gain: Option<f32>,
        pan: Option<f32>,
        mute: Option<bool>,
        solo: Option<bool>,
    },
    RemoveTrack {
        track_id: String,
    },
    AddClip {
        track_id: String,
        start: f64,
        length: f64,
        name: Option<String>,
        #[serde(default)]
        notes: Vec<Note>,
    },
    UpdateClip {
        clip_id: String,
        name: Option<String>,
        start: Option<f64>,
        length: Option<f64>,
    },
    RemoveClip {
        clip_id: String,
    },
    AddNotes {
        clip_id: String,
        notes: Vec<Note>,
    },
    /// Remove notes whose start lies in [from, to) and (optionally) match `pitch`.
    /// With no filters, clears the clip.
    RemoveNotes {
        clip_id: String,
        from: Option<f64>,
        to: Option<f64>,
        #[serde(default, deserialize_with = "opt_pitch")]
        pitch: Option<u8>,
    },
    TransposeClip {
        clip_id: String,
        semitones: i32,
    },
}

fn opt_pitch<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<u8>, D::Error> {
    #[derive(Deserialize)]
    struct W(#[serde(deserialize_with = "crate::pitch::deserialize")] u8);
    Ok(Option::<W>::deserialize(d)?.map(|w| w.0))
}

fn check_notes(notes: &[Note]) -> Result<()> {
    for n in notes {
        if !(n.start.is_finite() && n.start >= 0.0) {
            bail!("note start must be >= 0 (got {})", n.start);
        }
        if !(n.duration.is_finite() && n.duration > 0.0) {
            bail!("note duration must be > 0 (got {})", n.duration);
        }
        if !(1..=127).contains(&n.velocity) {
            bail!("velocity must be 1..=127 (got {})", n.velocity);
        }
    }
    Ok(())
}

fn sort_notes(clip: &mut Clip) {
    clip.notes
        .sort_by(|a, b| a.start.total_cmp(&b.start).then(a.pitch.cmp(&b.pitch)));
}

/// Apply one operation. Returns a small JSON result (e.g. the id of a created object).
pub fn apply(project: &mut Project, op: Op) -> Result<Value> {
    match op {
        Op::UpdateProject {
            name,
            bpm,
            beats_per_bar,
            master_gain,
        } => {
            if let Some(v) = name {
                project.name = v;
            }
            if let Some(v) = bpm {
                if !(20.0..=400.0).contains(&v) {
                    bail!("bpm must be within 20..=400");
                }
                project.bpm = v;
            }
            if let Some(v) = beats_per_bar {
                project.beats_per_bar = v.clamp(1, 32);
            }
            if let Some(v) = master_gain {
                project.master_gain = v.clamp(0.0, 2.0);
            }
            Ok(json!({"ok": true}))
        }
        Op::AddTrack {
            name,
            instrument,
            gain,
            pan,
        } => {
            let id = project.next_id("t");
            project.tracks.push(Track {
                id: id.clone(),
                name,
                instrument: instrument.unwrap_or_else(Instrument::default_synth),
                gain: gain.unwrap_or(0.8).clamp(0.0, 2.0),
                pan: pan.unwrap_or(0.0).clamp(-1.0, 1.0),
                mute: false,
                solo: false,
                clips: Vec::new(),
            });
            Ok(json!({"track_id": id}))
        }
        Op::UpdateTrack {
            track_id,
            name,
            instrument,
            gain,
            pan,
            mute,
            solo,
        } => {
            let t = project
                .track_mut(&track_id)
                .ok_or_else(|| anyhow!("no track {track_id}"))?;
            if let Some(v) = name {
                t.name = v;
            }
            if let Some(v) = instrument {
                t.instrument = v;
            }
            if let Some(v) = gain {
                t.gain = v.clamp(0.0, 2.0);
            }
            if let Some(v) = pan {
                t.pan = v.clamp(-1.0, 1.0);
            }
            if let Some(v) = mute {
                t.mute = v;
            }
            if let Some(v) = solo {
                t.solo = v;
            }
            Ok(json!({"ok": true}))
        }
        Op::RemoveTrack { track_id } => {
            let before = project.tracks.len();
            project.tracks.retain(|t| t.id != track_id);
            if project.tracks.len() == before {
                bail!("no track {track_id}");
            }
            Ok(json!({"ok": true}))
        }
        Op::AddClip {
            track_id,
            start,
            length,
            name,
            notes,
        } => {
            if start < 0.0 || length <= 0.0 {
                bail!("clip needs start >= 0 and length > 0");
            }
            check_notes(&notes)?;
            let id = project.next_id("c");
            let t = project
                .track_mut(&track_id)
                .ok_or_else(|| anyhow!("no track {track_id}"))?;
            let mut clip = Clip {
                id: id.clone(),
                name: name.unwrap_or_default(),
                start,
                length,
                notes,
            };
            sort_notes(&mut clip);
            t.clips.push(clip);
            t.clips.sort_by(|a, b| a.start.total_cmp(&b.start));
            Ok(json!({"clip_id": id}))
        }
        Op::UpdateClip {
            clip_id,
            name,
            start,
            length,
        } => {
            let c = project
                .clip_mut(&clip_id)
                .ok_or_else(|| anyhow!("no clip {clip_id}"))?;
            if let Some(v) = name {
                c.name = v;
            }
            if let Some(v) = start {
                c.start = v.max(0.0);
            }
            if let Some(v) = length {
                if v <= 0.0 {
                    bail!("length must be > 0");
                }
                c.length = v;
            }
            Ok(json!({"ok": true}))
        }
        Op::RemoveClip { clip_id } => {
            for t in &mut project.tracks {
                if let Some(i) = t.clips.iter().position(|c| c.id == clip_id) {
                    t.clips.remove(i);
                    return Ok(json!({"ok": true}));
                }
            }
            bail!("no clip {clip_id}")
        }
        Op::AddNotes { clip_id, notes } => {
            check_notes(&notes)?;
            let c = project
                .clip_mut(&clip_id)
                .ok_or_else(|| anyhow!("no clip {clip_id}"))?;
            let n = notes.len();
            c.notes.extend(notes);
            sort_notes(c);
            Ok(json!({"added": n, "total": c.notes.len()}))
        }
        Op::RemoveNotes {
            clip_id,
            from,
            to,
            pitch,
        } => {
            let c = project
                .clip_mut(&clip_id)
                .ok_or_else(|| anyhow!("no clip {clip_id}"))?;
            let from = from.unwrap_or(f64::NEG_INFINITY);
            let to = to.unwrap_or(f64::INFINITY);
            let before = c.notes.len();
            c.notes.retain(|n| {
                !(n.start >= from && n.start < to && pitch.is_none_or(|p| p == n.pitch))
            });
            Ok(json!({"removed": before - c.notes.len()}))
        }
        Op::TransposeClip { clip_id, semitones } => {
            let c = project
                .clip_mut(&clip_id)
                .ok_or_else(|| anyhow!("no clip {clip_id}"))?;
            for n in &mut c.notes {
                n.pitch = (n.pitch as i32 + semitones).clamp(0, 127) as u8;
            }
            Ok(json!({"ok": true}))
        }
    }
}

/// Apply a batch atomically: if any op fails, the project is left untouched.
pub fn apply_all(project: &mut Project, ops: Vec<Op>) -> Result<Vec<Value>> {
    let mut work = project.clone();
    let mut results = Vec::with_capacity(ops.len());
    for (i, op) in ops.into_iter().enumerate() {
        results.push(apply(&mut work, op).map_err(|e| anyhow!("op #{i}: {e}"))?);
    }
    *project = work;
    Ok(results)
}
