//! The song clock: which beat is being heard at a given moment.
//!
//! The output callback knows which beat it is about to render and, from
//! the device's timestamps, when that audio will come out of the speakers.
//! It publishes that pair here. The input callback and the MIDI thread then
//! work out which beat a sound was played on: the beat the musician was
//! hearing at the time, so recordings line up with the song no matter how
//! much latency the devices add.
//!
//! All times are nanoseconds since the clock was made, measured with one
//! `Instant` that every thread shares. A device's own timestamps are only
//! used for the delay between "now" and when its audio is heard (output)
//! or was captured (input), so devices on different clocks still agree.
//!
//! The values are published with a seqlock: one writer (the output
//! callback), any number of readers, and nobody ever blocks.

use std::sync::atomic::{fence, AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// Longest device delay believed; anything larger is a broken timestamp.
const MAX_DELAY: Duration = Duration::from_secs(2);

/// One published position of the song.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Moment {
    /// When the frame at `beat` is heard, in ns since the clock's epoch.
    pub heard_ns: u64,
    pub beat: f64,
    /// How fast the song moves; 0 while it stands still.
    pub beats_per_ns: f64,
}

impl Moment {
    /// The beat heard at `ns`, before or after `heard_ns`.
    pub fn beat_at(&self, ns: u64) -> f64 {
        self.beat + (ns as f64 - self.heard_ns as f64) * self.beats_per_ns
    }

    pub fn moving(&self) -> bool {
        self.beats_per_ns > 0.0
    }
}

pub(crate) struct Clock {
    epoch: Instant,
    /// Even when the fields below are consistent, odd while being written.
    seq: AtomicU64,
    heard_ns: AtomicU64,
    beat: AtomicU64,
    beats_per_ns: AtomicU64,
}

impl Clock {
    pub fn new() -> Clock {
        Clock {
            epoch: Instant::now(),
            seq: AtomicU64::new(0),
            heard_ns: AtomicU64::new(0),
            beat: AtomicU64::new(0),
            beats_per_ns: AtomicU64::new(0),
        }
    }

    pub fn now_ns(&self) -> u64 {
        self.epoch.elapsed().as_nanos() as u64
    }

    /// Publish where the song is. Only the output callback calls this.
    pub fn publish(&self, m: Moment) {
        let s = self.seq.load(Ordering::Relaxed);
        self.seq.store(s.wrapping_add(1), Ordering::Relaxed);
        fence(Ordering::Release);
        self.heard_ns.store(m.heard_ns, Ordering::Relaxed);
        self.beat.store(m.beat.to_bits(), Ordering::Relaxed);
        self.beats_per_ns
            .store(m.beats_per_ns.to_bits(), Ordering::Relaxed);
        self.seq.store(s.wrapping_add(2), Ordering::Release);
    }

    /// The last published position; `None` before the first one (or in
    /// the unlikely case the writer was interrupted mid-write for long).
    pub fn read(&self) -> Option<Moment> {
        for _ in 0..64 {
            let s1 = self.seq.load(Ordering::Acquire);
            if s1 == 0 {
                return None;
            }
            if s1 % 2 == 1 {
                std::hint::spin_loop();
                continue;
            }
            let m = Moment {
                heard_ns: self.heard_ns.load(Ordering::Relaxed),
                beat: f64::from_bits(self.beat.load(Ordering::Relaxed)),
                beats_per_ns: f64::from_bits(self.beats_per_ns.load(Ordering::Relaxed)),
            };
            fence(Ordering::Acquire);
            if self.seq.load(Ordering::Relaxed) == s1 {
                return Some(m);
            }
        }
        None
    }

    /// The beat heard right now, while the song moves.
    pub fn beat_now(&self) -> Option<f64> {
        let m = self.read().filter(Moment::moving)?;
        Some(m.beat_at(self.now_ns()))
    }
}

/// How far apart two timestamps of one stream are, clamped to something
/// believable (0 when the device reports nonsense).
pub(crate) fn delay(later: cpal::StreamInstant, earlier: cpal::StreamInstant) -> Duration {
    let d = later.saturating_duration_since(earlier);
    if d > MAX_DELAY {
        Duration::ZERO
    } else {
        d
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn moments_map_times_to_beats() {
        let m = Moment {
            heard_ns: 1_000_000_000,
            beat: 8.0,
            beats_per_ns: 2.0 / 1e9, // 120 BPM
        };
        assert_eq!(m.beat_at(1_000_000_000), 8.0);
        assert!((m.beat_at(1_500_000_000) - 9.0).abs() < 1e-9);
        // Before the moment too (a take that started during the count-in).
        assert!((m.beat_at(250_000_000) - 6.5).abs() < 1e-9);
    }

    #[test]
    fn readers_never_see_a_half_written_moment() {
        let clock = Arc::new(Clock::new());
        assert_eq!(clock.read(), None);
        let writer = {
            let clock = clock.clone();
            std::thread::spawn(move || {
                for i in 1..200_000u64 {
                    // Every field encodes the same number.
                    clock.publish(Moment {
                        heard_ns: i,
                        beat: i as f64,
                        beats_per_ns: i as f64 * 2.0,
                    });
                }
            })
        };
        let mut seen = 0;
        while !writer.is_finished() {
            if let Some(m) = clock.read() {
                assert_eq!(m.beat, m.heard_ns as f64);
                assert_eq!(m.beats_per_ns, m.heard_ns as f64 * 2.0);
                seen += 1;
            }
        }
        writer.join().unwrap();
        assert!(seen > 0);
        assert_eq!(clock.read().unwrap().heard_ns, 199_999);
    }

    #[test]
    fn broken_device_delays_are_ignored() {
        let t = |ms| cpal::StreamInstant::from_millis(ms);
        assert_eq!(delay(t(1_010), t(1_000)), Duration::from_millis(10));
        assert_eq!(delay(t(1_000), t(1_010)), Duration::ZERO);
        assert_eq!(delay(t(90_000), t(1_000)), Duration::ZERO);
    }
}
