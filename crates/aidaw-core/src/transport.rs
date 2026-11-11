//! Playback settings that live in the project file so the app and the AI
//! share them: the loop region (A/B loop) and the AI's requests to move the
//! app's playhead. They are not part of the music: WAV export and analysis
//! play the whole song from the start whatever is set here.

use serde::{Deserialize, Serialize};

use crate::model::Project;

/// Shortest loop region, in beats (a 1/64 note).
pub const MIN_LOOP_BEATS: f64 = 1.0 / 16.0;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Transport {
    /// The part of the song live playback repeats, if one was set.
    #[serde(default, rename = "loop", skip_serializing_if = "Option::is_none")]
    pub loop_region: Option<LoopRegion>,
    /// The AI's last request to move the app's playhead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cue: Option<Cue>,
}

impl Transport {
    pub fn is_empty(&self) -> bool {
        *self == Transport::default()
    }
}

/// An A/B loop: playback jumps from `end` back to `start` (both in beats).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LoopRegion {
    pub start: f64,
    pub end: f64,
    /// Off keeps the region for later; playback then runs through the song.
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub on: bool,
}

impl LoopRegion {
    /// Whether the numbers make a loop playback can use.
    pub fn is_valid(&self) -> bool {
        self.start.is_finite()
            && self.end.is_finite()
            && self.start >= 0.0
            && self.end - self.start >= MIN_LOOP_BEATS - 1e-9
    }
}

/// "Move the playhead here": written by the AI, carried out by the app.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Cue {
    /// Where the playhead goes, in beats.
    pub beat: f64,
    /// Start (true) or stop (false) playing there; unset leaves it as it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub play: Option<bool>,
    /// New for every request, so asking for the same spot twice moves the
    /// playhead twice. The app acts on ids it has not seen yet.
    pub id: u64,
}

fn yes() -> bool {
    true
}

fn is_true(v: &bool) -> bool {
    *v
}

impl Project {
    /// The loop region, when there is one, it is switched on and usable.
    pub fn active_loop(&self) -> Option<(f64, f64)> {
        self.transport
            .loop_region
            .filter(|r| r.on && r.is_valid())
            .map(|r| (r.start, r.end))
    }

    /// What live playback repeats: the loop region while it is on, else the
    /// whole song (from the start to [`Project::loop_end_beats`]).
    pub fn playback_loop(&self) -> Option<(f64, f64)> {
        self.active_loop()
            .or_else(|| self.loop_end_beats().map(|end| (0.0, end)))
    }

    /// Whether both versions hold the same music, whatever their loop
    /// regions and playhead requests are, and in whatever order their notes
    /// are listed (an editor may hold them out of order for a moment).
    pub fn same_music(&self, other: &Project) -> bool {
        if self.transport == other.transport && self == other {
            return true;
        }
        let mut a = self.clone();
        a.transport = other.transport.clone();
        if a == *other || (a.notes_sorted() && other.notes_sorted()) {
            return a == *other;
        }
        let mut b = other.clone();
        a.sort_notes();
        b.sort_notes();
        a == b
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_is_left_out_of_files_until_it_is_used() {
        let mut p = Project::new("x", 120.0);
        let plain = serde_json::to_string(&p).unwrap();
        assert!(!plain.contains("transport"), "{plain}");
        p.transport.loop_region = Some(LoopRegion {
            start: 8.0,
            end: 16.0,
            on: true,
        });
        let s = serde_json::to_string(&p).unwrap();
        assert!(
            s.contains(r#""transport":{"loop":{"start":8.0,"end":16.0}}"#),
            "{s}"
        );
        assert_eq!(serde_json::from_str::<Project>(&s).unwrap(), p);
        p.transport.loop_region.as_mut().unwrap().on = false;
        p.transport.cue = Some(Cue {
            beat: 4.0,
            play: Some(true),
            id: 7,
        });
        let s = serde_json::to_string(&p).unwrap();
        assert!(s.contains(r#""on":false"#), "{s}");
        assert!(
            s.contains(r#""cue":{"beat":4.0,"play":true,"id":7}"#),
            "{s}"
        );
        assert_eq!(serde_json::from_str::<Project>(&s).unwrap(), p);
    }

    #[test]
    fn playback_loops_the_region_or_else_the_song() {
        let mut p: Project = serde_json::from_value(serde_json::json!({
            "name": "x", "bpm": 120.0,
            "tracks": [{"id": "t1", "name": "A", "instrument": {"type": "drums"},
                        "clips": [{"id": "c1", "start": 0, "length": 8,
                                   "notes": [{"pitch": 36, "start": 5, "duration": 1}]}]}]
        }))
        .unwrap();
        assert_eq!(p.active_loop(), None);
        assert_eq!(p.playback_loop(), Some((0.0, 8.0)));
        p.transport.loop_region = Some(LoopRegion {
            start: 2.0,
            end: 6.0,
            on: true,
        });
        assert_eq!(p.playback_loop(), Some((2.0, 6.0)));
        // Switched off, or unusable (hand-edited files), it is ignored.
        for r in [
            LoopRegion {
                start: 2.0,
                end: 6.0,
                on: false,
            },
            LoopRegion {
                start: 6.0,
                end: 2.0,
                on: true,
            },
            LoopRegion {
                start: f64::NAN,
                end: 2.0,
                on: true,
            },
        ] {
            p.transport.loop_region = Some(r);
            assert_eq!(p.playback_loop(), Some((0.0, 8.0)), "{r:?}");
        }
    }

    #[test]
    fn same_music_ignores_the_transport() {
        let a = Project::new("x", 120.0);
        let mut b = a.clone();
        b.transport.cue = Some(Cue {
            beat: 1.0,
            play: None,
            id: 1,
        });
        assert!(a.same_music(&b) && b.same_music(&a));
        assert_ne!(a, b);
        b.bpm = 90.0;
        assert!(!a.same_music(&b));
    }

    #[test]
    fn same_music_ignores_the_order_of_the_notes() {
        let mut a = Project::new("x", 120.0);
        let ops = serde_json::from_value(serde_json::json!([
            {"op": "add_track", "name": "Keys"},
            {"op": "add_clip", "track_id": "t1", "start": 0, "length": 4, "notes": [
                {"pitch": 60, "start": 0, "duration": 1},
                {"pitch": 62, "start": 1, "duration": 1},
                {"pitch": 64, "start": 2, "duration": 1}]}
        ]))
        .unwrap();
        crate::edit::apply_all(&mut a, ops).unwrap();
        let mut b = a.clone();
        b.tracks[0].clips[0].notes.swap(0, 2);
        assert_ne!(a, b);
        assert!(a.same_music(&b) && b.same_music(&a));
        b.tracks[0].clips[0].notes[0].velocity = 90;
        assert!(!a.same_music(&b));
    }
}
