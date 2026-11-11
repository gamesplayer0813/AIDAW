//! Where the sequencer meets hosted plugins. The core knows nothing about
//! VST3: a [`Rack`] (built by the `aidaw-plugin` crate for one snapshot of
//! the project) holds each track's plugins, and the [`crate::render::Sequencer`]
//! drives it block by block: it hands plugin instruments their notes as
//! [`NoteEvent`]s, lets the rack run each track's effects, and mixes the
//! result like any other track.

/// Longest block the sequencer renders in one go; racks prepare their
/// plugins and buffers for blocks of up to this many frames.
pub const MAX_BLOCK: usize = 1024;

/// A note starting or ending inside a block, for a plugin instrument.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NoteEvent {
    /// Frame within the block.
    pub offset: u32,
    pub pitch: u8,
    /// 0..1 (the note's velocity / 127).
    pub velocity: f32,
    /// True for a note-on, false for a note-off.
    pub on: bool,
    /// Pairs a note-off with its note-on, so overlapping notes of the same
    /// pitch end in the right order.
    pub id: i32,
}

/// The song's clock at the start of a block, for tempo-synced plugins.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transport {
    pub sample_rate: f64,
    pub bpm: f64,
    pub beats_per_bar: u32,
    /// Playhead in beats at the start of the block.
    pub beat: f64,
    /// Whether the song is playing (false while stopped, when only note
    /// previews and tails are heard).
    pub playing: bool,
    /// Where playback wraps back to beat 0, when it loops.
    pub loop_end: Option<f64>,
    /// Frames the sequencer has rendered so far, playing or not: a steady
    /// clock that never jumps.
    pub frames: u64,
}

/// The plugins of one project snapshot, driven by the sequencer.
///
/// A rack is built away from the audio thread (loading plugins is slow and
/// allocates); every method below is then called on the audio thread, once
/// per block, and must neither block nor allocate. Track numbers are
/// indexes into the snapshot's `tracks`.
pub trait Rack: Send {
    /// A stable id of track `track`'s plugin instrument, when it has one
    /// that loaded. Notes are tied to this id, so a note always ends on the
    /// plugin it started on, even when tracks move around.
    fn instrument(&self, track: usize) -> Option<u64>;
    /// Whether track `track` is rendered through the rack: it plays a plugin
    /// instrument or has effects. Its built-in voices (if any) then sound
    /// through its bus too, so its effects hear them.
    fn routes(&self, track: usize) -> bool;
    /// Track `track`'s stereo bus for this block (at least [`MAX_BLOCK`]
    /// frames per side). The sequencer clears it, adds the track's built-in
    /// voices, calls [`Rack::process`] and then mixes it like any track.
    fn bus(&mut self, track: usize) -> Option<[&mut [f32]; 2]>;
    /// Run track `track`'s plugin instrument with `events` (sorted by
    /// offset), adding what it plays to the bus, then its effects on the bus.
    fn process(&mut self, track: usize, frames: usize, events: &[NoteEvent], transport: &Transport);
    /// Extra seconds an offline render should run after the song, so plugin
    /// tails (reverbs, delays) ring out.
    fn tail_seconds(&self) -> f64 {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edit::{apply_all, Op};
    use crate::model::{Instrument, Note, PluginRef, Project};
    use crate::render::{self, Sequencer};

    const SR: u32 = 24_000;
    /// 120 BPM at 24 kHz.
    const FRAMES_PER_BEAT: u64 = 12_000;
    const UID: &str = "00112233445566778899AABBCCDDEEFF";

    /// A stand-in for real plugins. Instruments sound a steady 0.25 per held
    /// note; effects scale the bus by `fx_gain`. Every event is logged with
    /// its absolute frame.
    struct FakeRack {
        /// Per track: (instrument slot, has effects).
        tracks: Vec<(Option<u64>, bool)>,
        buses: Vec<[Vec<f32>; 2]>,
        held: Vec<Vec<i32>>,
        fx_gain: f32,
        log: Vec<(u64, u64, NoteEvent)>,
        transports: Vec<Transport>,
    }

    impl FakeRack {
        fn new(tracks: Vec<(Option<u64>, bool)>) -> FakeRack {
            let n = tracks.len();
            FakeRack {
                tracks,
                buses: vec![[vec![0.0; MAX_BLOCK], vec![0.0; MAX_BLOCK]]; n],
                held: vec![Vec::new(); n],
                fx_gain: 1.0,
                log: Vec::new(),
                transports: Vec::new(),
            }
        }

        /// (frame, on, pitch) of every event for `slot`.
        fn events(&self, slot: u64) -> Vec<(u64, bool, u8)> {
            self.log
                .iter()
                .filter(|(s, ..)| *s == slot)
                .map(|(_, at, e)| (*at, e.on, e.pitch))
                .collect()
        }
    }

    impl Rack for FakeRack {
        fn instrument(&self, track: usize) -> Option<u64> {
            self.tracks.get(track)?.0
        }
        fn routes(&self, track: usize) -> bool {
            self.tracks.get(track).is_some_and(|t| t.0.is_some() || t.1)
        }
        fn bus(&mut self, track: usize) -> Option<[&mut [f32]; 2]> {
            let [l, r] = self.buses.get_mut(track)?;
            Some([l.as_mut_slice(), r.as_mut_slice()])
        }
        fn process(&mut self, track: usize, frames: usize, events: &[NoteEvent], t: &Transport) {
            self.transports.push(*t);
            let (slot, fx) = self.tracks[track];
            let [l, r] = &mut self.buses[track];
            if let Some(slot) = slot {
                let mut next = events.iter().peekable();
                for f in 0..frames {
                    while let Some(e) = next.next_if(|e| e.offset as usize == f) {
                        self.log.push((slot, t.frames + f as u64, *e));
                        if e.on {
                            self.held[track].push(e.id);
                        } else {
                            self.held[track].retain(|id| *id != e.id);
                        }
                    }
                    let s = 0.25 * self.held[track].len() as f32;
                    l[f] += s;
                    r[f] += s;
                }
                assert!(next.next().is_none(), "event offsets inside the block");
            }
            if fx {
                for x in l[..frames].iter_mut().chain(r[..frames].iter_mut()) {
                    *x *= self.fx_gain;
                }
            }
        }
        fn tail_seconds(&self) -> f64 {
            1.5
        }
    }

    fn note(pitch: u8, start: f64, duration: f64) -> Note {
        Note {
            pitch,
            start,
            duration,
            velocity: 127,
        }
    }

    /// Track 1 a built-in synth, track 2 a plugin instrument.
    fn song(plugin_notes: Vec<Note>) -> Project {
        let mut p = Project::new("rack", 120.0);
        apply_all(
            &mut p,
            vec![
                Op::AddTrack {
                    name: "Synth".into(),
                    instrument: None,
                    gain: Some(1.0),
                    pan: None,
                    reverb: None,
                },
                Op::AddTrack {
                    name: "Plugin".into(),
                    instrument: Some(Instrument::Vst3(PluginRef::new("Fake", UID))),
                    gain: Some(1.0),
                    pan: None,
                    reverb: None,
                },
                Op::AddClip {
                    track_id: "t2".into(),
                    start: 0.0,
                    length: 8.0,
                    name: None,
                    notes: plugin_notes,
                },
            ],
        )
        .unwrap();
        p
    }

    fn run(seq: &mut Sequencer, p: &Project, rack: &mut FakeRack, frames: usize) -> Vec<f32> {
        let mut out = vec![0.0; frames * 2];
        for chunk in out.chunks_mut(2 * 300) {
            seq.process_with(p, Some(rack), chunk);
        }
        out
    }

    #[test]
    fn plugin_notes_start_and_end_on_the_exact_frame() {
        let p = song(vec![
            note(60, 0.5, 1.0),
            note(64, 1.0, 0.25),
            note(60, 2.0, 1.0),
        ]);
        let mut rack = FakeRack::new(vec![(None, false), (Some(7), false)]);
        let mut seq = Sequencer::new(SR);
        run(&mut seq, &p, &mut rack, 4 * FRAMES_PER_BEAT as usize);
        let b = |beats: f64| (beats * FRAMES_PER_BEAT as f64) as u64;
        let want = [
            (b(0.5), true, 60),
            (b(1.0), true, 64),
            (b(1.25), false, 64),
            (b(1.5), false, 60),
            (b(2.0), true, 60),
            (b(3.0), false, 60),
        ];
        let got = rack.events(7);
        assert_eq!(got.len(), want.len(), "{got:?}");
        for (g, w) in got.iter().zip(want) {
            // Within a frame: the playhead is a float, like for built-in voices.
            assert!(
                g.0.abs_diff(w.0) <= 1 && (g.1, g.2) == (w.1, w.2),
                "{got:?}"
            );
        }
        assert_eq!(seq.active_plugin_notes(), 0);
        let t = rack.transports[0];
        assert_eq!(
            (t.sample_rate, t.bpm, t.beats_per_bar),
            (24_000.0, 120.0, 4)
        );
        assert!(t.playing && t.beat == 0.0 && t.loop_end.is_none());
    }

    #[test]
    fn a_plugin_sounds_through_the_track_volume_and_pan() {
        let mut p = song(vec![note(60, 0.0, 4.0)]);
        let mut rack = FakeRack::new(vec![(None, false), (Some(1), false)]);
        let mut seq = Sequencer::new(SR);
        let out = run(&mut seq, &p, &mut rack, 2048);
        // 0.25 at unity gain, centred: the master limiter leaves it alone.
        let master = p.master_gain;
        assert!((out[1500] - 0.25 * master).abs() < 1e-4, "{}", out[1500]);
        assert_eq!(out[1500], out[1501]);

        p.tracks[1].pan = 1.0; // hard right
        let out = run(&mut seq, &p, &mut rack, 2048);
        // Equal-power pan: all of it on the right, 3 dB up.
        let right = 0.25 * std::f32::consts::SQRT_2 * master;
        assert!(out[2000].abs() < 1e-4, "{:?}", &out[2000..2002]);
        assert!((out[2001] - right).abs() < 1e-4, "{:?}", &out[2000..2002]);

        p.tracks[1].mute = true;
        let out = run(&mut seq, &p, &mut rack, 2048);
        assert!(out[1000..].iter().all(|s| s.abs() < 1e-6));
    }

    #[test]
    fn effects_process_built_in_instruments_too() {
        let mut p = song(Vec::new());
        p.tracks[0].clips.push(crate::Clip {
            id: "c9".into(),
            name: String::new(),
            start: 0.0,
            length: 4.0,
            notes: vec![note(57, 0.0, 2.0)],
        });
        let frames = 2 * FRAMES_PER_BEAT as usize;
        let plain = render::render(&p, SR)[..frames * 2].to_vec();

        // A pass-through effect on the synth track changes nothing.
        let mut rack = FakeRack::new(vec![(None, true), (Some(1), false)]);
        let mut seq = Sequencer::new(SR);
        let through = run(&mut seq, &p, &mut rack, frames);
        let diff = plain
            .iter()
            .zip(&through)
            .fold(0.0f32, |m, (a, b)| m.max((a - b).abs()));
        assert!(diff < 1e-5, "pass-through differs by {diff}");
        assert!(through.iter().any(|s| s.abs() > 0.05));

        // A silencing effect silences it, including its reverb send.
        p.tracks[0].reverb = 1.0;
        rack.fx_gain = 0.0;
        let mut seq = Sequencer::new(SR);
        let silenced = run(&mut seq, &p, &mut rack, frames);
        assert!(silenced.iter().all(|s| *s == 0.0));
    }

    #[test]
    fn seeking_and_stopping_end_plugin_notes() {
        let p = song(vec![note(60, 0.0, 8.0)]);
        let mut rack = FakeRack::new(vec![(None, false), (Some(3), false)]);
        let mut seq = Sequencer::new(SR);
        run(&mut seq, &p, &mut rack, 1000);
        // Seek into the middle of the long note: it ends, then (chased)
        // starts again at the new position.
        seq.seek(4.0);
        run(&mut seq, &p, &mut rack, 600);
        let ev = rack.events(3);
        assert_eq!(ev.len(), 3, "{ev:?}");
        assert_eq!((ev[0].1, ev[1].1, ev[2].1), (true, false, true));
        assert_eq!(
            ev[1].0, ev[2].0,
            "the old note ends where the new one starts"
        );
        assert_eq!(seq.active_plugin_notes(), 1);
        // Stopping (pause = seek to the same place) ends it while nothing plays.
        let pos = seq.position();
        seq.seek(pos);
        let mut buf = vec![0.0; 600];
        seq.process_voices_with(&p, Some(&mut rack), &mut buf);
        assert_eq!(rack.events(3).last().map(|e| e.1), Some(false));
        assert_eq!(seq.active_plugin_notes(), 0);
        assert!(!rack.transports.last().unwrap().playing);
    }

    #[test]
    fn chased_plugin_notes_end_on_time() {
        let p = song(vec![note(62, 0.0, 3.0)]);
        let mut rack = FakeRack::new(vec![(None, false), (Some(5), false)]);
        let mut seq = Sequencer::new(SR);
        seq.seek(1.0);
        run(&mut seq, &p, &mut rack, 3 * FRAMES_PER_BEAT as usize);
        // Picked up at beat 1, it still ends at beat 3 (two beats later).
        assert_eq!(
            rack.events(5),
            vec![(0, true, 62), (2 * FRAMES_PER_BEAT, false, 62)]
        );
    }

    #[test]
    fn previews_and_held_keys_reach_plugins() {
        let p = song(Vec::new());
        let mut rack = FakeRack::new(vec![(None, false), (Some(9), false)]);
        let mut seq = Sequencer::new(SR);
        let mut buf = vec![0.0; 2 * 512];
        seq.preview(&p, 1, 67);
        seq.process_voices_with(&p, Some(&mut rack), &mut buf);
        assert!(buf.iter().any(|s| *s > 0.1), "the preview sounds");
        for _ in 0..40 {
            seq.process_voices_with(&p, Some(&mut rack), &mut buf);
        }
        assert_eq!(rack.events(9), vec![(0, true, 67), (8400, false, 67)]);

        seq.preview_hold(&p, 1, 69);
        for _ in 0..100 {
            seq.process_voices_with(&p, Some(&mut rack), &mut buf);
        }
        assert_eq!(seq.active_plugin_notes(), 1, "held until released");
        seq.release_held();
        seq.process_voices_with(&p, Some(&mut rack), &mut buf);
        assert_eq!(seq.active_plugin_notes(), 0);
        assert_eq!(rack.events(9).last().map(|e| (e.1, e.2)), Some((false, 69)));
    }

    #[test]
    fn notes_stay_with_their_plugin_when_tracks_move() {
        let p = song(vec![note(60, 0.0, 1.0)]);
        let mut rack = FakeRack::new(vec![(None, false), (Some(4), false)]);
        let mut seq = Sequencer::new(SR);
        run(&mut seq, &p, &mut rack, 600);
        // The synth track is deleted mid-note: the plugin track is now first.
        let mut moved = p.clone();
        moved.tracks.remove(0);
        let mut rack2 = FakeRack::new(vec![(Some(4), false)]);
        run(&mut seq, &moved, &mut rack2, FRAMES_PER_BEAT as usize);
        assert_eq!(rack2.events(4), vec![(FRAMES_PER_BEAT, false, 60)]);
        // A plugin that went away takes its notes with it.
        let p = song(vec![note(60, 0.0, 4.0)]);
        let mut seq = Sequencer::new(SR);
        run(&mut seq, &p, &mut rack, 600);
        assert_eq!(seq.active_plugin_notes(), 1);
        let mut gone = FakeRack::new(vec![(None, false), (Some(99), false)]);
        let mut buf = vec![0.0; 600];
        seq.process_with(&p, Some(&mut gone), &mut buf);
        assert_eq!(seq.active_plugin_notes(), 0);
        assert!(gone.events(4).is_empty());
    }

    #[test]
    fn loops_and_renders_tell_plugins_the_time() {
        let p = song(vec![note(60, 0.0, 1.0)]);
        let mut rack = FakeRack::new(vec![(None, false), (Some(2), false)]);
        let mut seq = Sequencer::new(SR);
        let mut buf = vec![0.0; 2 * 1000];
        seq.process_looping_with(&p, Some(&mut rack), &mut buf, Some(4.0));
        assert_eq!(rack.transports[0].loop_end, Some(4.0));
        assert_eq!(rack.transports.last().unwrap().frames, 0);
        seq.process_looping_with(&p, Some(&mut rack), &mut buf, Some(4.0));
        assert_eq!(rack.transports.last().unwrap().frames, 1000);

        // Offline renders add the plugins' tail.
        let audio = render::render_with(&p, Some(&mut rack), SR);
        let plain = render::render(&p, SR);
        assert_eq!(audio.len() - plain.len(), 2 * (1.5 * SR as f64) as usize);
    }

    #[test]
    fn without_a_rack_plugin_tracks_are_silent() {
        let p = song(vec![note(60, 0.0, 4.0)]);
        let audio = render::render(&p, SR);
        assert!(audio.iter().all(|s| *s == 0.0));
    }
}
