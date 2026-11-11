//! Core of AIDAW: the project document, editing operations, built-in
//! instruments and the renderer shared by real-time playback and export.

pub mod analysis;
pub mod automation;
pub mod color;
pub mod edit;
pub mod io;
pub mod model;
pub mod music;
pub mod pitch;
pub mod plugin;
pub mod presets;
pub mod rack;
pub mod render;
pub mod transform;
pub mod transport;

pub use automation::{Lane, Param, Point, Shape};
pub use model::{
    Blob, Clip, DrumKit, Effect, EffectKind, Instrument, Note, PluginRef, Project, Reverb, Synth,
    TimeSignature, Track, Waveform,
};
pub use transport::{Cue, LoopRegion, Transport};

#[cfg(test)]
mod tests {
    use super::*;
    use edit::{apply_all, Op};
    use serde_json::json;

    fn ops(v: serde_json::Value) -> Vec<Op> {
        serde_json::from_value(v).unwrap()
    }

    fn demo() -> Project {
        let mut p = Project::new("demo", 120.0);
        apply_all(
            &mut p,
            ops(json!([
                {"op": "add_track", "name": "Lead"},
                {"op": "add_track", "name": "Drums", "instrument": {"type": "drums"}},
                {"op": "add_clip", "track_id": "t1", "start": 0, "length": 4,
                 "notes": [{"pitch": "C4", "start": 0, "duration": 1}, {"pitch": 64, "start": 1, "duration": 1, "velocity": 90}]},
                {"op": "add_clip", "track_id": "t2", "start": 0, "length": 4,
                 "notes": [{"pitch": 36, "start": 0, "duration": 0.25}, {"pitch": 38, "start": 1, "duration": 0.25}]}
            ])),
        )
        .unwrap();
        p
    }

    #[test]
    fn ops_build_project() {
        let p = demo();
        assert_eq!(p.tracks.len(), 2);
        assert_eq!(p.tracks[0].clips[0].id, "c1");
        assert_eq!(p.tracks[0].clips[0].notes[0].pitch, 60);
        assert_eq!(p.length_beats(), 4.0);
    }

    #[test]
    fn batch_is_atomic() {
        let mut p = demo();
        let before = p.clone();
        let err = apply_all(
            &mut p,
            ops(
                json!([{"op": "add_track", "name": "x"}, {"op": "remove_clip", "clip_id": "nope"}]),
            ),
        );
        assert!(err.is_err());
        assert_eq!(p, before);
    }

    #[test]
    fn remove_and_transpose() {
        let mut p = demo();
        apply_all(
            &mut p,
            ops(
                json!([{"op": "transpose_clip", "clip_id": "c1", "semitones": 12},
                                     {"op": "remove_notes", "clip_id": "c1", "pitch": "E5"}]),
            ),
        )
        .unwrap();
        let notes = &p.tracks[0].clips[0].notes;
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].pitch, 72);
    }

    #[test]
    fn reverb_settings_are_clamped_and_checked_before_anything_changes() {
        let mut p = demo();
        apply_all(
            &mut p,
            ops(json!([
                {"op": "update_track", "track_id": "t1", "reverb": 1.5},
                {"op": "add_track", "name": "Pad", "reverb": 0.3},
                {"op": "update_project", "reverb_size": -1, "reverb_tone": 0.25}
            ])),
        )
        .unwrap();
        assert_eq!(p.tracks[0].reverb, 1.0);
        assert_eq!(p.tracks[1].reverb, 0.0, "tracks start dry unless told");
        assert_eq!(p.tracks[2].reverb, 0.3);
        assert_eq!((p.reverb.size, p.reverb.tone), (0.0, 0.25));

        let before = p.clone();
        let bad = edit::apply(
            &mut p,
            Op::UpdateTrack {
                track_id: "t1".into(),
                name: Some("renamed".into()),
                instrument: None,
                gain: None,
                pan: None,
                mute: None,
                solo: None,
                reverb: Some(f32::NAN),
            },
        );
        assert!(bad.is_err());
        assert_eq!(p, before, "a rejected edit changes nothing");
    }

    #[test]
    fn split_clip_cuts_notes_at_the_split() {
        let mut p = demo();
        let r = apply_all(
            &mut p,
            ops(json!([{"op": "split_clip", "clip_id": "c1", "at": 1.5}])),
        )
        .unwrap();
        assert_eq!(r[0], json!({"clip_id": "c1", "new_clip_id": "c3"}));
        let clips = &p.tracks[0].clips;
        assert_eq!(clips.len(), 2);
        let (left, right) = (&clips[0], &clips[1]);
        assert_eq!((left.start, left.length), (0.0, 1.5));
        assert_eq!((right.start, right.length), (1.5, 2.5));
        assert_eq!(right.name, left.name);
        // C4 (beat 0-1) stays; E4 (beat 1-2) starts before the cut, so it
        // stays too, shortened to end there.
        let kept: Vec<(u8, f64, f64)> = left
            .notes
            .iter()
            .map(|n| (n.pitch, n.start, n.duration))
            .collect();
        assert_eq!(kept, [(60, 0.0, 1.0), (64, 1.0, 0.5)]);
        assert!(right.notes.is_empty());

        // A note after the cut moves over, counted from the new clip's start.
        let mut p = demo();
        apply_all(
            &mut p,
            ops(json!([{"op": "split_clip", "clip_id": "c1", "at": 0.5}])),
        )
        .unwrap();
        let right = &p.tracks[0].clips[1];
        assert_eq!(right.notes.len(), 1);
        assert_eq!((right.notes[0].pitch, right.notes[0].start), (64, 0.5));
        assert_eq!(right.notes[0].velocity, 90);

        let before = p.clone();
        for at in [0.0, 4.0, 9.0, -1.0, f64::NAN] {
            let bad = edit::apply(
                &mut p,
                Op::SplitClip {
                    clip_id: "c2".into(),
                    at,
                },
            );
            assert!(bad.is_err(), "split at {at}");
        }
        assert!(edit::apply(
            &mut p,
            Op::SplitClip {
                clip_id: "nope".into(),
                at: 1.0
            }
        )
        .is_err());
        assert_eq!(p, before);
    }

    #[test]
    fn tracks_move_and_keep_their_colours() {
        let mut p = demo();
        apply_all(&mut p, ops(json!([{"op": "add_track", "name": "Pad"}]))).unwrap();
        let colours: Vec<[u8; 3]> = p.tracks.iter().map(Track::rgb).collect();
        let r = apply_all(
            &mut p,
            ops(json!([{"op": "move_track", "track_id": "t3", "index": 0}])),
        )
        .unwrap();
        assert_eq!(r[0]["index"], 0);
        let order: Vec<&str> = p.tracks.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(order, ["t3", "t1", "t2"]);
        assert_eq!(p.tracks[0].rgb(), colours[2], "the colour moved with it");
        // Past the end means the bottom row.
        apply_all(
            &mut p,
            ops(json!([{"op": "move_track", "track_id": "t3", "index": 99},
                       {"op": "remove_track", "track_id": "t1"}])),
        )
        .unwrap();
        assert_eq!(p.tracks[1].id, "t3");
        assert_eq!(
            p.tracks[1].rgb(),
            colours[2],
            "deleting a track changes nothing"
        );
        assert_eq!(p.tracks[0].rgb(), colours[1]);

        let r = apply_all(
            &mut p,
            ops(json!([{"op": "set_track_color", "track_id": "t2", "color": "Orange"}])),
        )
        .unwrap();
        assert_eq!(r[0]["color"], "#ff8a3d");
        assert_eq!(p.tracks[0].color.as_deref(), Some("#ff8a3d"));
        assert!(serde_json::to_string(&p)
            .unwrap()
            .contains(r##""color":"#ff8a3d""##));
        let before = p.clone();
        let bad = apply_all(
            &mut p,
            ops(json!([{"op": "set_track_color", "track_id": "t2", "color": "plaid"}])),
        );
        assert!(bad.unwrap_err().to_string().contains("orange"));
        assert_eq!(p, before);
        apply_all(
            &mut p,
            ops(json!([{"op": "set_track_color", "track_id": "t2", "color": "auto"}])),
        )
        .unwrap();
        assert_eq!(p.tracks[0].color, None);
    }

    #[test]
    fn loop_region_is_set_switched_and_checked() {
        let mut p = demo();
        // Switching off a loop that isn't there is fine; switching it on isn't.
        apply_all(&mut p, ops(json!([{"op": "set_loop", "on": false}]))).unwrap();
        assert!(apply_all(&mut p, ops(json!([{"op": "set_loop"}]))).is_err());
        apply_all(
            &mut p,
            ops(json!([{"op": "set_loop", "start": 4, "end": 8}])),
        )
        .unwrap();
        assert_eq!(p.active_loop(), Some((4.0, 8.0)));
        apply_all(&mut p, ops(json!([{"op": "set_loop", "on": false}]))).unwrap();
        assert_eq!(p.active_loop(), None);
        assert_eq!(
            p.transport.loop_region.unwrap().start,
            4.0,
            "kept for later"
        );
        apply_all(&mut p, ops(json!([{"op": "set_loop"}]))).unwrap();
        assert_eq!(p.active_loop(), Some((4.0, 8.0)));
        // Move just one end.
        apply_all(&mut p, ops(json!([{"op": "set_loop", "end": 12}]))).unwrap();
        assert_eq!(p.active_loop(), Some((4.0, 12.0)));
        let before = p.clone();
        for bad in [
            json!({"op": "set_loop", "end": 2}),
            json!({"op": "set_loop", "start": -1, "end": 4}),
            json!({"op": "set_loop", "start": 4, "end": 4}),
        ] {
            assert!(
                apply_all(&mut p, ops(json!([bad.clone()]))).is_err(),
                "{bad}"
            );
        }
        assert_eq!(p, before);
        assert!(p.same_music(&demo()), "the loop is not part of the music");
    }

    #[test]
    fn every_playhead_request_is_new() {
        let mut p = demo();
        let set = |p: &mut Project, play| {
            edit::apply(p, Op::SetPlayhead { beat: 8.0, play }).unwrap();
            p.transport.cue.unwrap()
        };
        let a = set(&mut p, None);
        let b = set(&mut p, Some(true));
        assert_eq!((a.beat, b.beat, b.play), (8.0, 8.0, Some(true)));
        assert_ne!(
            a.id, b.id,
            "the same spot twice still moves the playhead twice"
        );
        assert!(edit::apply(
            &mut p,
            Op::SetPlayhead {
                beat: -2.0,
                play: None
            }
        )
        .is_err());
    }

    #[test]
    fn automation_ops_write_clear_and_refuse_what_cannot_work() {
        let mut p = demo();
        apply_all(
            &mut p,
            ops(json!([
                {"op": "set_automation", "track_id": "t1", "param": "cutoff",
                 "points": [{"beat": 4, "value": 8000}, {"beat": 0, "value": 300, "shape": "smooth"}]},
                {"op": "set_automation", "track_id": "t2", "param": "volume",
                 "points": [{"beat": 0, "value": 5.0}]},
                {"op": "set_automation", "param": "master_volume",
                 "points": [{"beat": 0, "value": 0}, {"beat": 8, "value": 1}]}
            ])),
        )
        .unwrap();
        let cutoff = p.tracks[0].lane(Param::Cutoff).unwrap();
        assert_eq!(cutoff.points.len(), 2);
        assert_eq!(cutoff.points[0].shape, Shape::Smooth);
        assert_eq!(
            p.tracks[1].automated(Param::Volume, 1.0),
            Some(2.0),
            "clamped"
        );
        assert_eq!(p.automated(Param::MasterVolume, 8.0), Some(1.0));

        // Writing over a span replaces only what was there.
        edit::apply(
            &mut p,
            Op::SetAutomation {
                track_id: Some("t1".into()),
                param: Param::Cutoff,
                points: vec![Point::new(2.0, 1000.0)],
            },
        )
        .unwrap();
        assert_eq!(p.tracks[0].lane(Param::Cutoff).unwrap().points.len(), 3);

        // Things that cannot work change nothing.
        let before = p.clone();
        for bad in [
            json!({"op": "set_automation", "track_id": "t2", "param": "cutoff", "points": [{"beat": 0, "value": 1}]}),
            json!({"op": "set_automation", "track_id": "t1", "param": "reverb_size", "points": [{"beat": 0, "value": 1}]}),
            json!({"op": "set_automation", "param": "pan", "points": [{"beat": 0, "value": 1}]}),
            json!({"op": "set_automation", "track_id": "t1", "param": "pan", "points": [{"beat": -1, "value": 1}]}),
            json!({"op": "set_automation", "track_id": "t1", "param": "pan", "points": []}),
            json!({"op": "set_automation", "track_id": "t9", "param": "pan", "points": [{"beat": 0, "value": 1}]}),
        ] {
            assert!(
                edit::apply(&mut p, serde_json::from_value(bad.clone()).unwrap()).is_err(),
                "{bad}"
            );
            assert_eq!(p, before, "{bad}");
        }
        let nan = edit::apply(
            &mut p,
            Op::SetAutomation {
                track_id: Some("t1".into()),
                param: Param::Pan,
                points: vec![Point::new(0.0, f32::NAN)],
            },
        );
        assert!(nan.is_err());
        assert_eq!(p, before);

        // Clearing a stretch, a lane, everything.
        let r = apply_all(
            &mut p,
            ops(json!([{"op": "clear_automation", "track_id": "t1", "param": "cutoff", "from": 1, "to": 3}])),
        )
        .unwrap();
        assert_eq!(r[0]["removed"], 1);
        assert_eq!(p.tracks[0].lane(Param::Cutoff).unwrap().points.len(), 2);
        apply_all(
            &mut p,
            ops(json!([{"op": "clear_automation", "track_id": "t1"}])),
        )
        .unwrap();
        assert!(p.tracks[0].automation.is_empty());
        apply_all(
            &mut p,
            ops(json!([{"op": "clear_automation", "param": "master_volume"}])),
        )
        .unwrap();
        assert!(p.automation.is_empty());
        assert_eq!(p.tracks[1].automation.len(), 1, "other tracks keep theirs");
    }

    #[test]
    fn json_round_trip() {
        let p = demo();
        let s = serde_json::to_string(&p).unwrap();
        let q: Project = serde_json::from_str(&s).unwrap();
        assert_eq!(p, q);
    }

    #[test]
    fn render_produces_sound_and_silence_when_muted() {
        let mut p = demo();
        let audio = render::render(&p, 44_100);
        assert_eq!(audio.len(), ((2.0 + 2.0) * 44_100.0) as usize * 2);
        let peak = audio.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(peak > 0.05 && peak <= 1.0, "peak {peak}");
        for t in &mut p.tracks {
            t.mute = true;
        }
        assert!(render::render(&p, 44_100).iter().all(|s| *s == 0.0));
    }

    #[test]
    fn preview_sounds_without_moving_playhead() {
        let p = demo();
        let mut seq = render::Sequencer::new(22_050);
        seq.preview(&p, 0, 60);
        let mut buf = vec![0.0f32; 2048];
        seq.process_voices(&p, &mut buf);
        assert!(buf.iter().any(|s| s.abs() > 0.01));
        assert_eq!(seq.position(), 0.0);
    }

    #[test]
    fn held_preview_sustains_until_released() {
        let p = demo();
        let mut seq = render::Sequencer::new(22_050);
        seq.preview_hold(&p, 0, 60);
        let mut buf = vec![0.0f32; 2 * 22_050]; // one second, longer than a normal preview
        seq.process_voices(&p, &mut buf);
        assert!(
            buf[buf.len() - 200..].iter().any(|s| s.abs() > 0.01),
            "still sounding"
        );
        seq.release_held();
        seq.process_voices(&p, &mut buf); // release (0.25 s) finishes within this second
        assert_eq!(seq.active_voices(), 0);
    }

    #[test]
    fn solo_isolates_track() {
        let mut p = demo();
        p.tracks[1].solo = true;
        p.tracks[1].clips.clear();
        assert!(render::render(&p, 22_050).iter().all(|s| *s == 0.0));
    }
}
