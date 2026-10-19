//! Musical keys: highlight and optionally "stay in key" so random clicks still sound good.

use crate::i18n::tr;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Scale {
    Off,
    Major,
    Minor,
    Pentatonic,
    MinorPentatonic,
    Blues,
}

pub const ROOTS: [&str; 12] = [
    "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
];

impl Scale {
    pub const ALL: [Scale; 6] = [
        Scale::Off,
        Scale::Major,
        Scale::Minor,
        Scale::Pentatonic,
        Scale::MinorPentatonic,
        Scale::Blues,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Scale::Off => tr("Any notes"),
            Scale::Major => tr("Major (happy)"),
            Scale::Minor => tr("Minor (sad)"),
            Scale::Pentatonic => tr("Pentatonic (can't go wrong)"),
            Scale::MinorPentatonic => tr("Minor pentatonic"),
            Scale::Blues => tr("Blues"),
        }
    }

    fn intervals(self) -> &'static [u8] {
        match self {
            Scale::Off => &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11],
            Scale::Major => &[0, 2, 4, 5, 7, 9, 11],
            Scale::Minor => &[0, 2, 3, 5, 7, 8, 10],
            Scale::Pentatonic => &[0, 2, 4, 7, 9],
            Scale::MinorPentatonic => &[0, 3, 5, 7, 10],
            Scale::Blues => &[0, 3, 5, 6, 7, 10],
        }
    }

    pub fn contains(self, root: u8, pitch: u8) -> bool {
        self.intervals().contains(&((pitch + 12 - root % 12) % 12))
    }

    /// Nearest in-key pitch (ties go down).
    pub fn snap(self, root: u8, pitch: u8) -> u8 {
        (0..12i16)
            .flat_map(|d| [pitch as i16 - d, pitch as i16 + d])
            .filter(|p| (0..=127).contains(p))
            .map(|p| p as u8)
            .find(|p| self.contains(root, *p))
            .unwrap_or(pitch)
    }

    /// The pitch `steps` notes of the key above `pitch` (below when
    /// negative), or `None` past either end of the keyboard. With `Off`
    /// every key counts, so a step is a semitone.
    pub fn step(self, root: u8, pitch: u8, steps: i32) -> Option<u8> {
        let mut p = pitch as i32;
        let mut left = steps.abs();
        while left > 0 {
            p += steps.signum();
            if !(0..=127).contains(&p) {
                return None;
            }
            if self.contains(root, p as u8) {
                left -= 1;
            }
        }
        Some(p as u8)
    }

    /// How many [`Self::step`]s take `from` to `to`, which is in the key.
    pub fn steps_between(self, root: u8, from: u8, to: u8) -> i32 {
        let count =
            |r: std::ops::RangeInclusive<u8>| r.filter(|&p| self.contains(root, p)).count() as i32;
        if to >= from {
            count(from.saturating_add(1)..=to)
        } else {
            -count(to..=from - 1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapping() {
        assert!(Scale::Major.contains(0, 64));
        assert!(!Scale::Major.contains(0, 61));
        assert_eq!(Scale::Major.snap(0, 61), 60);
        assert_eq!(Scale::Pentatonic.snap(0, 65), 64);
        assert_eq!(Scale::Minor.snap(9, 61), 60); // A minor: C# -> C
        assert_eq!(Scale::Off.snap(0, 61), 61);
    }

    #[test]
    fn stepping_through_the_key() {
        // C major: C4 up two steps is E4, E4 down two is C4.
        assert_eq!(Scale::Major.step(0, 60, 2), Some(64));
        assert_eq!(Scale::Major.step(0, 64, -2), Some(60));
        assert_eq!(Scale::Major.step(0, 64, 0), Some(64));
        // An out-of-key note moves to the next key note on that side.
        assert_eq!(Scale::Major.step(0, 61, 1), Some(62));
        assert_eq!(Scale::Major.step(0, 61, -1), Some(60));
        assert_eq!(Scale::Off.step(0, 60, -3), Some(57));
        assert_eq!(Scale::Major.step(0, 127, 1), None);
        assert_eq!(Scale::Off.step(0, 1, -2), None);

        assert_eq!(Scale::Major.steps_between(0, 60, 64), 2);
        assert_eq!(Scale::Major.steps_between(0, 64, 60), -2);
        assert_eq!(Scale::Major.steps_between(0, 61, 60), -1);
        assert_eq!(Scale::Major.steps_between(0, 61, 62), 1);
        assert_eq!(Scale::Off.steps_between(0, 127, 127), 0);
        assert_eq!(Scale::Off.steps_between(0, 0, 127), 127);
        // A whole chord moved by the same steps stays in key: C E G -> E G B.
        let up = Scale::Major.steps_between(0, 60, 64);
        let moved: Vec<u8> = [60, 64, 67]
            .iter()
            .filter_map(|&p| Scale::Major.step(0, p, up))
            .collect();
        assert_eq!(moved, [64, 67, 71]);
    }
}
