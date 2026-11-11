//! Editing operations. Every change the AI makes goes through [`Op`], which is
//! also the JSON shape of the MCP tool arguments, so the vocabulary the AI
//! uses and the code that validates it live in one place.

use std::collections::BTreeMap;

use anyhow::{anyhow, bail, Result};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::automation::{self, Lane, Param, Point};
use crate::model::{
    Clip, Effect, EffectKind, Instrument, Note, PluginRef, Project, TimeSignature, Track,
};
use crate::transport::{Cue, LoopRegion, MIN_LOOP_BEATS};
use crate::{color, transform};

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    UpdateProject {
        name: Option<String>,
        bpm: Option<f64>,
        /// Bar length in beats (quarter notes).
        beats_per_bar: Option<u32>,
        /// Meter such as "6/8"; sets `beats_per_bar` to match (6/8 -> 3).
        time_signature: Option<TimeSignature>,
        master_gain: Option<f32>,
        /// Shared reverb room size 0..1 (small room .. big hall).
        reverb_size: Option<f32>,
        /// Shared reverb tone 0..1 (dark .. bright).
        reverb_tone: Option<f32>,
    },
    AddTrack {
        name: String,
        instrument: Option<Instrument>,
        gain: Option<f32>,
        pan: Option<f32>,
        /// Reverb send 0..1 (default 0, dry).
        reverb: Option<f32>,
    },
    UpdateTrack {
        track_id: String,
        name: Option<String>,
        instrument: Option<Instrument>,
        gain: Option<f32>,
        pan: Option<f32>,
        mute: Option<bool>,
        solo: Option<bool>,
        reverb: Option<f32>,
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
    /// Copy the notes that start in [from, from + length) beats to `to`,
    /// `times` times in a row, on `track_ids` (empty: every track with notes
    /// there). What those tracks had in the destination is replaced.
    CopyBars {
        from: f64,
        length: f64,
        to: f64,
        #[serde(default = "one")]
        times: u32,
        #[serde(default)]
        track_ids: Vec<String>,
    },
    /// Snap note starts in these clips to a grid of `grid` beats; see
    /// [`transform::quantize`] for `strength` and `swing`.
    Quantize {
        clip_ids: Vec<String>,
        #[serde(default = "sixteenth")]
        grid: f64,
        #[serde(default = "full")]
        strength: f64,
        #[serde(default = "straight")]
        swing: f64,
    },
    /// Small random changes to timing (up to `timing` beats) and velocity
    /// (up to `velocity`) in these clips; see [`transform::humanize`].
    Humanize {
        clip_ids: Vec<String>,
        #[serde(default = "light_timing")]
        timing: f64,
        #[serde(default = "light_touch")]
        velocity: u8,
        #[serde(default)]
        seed: u64,
    },
    /// Put an effect plugin into a track's chain at `index` (default: last).
    AddEffect {
        track_id: String,
        plugin: PluginRef,
        index: Option<usize>,
        #[serde(default)]
        bypass: bool,
    },
    RemoveEffect {
        effect_id: String,
    },
    /// Switch an effect off (`bypass`) or on again, or move it to `index`
    /// in its track's chain.
    UpdateEffect {
        effect_id: String,
        bypass: Option<bool>,
        index: Option<usize>,
    },
    /// Set parameters of a track's plugin instrument, or of one of the
    /// track's effects (`effect_id`): parameter id -> value 0..1. They apply
    /// on top of the plugin's saved settings.
    SetPluginParams {
        track_id: String,
        effect_id: Option<String>,
        params: BTreeMap<String, f64>,
    },
    /// Move a track to `index` (0 = the top row); the others close up.
    MoveTrack {
        track_id: String,
        index: usize,
    },
    /// Give a track a colour: a name ("orange") or hex code ("#ff8a3d").
    /// None, "" or "auto" gives it back the colour that follows from its id.
    SetTrackColor {
        track_id: String,
        color: Option<String>,
    },
    /// Cut a clip in two at `at` beats (song time); see [`split_clip`].
    SplitClip {
        clip_id: String,
        at: f64,
    },
    /// Set the loop region in beats (both ends, or one to move just that
    /// end) and switch looping on, or off with `on: false`.
    SetLoop {
        start: Option<f64>,
        end: Option<f64>,
        on: Option<bool>,
    },
    /// Ask the app to put its playhead at `beat`, and to start (`play:
    /// true`) or stop (`false`) playing there.
    SetPlayhead {
        beat: f64,
        play: Option<bool>,
    },
    /// Put points on the automation lane of `param`, on a track or, without
    /// `track_id`, on the song (master volume, reverb room). They replace
    /// what the lane had from the first to the last of them; values are
    /// clamped to the setting's range.
    SetAutomation {
        track_id: Option<String>,
        param: Param,
        points: Vec<Point>,
    },
    /// Remove automation from a track (or the song, without `track_id`): the
    /// lane of `param`, or every lane; with `from`/`to` only the points in
    /// [from, to) beats. Lanes left without points go too.
    ClearAutomation {
        track_id: Option<String>,
        param: Option<Param>,
        from: Option<f64>,
        to: Option<f64>,
    },
}

fn one() -> u32 {
    1
}
fn sixteenth() -> f64 {
    0.25
}
fn full() -> f64 {
    1.0
}
fn straight() -> f64 {
    0.5
}
fn light_timing() -> f64 {
    0.02
}
fn light_touch() -> u8 {
    8
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

/// A 0..1 amount such as a reverb send, clamped; NaN is refused.
fn unit(v: f32, what: &str) -> Result<f32> {
    if v.is_nan() {
        bail!("{what} must be a number from 0 to 1");
    }
    Ok(v.clamp(0.0, 1.0))
}

/// The clips with these ids, or an error naming the first one missing.
fn clips_mut<'a>(project: &'a mut Project, ids: &[String]) -> Result<Vec<&'a mut Clip>> {
    if ids.is_empty() {
        bail!("no clips given");
    }
    if let Some(id) = ids.iter().find(|id| project.clip(id).is_none()) {
        bail!("no clip {id}");
    }
    Ok(project
        .tracks
        .iter_mut()
        .flat_map(|t| t.clips.iter_mut())
        .filter(|c| ids.contains(&c.id))
        .collect())
}

fn sort_notes(clip: &mut Clip) {
    clip.sort_notes();
}

/// The automation lanes `param` belongs on: a track's, or the song's when
/// `track_id` is None. Refuses settings that would do nothing there.
fn lanes_mut<'a>(
    project: &'a mut Project,
    track_id: Option<&str>,
    param: Param,
) -> Result<&'a mut Vec<Lane>> {
    if param == Param::Unknown {
        bail!("unknown setting to automate");
    }
    let Some(id) = track_id else {
        if !param.is_master() {
            bail!(
                "{} is a track setting; say which track (the song itself has master_volume, reverb_size and reverb_tone)",
                param.id()
            );
        }
        return Ok(&mut project.automation);
    };
    let t = project
        .track_mut(id)
        .ok_or_else(|| anyhow!("no track {id}"))?;
    if param.is_master() {
        bail!(
            "{} is a setting of the whole song, not of a track; leave the track out",
            param.id()
        );
    }
    if !param.suits_track(&t.instrument) {
        if let Some(plugin) = t.instrument.plugin() {
            bail!(
                "track {id} plays the plugin {}, which has no {} (only the built-in synth does); plugin tracks can automate volume, pan and reverb",
                plugin.name,
                param.id()
            );
        }
        bail!(
            "track {id} plays drums, which have no {} (only synths do); drums can automate volume, pan and reverb",
            param.id()
        );
    }
    Ok(&mut t.automation)
}

/// Apply one operation. Returns a small JSON result (e.g. the id of a created object).
pub fn apply(project: &mut Project, op: Op) -> Result<Value> {
    match op {
        Op::UpdateProject {
            name,
            bpm,
            beats_per_bar,
            time_signature,
            master_gain,
            reverb_size,
            reverb_tone,
        } => {
            let reverb_size = reverb_size.map(|v| unit(v, "reverb_size")).transpose()?;
            let reverb_tone = reverb_tone.map(|v| unit(v, "reverb_tone")).transpose()?;
            if let Some(v) = name {
                project.name = v;
            }
            if let Some(v) = bpm {
                if !(20.0..=400.0).contains(&v) {
                    bail!("bpm must be within 20..=400");
                }
                project.bpm = v;
            }
            if let Some(ts) = time_signature {
                let beats = ts.beats_per_bar();
                if beats.is_none() || beats_per_bar.is_some_and(|b| Some(b) != beats) {
                    bail!(
                        "time_signature {ts} makes bars {} beats long; give either time_signature or a matching beats_per_bar",
                        f64::from(ts.num) * 4.0 / f64::from(ts.den)
                    );
                }
                project.set_time_signature(ts);
            } else if let Some(v) = beats_per_bar {
                project.beats_per_bar = v.clamp(1, 32);
                // A meter that no longer fits the bar length is dropped.
                if project
                    .time_signature
                    .and_then(TimeSignature::beats_per_bar)
                    != Some(project.beats_per_bar)
                {
                    project.time_signature = None;
                }
            }
            if let Some(v) = master_gain {
                project.master_gain = v.clamp(0.0, 2.0);
            }
            if let Some(v) = reverb_size {
                project.reverb.size = v;
            }
            if let Some(v) = reverb_tone {
                project.reverb.tone = v;
            }
            Ok(json!({"ok": true}))
        }
        Op::AddTrack {
            name,
            instrument,
            gain,
            pan,
            reverb,
        } => {
            let reverb = unit(reverb.unwrap_or(0.0), "reverb")?;
            if let Some(p) = instrument.as_ref().and_then(Instrument::plugin) {
                p.check()?;
            }
            let id = project.next_id("t");
            project.tracks.push(Track {
                id: id.clone(),
                name,
                instrument: instrument.unwrap_or_else(Instrument::default_synth),
                gain: gain.unwrap_or(0.8).clamp(0.0, 2.0),
                pan: pan.unwrap_or(0.0).clamp(-1.0, 1.0),
                mute: false,
                solo: false,
                reverb,
                effects: Vec::new(),
                color: None,
                clips: Vec::new(),
                automation: Vec::new(),
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
            reverb,
        } => {
            let reverb = reverb.map(|v| unit(v, "reverb")).transpose()?;
            if let Some(p) = instrument.as_ref().and_then(Instrument::plugin) {
                p.check()?;
            }
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
            if let Some(v) = reverb {
                t.reverb = v;
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
        Op::CopyBars {
            from,
            length,
            to,
            times,
            track_ids,
        } => {
            let copied = transform::copy_bars(project, &track_ids, from, length, to, times)?;
            let tracks: Vec<&str> = copied.iter().map(|c| c.track_id.as_str()).collect();
            let notes: usize = copied.iter().map(|c| c.notes).sum();
            Ok(json!({"tracks": tracks, "notes_per_copy": notes, "copies": times}))
        }
        Op::Quantize {
            clip_ids,
            grid,
            strength,
            swing,
        } => {
            if !(grid.is_finite() && grid > 0.0) {
                bail!("grid must be > 0 beats, e.g. 0.25 for sixteenth notes");
            }
            let mut moved = 0;
            for c in clips_mut(project, &clip_ids)? {
                moved += transform::quantize(&mut c.notes, c.start, grid, strength, swing);
                sort_notes(c);
            }
            Ok(json!({"moved": moved}))
        }
        Op::Humanize {
            clip_ids,
            timing,
            velocity,
            seed,
        } => {
            let mut changed = 0;
            for c in clips_mut(project, &clip_ids)? {
                let seed = transform::clip_seed(seed, c);
                changed += transform::humanize(&mut c.notes, timing, velocity, seed);
                sort_notes(c);
            }
            Ok(json!({"changed": changed}))
        }
        Op::AddEffect {
            track_id,
            plugin,
            index,
            bypass,
        } => {
            plugin.check()?;
            let id = project.next_id("fx");
            let t = project
                .track_mut(&track_id)
                .ok_or_else(|| anyhow!("no track {track_id}"))?;
            let at = index.unwrap_or(t.effects.len()).min(t.effects.len());
            t.effects.insert(
                at,
                Effect {
                    id: id.clone(),
                    bypass,
                    kind: EffectKind::Vst3(plugin),
                },
            );
            Ok(json!({"effect_id": id, "position": at + 1}))
        }
        Op::RemoveEffect { effect_id } => {
            for t in &mut project.tracks {
                if let Some(i) = t.effects.iter().position(|e| e.id == effect_id) {
                    t.effects.remove(i);
                    return Ok(json!({"ok": true}));
                }
            }
            bail!("no effect {effect_id}")
        }
        Op::UpdateEffect {
            effect_id,
            bypass,
            index,
        } => {
            let t = project
                .tracks
                .iter_mut()
                .find(|t| t.effects.iter().any(|e| e.id == effect_id))
                .ok_or_else(|| anyhow!("no effect {effect_id}"))?;
            let mut i = t.effects.iter().position(|e| e.id == effect_id).unwrap();
            if let Some(v) = bypass {
                t.effects[i].bypass = v;
            }
            if let Some(to) = index {
                let fx = t.effects.remove(i);
                i = to.min(t.effects.len());
                t.effects.insert(i, fx);
            }
            Ok(json!({"ok": true, "position": i + 1}))
        }
        Op::SetPluginParams {
            track_id,
            effect_id,
            params,
        } => {
            let t = project
                .track_mut(&track_id)
                .ok_or_else(|| anyhow!("no track {track_id}"))?;
            let plugin = match &effect_id {
                Some(fx) => t
                    .effects
                    .iter_mut()
                    .find(|e| &e.id == fx)
                    .ok_or_else(|| anyhow!("track {track_id} has no effect {fx}"))?
                    .plugin_mut(),
                None => t
                    .instrument
                    .plugin_mut()
                    .ok_or_else(|| anyhow!("track {track_id} does not play a plugin"))?,
            };
            let mut next = plugin.clone();
            next.params.extend(params);
            next.check()?;
            let n = next.params.len();
            *plugin = next;
            Ok(json!({"ok": true, "params_set": n}))
        }
        Op::MoveTrack { track_id, index } => {
            let from = project
                .tracks
                .iter()
                .position(|t| t.id == track_id)
                .ok_or_else(|| anyhow!("no track {track_id}"))?;
            let to = index.min(project.tracks.len() - 1);
            let track = project.tracks.remove(from);
            project.tracks.insert(to, track);
            Ok(json!({"index": to}))
        }
        Op::SetTrackColor { track_id, color } => {
            let picked = match color.as_deref().map(str::trim) {
                None | Some("" | "auto" | "none" | "default") => None,
                Some(text) => Some(color::hex(color::parse(text).ok_or_else(|| {
                    anyhow!(
                        "unknown colour {text:?}; use a hex code like \"#ff8a3d\" or one of: {}",
                        color::names()
                    )
                })?)),
            };
            let t = project
                .track_mut(&track_id)
                .ok_or_else(|| anyhow!("no track {track_id}"))?;
            t.color = picked;
            Ok(json!({"color": color::hex(t.rgb())}))
        }
        Op::SplitClip { clip_id, at } => {
            let new_id = split_clip(project, &clip_id, at)?;
            Ok(json!({"clip_id": clip_id, "new_clip_id": new_id}))
        }
        Op::SetLoop { start, end, on } => {
            let old = project.transport.loop_region;
            if start.is_none() && end.is_none() && old.is_none() {
                if on == Some(false) {
                    return Ok(json!({"on": false}));
                }
                bail!("there is no loop region yet; give its start and end in beats");
            }
            let start = start.or(old.map(|r| r.start)).unwrap_or(0.0);
            let end = match end.or(old.map(|r| r.end)) {
                Some(e) => e,
                None => bail!("the loop needs an end, in beats"),
            };
            let region = LoopRegion {
                start,
                end,
                on: on.unwrap_or(true),
            };
            if !(start.is_finite() && start >= 0.0 && end.is_finite()) {
                bail!("the loop's start and end must be beats >= 0");
            }
            if !region.is_valid() {
                bail!(
                    "the loop must end after it starts (start {start}, end {end}, at least {MIN_LOOP_BEATS} beats long)"
                );
            }
            project.transport.loop_region = Some(region);
            Ok(json!({"start": start, "end": end, "on": region.on}))
        }
        Op::SetPlayhead { beat, play } => {
            if !(beat.is_finite() && beat >= 0.0) {
                bail!("the playhead goes to a beat >= 0 (got {beat})");
            }
            let last = project.transport.cue.map_or(0, |c| c.id);
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_millis() as u64);
            let id = if now == last { now + 1 } else { now };
            project.transport.cue = Some(Cue { beat, play, id });
            Ok(json!({"beat": beat}))
        }
        Op::SetAutomation {
            track_id,
            param,
            points,
        } => {
            let info = param.info();
            if points.is_empty() {
                bail!("no points given");
            }
            let mut clean = Vec::with_capacity(points.len());
            for p in points {
                if !(p.beat.is_finite() && p.beat >= 0.0) {
                    bail!("automation points need a beat >= 0 (got {})", p.beat);
                }
                if !p.value.is_finite() {
                    bail!(
                        "{} values must be numbers from {} to {}",
                        info.id,
                        info.min,
                        info.max
                    );
                }
                clean.push(Point {
                    value: info.clamp(p.value),
                    ..p
                });
            }
            let lanes = lanes_mut(project, track_id.as_deref(), param)?;
            automation::sort(&mut clean);
            let (from, to) = (clean[0].beat, clean[clean.len() - 1].beat);
            let kept = lanes.iter().find(|l| l.param == param).map_or(0, |l| {
                l.points
                    .iter()
                    .filter(|p| p.beat < from || p.beat > to)
                    .count()
            });
            if kept + clean.len() > automation::MAX_POINTS {
                bail!(
                    "a lane can hold at most {} points; that would make {}",
                    automation::MAX_POINTS,
                    kept + clean.len()
                );
            }
            let n = clean.len();
            let lane = match lanes.iter().position(|l| l.param == param) {
                Some(i) => &mut lanes[i],
                None => {
                    lanes.push(Lane::new(param));
                    lanes.last_mut().expect("just pushed")
                }
            };
            lane.write(clean);
            Ok(json!({"points": n, "total": lane.points.len()}))
        }
        Op::ClearAutomation {
            track_id,
            param,
            from,
            to,
        } => {
            let lanes = match &track_id {
                None => &mut project.automation,
                Some(id) => {
                    &mut project
                        .track_mut(id)
                        .ok_or_else(|| anyhow!("no track {id}"))?
                        .automation
                }
            };
            let from = from.unwrap_or(f64::NEG_INFINITY);
            let to = to.unwrap_or(f64::INFINITY);
            let mut removed = 0;
            for lane in lanes
                .iter_mut()
                .filter(|l| param.is_none_or(|p| l.param == p))
            {
                removed += lane.clear_range(from, to);
            }
            let before = lanes.len();
            lanes.retain(|l| !(param.is_none_or(|p| l.param == p) && l.points.is_empty()));
            Ok(json!({"removed": removed, "lanes_removed": before - lanes.len()}))
        }
    }
}

/// Cut clip `clip_id` in two at `at` beats (song time, strictly inside the
/// clip). Notes that start before the cut stay in the clip, shortened so
/// they end at the cut (they were silent past it anyway); the others move
/// to a new clip right after it, which gets the same name. Returns the id
/// of the new clip.
pub fn split_clip(project: &mut Project, clip_id: &str, at: f64) -> Result<String> {
    const EPS: f64 = 1e-9;
    let new_id = project.next_id("c");
    let track = project
        .tracks
        .iter_mut()
        .find(|t| t.clips.iter().any(|c| c.id == clip_id))
        .ok_or_else(|| anyhow!("no clip {clip_id}"))?;
    let i = track
        .clips
        .iter()
        .position(|c| c.id == clip_id)
        .expect("found above");
    let clip = &mut track.clips[i];
    let cut = at - clip.start;
    if !(cut.is_finite() && cut > EPS && cut < clip.length - EPS) {
        bail!(
            "beat {at} is not inside clip {clip_id}, which covers beats {}..{}; split it somewhere in between",
            clip.start,
            clip.end()
        );
    }
    let (keep, moved): (Vec<Note>, Vec<Note>) =
        clip.notes.drain(..).partition(|n| n.start < cut - EPS);
    clip.notes = keep
        .into_iter()
        .map(|n| Note {
            duration: n.duration.min(cut - n.start),
            ..n
        })
        .collect();
    // A copy, so the new half keeps everything else the clip carries.
    let mut right = Clip {
        notes: Vec::new(),
        ..clip.clone()
    };
    right.id.clone_from(&new_id);
    right.start = at;
    right.length = clip.length - cut;
    right.notes = moved
        .into_iter()
        .map(|n| Note {
            start: (n.start - cut).max(0.0),
            ..n
        })
        .collect();
    clip.length = cut;
    track.clips.insert(i + 1, right);
    Ok(new_id)
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
