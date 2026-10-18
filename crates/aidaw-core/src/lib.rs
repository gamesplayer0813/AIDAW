//! Core of AIDAW: the project document, editing operations, built-in
//! instruments and the renderer shared by real-time playback and export.

pub mod edit;
pub mod io;
pub mod model;
pub mod music;
pub mod pitch;
pub mod presets;
pub mod render;

pub use model::{Clip, Instrument, Note, Project, Track, Waveform};

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
