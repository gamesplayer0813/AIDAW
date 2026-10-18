//! Pitch names <-> MIDI note numbers. Middle C is "C4" = 60.

use serde::{de, Deserialize, Deserializer};

const NAMES: [&str; 12] = [
    "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
];

pub fn name(pitch: u8) -> String {
    let octave = pitch as i32 / 12 - 1;
    format!("{}{}", NAMES[pitch as usize % 12], octave)
}

pub fn is_black_key(pitch: u8) -> bool {
    matches!(pitch % 12, 1 | 3 | 6 | 8 | 10)
}

/// Parse "C4", "f#3", "Bb-1", "60".
pub fn parse(s: &str) -> Option<u8> {
    let s = s.trim();
    if let Ok(n) = s.parse::<i32>() {
        return u8::try_from(n).ok().filter(|n| *n <= 127);
    }
    let mut chars = s.chars();
    let base = match chars.next()?.to_ascii_uppercase() {
        'C' => 0,
        'D' => 2,
        'E' => 4,
        'F' => 5,
        'G' => 7,
        'A' => 9,
        'B' => 11,
        _ => return None,
    };
    let rest: String = chars.collect();
    let (accidental, octave) = if let Some(r) = rest.strip_prefix('#') {
        (1, r)
    } else if let Some(r) = rest.strip_prefix('b') {
        (-1, r)
    } else {
        (0, rest.as_str())
    };
    let octave: i32 = octave.parse().ok()?;
    let n = (octave + 1) * 12 + base + accidental;
    u8::try_from(n).ok().filter(|n| *n <= 127)
}

pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<u8, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum P {
        Num(i64),
        Name(String),
    }
    match P::deserialize(d)? {
        P::Num(n) => u8::try_from(n)
            .ok()
            .filter(|n| *n <= 127)
            .ok_or_else(|| de::Error::custom(format!("pitch {n} out of range 0..=127"))),
        P::Name(s) => {
            parse(&s).ok_or_else(|| de::Error::custom(format!("invalid pitch name {s:?}")))
        }
    }
}

pub fn frequency(pitch: f32) -> f32 {
    440.0 * 2f32.powf((pitch - 69.0) / 12.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        assert_eq!(parse("C4"), Some(60));
        assert_eq!(parse("A4"), Some(69));
        assert_eq!(parse("f#3"), Some(54));
        assert_eq!(parse("Bb2"), Some(46));
        assert_eq!(parse("C-1"), Some(0));
        assert_eq!(parse("G9"), Some(127));
        assert_eq!(parse("H2"), None);
        for p in 0..=127u8 {
            assert_eq!(parse(&name(p)), Some(p));
        }
    }
}
