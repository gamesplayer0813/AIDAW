//! Track colours: eight that suit the app's dark theme, names for them, and
//! reading what people (and AIs) type: "orange", "#ff8a3d", "f80".

use crate::model::Track;

/// The colours tracks get unless one is picked, in the order tracks get them.
pub const PALETTE: [(&str, [u8; 3]); 8] = [
    ("blue", [0x5b, 0x8c, 0xff]),
    ("orange", [0xff, 0x8a, 0x3d]),
    ("green", [0x3d, 0xdc, 0x97]),
    ("purple", [0xc8, 0x6b, 0xfa]),
    ("yellow", [0xff, 0xd2, 0x3f]),
    ("cyan", [0x2e, 0xd9, 0xff]),
    ("pink", [0xff, 0x5c, 0x8a]),
    ("violet", [0x9d, 0x8c, 0xff]),
];

/// More names that are understood, besides the palette's own.
const EXTRA: [(&str, [u8; 3]); 5] = [
    ("red", [0xff, 0x4f, 0x5e]),
    ("teal", [0x2e, 0xc4, 0xb6]),
    ("white", [0xe8, 0xea, 0xf2]),
    ("gray", [0x8b, 0x92, 0xab]),
    ("grey", [0x8b, 0x92, 0xab]),
];

/// Every colour name that [`parse`] understands, for messages.
pub fn names() -> String {
    PALETTE
        .iter()
        .chain(&EXTRA)
        .map(|(n, _)| *n)
        .collect::<Vec<_>>()
        .join(", ")
}

/// A colour name or hex code ("#ff8a3d", "ff8a3d", "#f83") as RGB.
pub fn parse(text: &str) -> Option<[u8; 3]> {
    let t = text.trim().to_ascii_lowercase();
    if let Some((_, rgb)) = PALETTE.iter().chain(&EXTRA).find(|(n, _)| *n == t) {
        return Some(*rgb);
    }
    let hex = t.strip_prefix('#').unwrap_or(&t);
    if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let digit = |i: usize| u8::from_str_radix(&hex[i..i + 1], 16).ok();
    match hex.len() {
        6 => {
            let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
            Some([byte(0)?, byte(2)?, byte(4)?])
        }
        3 => Some([digit(0)? * 17, digit(1)? * 17, digit(2)? * 17]),
        _ => None,
    }
}

/// "#rrggbb", the way colours are stored.
pub fn hex([r, g, b]: [u8; 3]) -> String {
    format!("#{r:02x}{g:02x}{b:02x}")
}

/// The name of a colour, when it has one.
pub fn name_of(rgb: [u8; 3]) -> Option<&'static str> {
    PALETTE
        .iter()
        .chain(&EXTRA)
        .find(|(_, c)| *c == rgb)
        .map(|(n, _)| *n)
}

impl Track {
    /// The track's colour: the one picked for it, or else one that follows
    /// from its id ("t1" blue, "t2" orange, ...), so it never changes when
    /// other tracks are added, deleted or moved.
    pub fn rgb(&self) -> [u8; 3] {
        self.color
            .as_deref()
            .and_then(parse)
            .unwrap_or_else(|| self.auto_rgb())
    }

    /// The colour that follows from the track's id, used while none is picked.
    pub fn auto_rgb(&self) -> [u8; 3] {
        let slot = match self
            .id
            .strip_prefix('t')
            .and_then(|n| n.parse::<usize>().ok())
        {
            Some(n) => n.saturating_sub(1),
            // FNV-1a over an unusual id, for some colour that stays put.
            None => self.id.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| {
                (h ^ b as u64).wrapping_mul(0x0100_0000_01b3)
            }) as usize,
        };
        PALETTE[slot % PALETTE.len()].1
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Instrument;

    fn track(id: &str) -> Track {
        Track {
            id: id.into(),
            name: "x".into(),
            instrument: Instrument::default_synth(),
            gain: 0.8,
            pan: 0.0,
            mute: false,
            solo: false,
            reverb: 0.0,
            effects: Vec::new(),
            color: None,
            clips: Vec::new(),
        }
    }

    #[test]
    fn names_and_hex_codes_are_read() {
        assert_eq!(parse("Orange"), Some([0xff, 0x8a, 0x3d]));
        assert_eq!(parse(" #FF8A3D "), Some([0xff, 0x8a, 0x3d]));
        assert_eq!(parse("ff8a3d"), Some([0xff, 0x8a, 0x3d]));
        assert_eq!(parse("#f80"), Some([0xff, 0x88, 0x00]));
        assert_eq!(parse("grey"), parse("gray"));
        for bad in ["", "#12345", "#ggg", "orangey", "#ff8a3d00"] {
            assert_eq!(parse(bad), None, "{bad}");
        }
        assert_eq!(hex([0xff, 0x8a, 0x3d]), "#ff8a3d");
        assert_eq!(name_of([0xff, 0x8a, 0x3d]), Some("orange"));
        assert_eq!(name_of([1, 2, 3]), None);
        assert!(names().starts_with("blue, orange,"));
    }

    #[test]
    fn colours_follow_the_track_not_its_place() {
        // The same colours as before for songs whose tracks were never deleted.
        let t: Vec<Track> = (1..=9).map(|i| track(&format!("t{i}"))).collect();
        for (i, tr) in t.iter().enumerate() {
            assert_eq!(tr.rgb(), PALETTE[i % 8].1);
        }
        // An odd id still gets a palette colour, always the same one.
        let odd = track("drums");
        assert_eq!(odd.rgb(), track("drums").rgb());
        assert!(PALETTE.iter().any(|(_, c)| *c == odd.rgb()));
        // A picked colour wins; one that can't be read is ignored.
        let mut picked = track("t1");
        picked.color = Some("#123456".into());
        assert_eq!(picked.rgb(), [0x12, 0x34, 0x56]);
        assert_eq!(picked.auto_rgb(), PALETTE[0].1);
        picked.color = Some("nonsense".into());
        assert_eq!(picked.rgb(), PALETTE[0].1);
    }
}
