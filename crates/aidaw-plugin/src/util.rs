//! Small conversions between VST3's C types and Rust ones.

use std::ffi::c_char;

use vst3::Steinberg::{char16, TUID};

/// Byte order between a class id in memory and its text form: Windows
/// keeps class ids in COM's GUID layout, whose first three fields are
/// little-endian; the text form (Steinberg's, as in `moduleinfo.json`) is
/// the same on every platform.
fn canonical(mut bytes: [u8; 16]) -> [u8; 16] {
    if cfg!(windows) {
        bytes[0..4].reverse();
        bytes[4..6].reverse();
        bytes[6..8].reverse();
    }
    bytes
}

/// A class id as 32 upper-case hex digits, the way Steinberg writes them
/// (so the same plugin has the same text on every platform).
pub fn uid_to_hex(uid: &TUID) -> String {
    canonical(uid.map(|b| b as u8))
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect()
}

/// Parse 32 hex digits back into a class id.
pub fn hex_to_uid(text: &str) -> Option<TUID> {
    let text = text.trim();
    if text.len() != 32 || !text.is_ascii() {
        return None;
    }
    let mut bytes = [0u8; 16];
    for (i, b) in bytes.iter_mut().enumerate() {
        *b = u8::from_str_radix(&text[2 * i..2 * i + 2], 16).ok()?;
    }
    Some(canonical(bytes).map(|b| b as c_char))
}

/// A NUL-terminated (or full) 8-bit string, as UTF-8 with bad bytes replaced.
pub fn from_c(chars: &[c_char]) -> String {
    let bytes: Vec<u8> = chars
        .iter()
        .take_while(|c| **c != 0)
        .map(|c| *c as u8)
        .collect();
    String::from_utf8_lossy(&bytes).trim().to_string()
}

/// A NUL-terminated (or full) UTF-16 string.
pub fn from_utf16(chars: &[char16]) -> String {
    let len = chars.iter().position(|c| *c == 0).unwrap_or(chars.len());
    String::from_utf16_lossy(&chars[..len]).trim().to_string()
}

/// The bindings' enum constants are C `int`s on Windows and `unsigned`
/// elsewhere, while methods take `int32` and structs hold `uint32`: these
/// convert either, without a cast that is useless on one of the platforms.
pub fn int(k: impl Into<i64>) -> i32 {
    k.into() as i32
}

/// See [`int`].
pub fn uint(k: impl Into<i64>) -> u32 {
    k.into() as u32
}

/// Copy `text` into a fixed UTF-16 buffer, cut to fit and NUL-terminated.
pub fn to_utf16(text: &str, out: &mut [char16]) {
    if out.is_empty() {
        return;
    }
    let mut n = 0;
    for (dst, src) in out.iter_mut().zip(text.encode_utf16()) {
        *dst = src;
        n += 1;
    }
    out[n.min(out.len() - 1)] = 0;
}

/// Read a NUL-terminated C string the plugin handed us.
///
/// # Safety
/// `ptr` must be null or point to a NUL-terminated string.
pub unsafe fn c_str_at(ptr: *const c_char) -> Option<String> {
    if ptr.is_null() {
        return None;
    }
    Some(
        unsafe { std::ffi::CStr::from_ptr(ptr) }
            .to_string_lossy()
            .into_owned(),
    )
}

/// Read a NUL-terminated UTF-16 string the plugin handed us.
///
/// # Safety
/// `ptr` must be null or point to a NUL-terminated UTF-16 string.
pub unsafe fn utf16_at(ptr: *const char16) -> Option<Vec<u16>> {
    if ptr.is_null() {
        return None;
    }
    let mut out = Vec::new();
    let mut p = ptr;
    unsafe {
        while *p != 0 && out.len() < 1 << 20 {
            out.push(*p);
            p = p.add(1);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uids_round_trip() {
        let uid = vst3::uid(0x00112233, 0x44556677, 0x8899AABB, 0xCCDDEEFF);
        let hex = uid_to_hex(&uid);
        assert_eq!(
            hex, "00112233445566778899AABBCCDDEEFF",
            "the same on every platform"
        );
        assert_eq!(hex_to_uid(&hex), Some(uid));
        assert_eq!(hex_to_uid(&hex.to_lowercase()), Some(uid));
        assert_eq!(hex_to_uid("0011"), None);
        assert_eq!(hex_to_uid(&"zz".repeat(16)), None);
    }

    #[test]
    fn strings_convert_and_fit() {
        let mut buf = [0u16; 5];
        to_utf16("AIDAW host", &mut buf);
        assert_eq!(from_utf16(&buf), "AIDA");
        to_utf16("hi", &mut buf);
        assert_eq!(from_utf16(&buf), "hi");
        let c: Vec<c_char> = b"Synth\0junk".iter().map(|b| *b as c_char).collect();
        assert_eq!(from_c(&c), "Synth");
    }
}
