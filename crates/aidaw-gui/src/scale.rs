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
}
