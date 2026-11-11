//! The project file's side of plugin hosting: which plugin a track plays or
//! runs its sound through, and the plugin's saved settings. Loading and
//! running the plugins themselves is the `aidaw-plugin` crate's job; the
//! core only describes them and drives them through [`crate::render::Rack`].

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use anyhow::{bail, Result};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A VST3 plugin chosen for a track, and how it was set up.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PluginRef {
    /// The name the plugin gives itself, e.g. "Surge XT".
    pub name: String,
    /// The plugin's class id, 32 hex digits. It finds the plugin again on
    /// any computer, wherever it is installed.
    pub uid: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub vendor: String,
    /// Where the plugin was installed when it was picked. Only a hint: a
    /// plugin that moved is found again by `uid`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub path: String,
    /// The plugin's own saved settings, restored when the song is opened.
    /// Written by the AIDAW editor, which reads it from the running plugin.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<Blob>,
    /// Settings of the plugin's editor half, for plugins that keep them apart.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub controller_state: Option<Blob>,
    /// Parameters set on top of `state` (by the AI, which cannot run the
    /// plugin's editor): parameter id (decimal) -> value from 0 to 1. The
    /// editor folds them into `state` the next time it saves the plugin.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub params: BTreeMap<String, f64>,
}

impl PluginRef {
    /// A plugin with nothing set up yet (it starts with its own defaults).
    pub fn new(name: impl Into<String>, uid: impl Into<String>) -> PluginRef {
        PluginRef {
            name: name.into(),
            uid: uid.into(),
            vendor: String::new(),
            path: String::new(),
            state: None,
            controller_state: None,
            params: BTreeMap::new(),
        }
    }

    /// Refuse what cannot be a plugin: an empty name or a malformed id.
    pub fn check(&self) -> Result<()> {
        if self.name.trim().is_empty() {
            bail!("a plugin needs a name");
        }
        if !is_uid(&self.uid) {
            bail!(
                "plugin uid must be 32 hex digits (got {:?}); list_plugins shows each plugin's uid",
                self.uid
            );
        }
        for (id, v) in &self.params {
            if id.parse::<u32>().is_err() {
                bail!("plugin parameter ids are whole numbers (got {id:?})");
            }
            if !(v.is_finite() && (0.0..=1.0).contains(v)) {
                bail!("plugin parameter values go from 0 to 1 (got {v} for {id})");
            }
        }
        Ok(())
    }

    /// The parameter overrides as (id, value) pairs, skipping malformed ones.
    pub fn param_values(&self) -> impl Iterator<Item = (u32, f64)> + '_ {
        self.params.iter().filter_map(|(id, v)| {
            let id = id.parse().ok()?;
            v.is_finite().then_some((id, v.clamp(0.0, 1.0)))
        })
    }

    /// Whether `other` is the same plugin (the settings may differ).
    pub fn same_plugin(&self, other: &PluginRef) -> bool {
        self.uid.eq_ignore_ascii_case(&other.uid)
    }
}

/// Whether `s` looks like a VST3 class id: 32 hex digits.
pub fn is_uid(s: &str) -> bool {
    s.len() == 32 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// One stage in a track's effect chain: the sound runs through these in
/// order, after the instrument and before the track's volume, pan and
/// reverb send.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Effect {
    /// Unique in the project, e.g. "fx1".
    pub id: String,
    /// Skipped (the sound passes through unchanged) while true.
    #[serde(default, skip_serializing_if = "is_false")]
    pub bypass: bool,
    #[serde(flatten)]
    pub kind: EffectKind,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EffectKind {
    /// A VST3 effect plugin.
    Vst3(PluginRef),
}

impl Effect {
    pub fn plugin(&self) -> &PluginRef {
        match &self.kind {
            EffectKind::Vst3(p) => p,
        }
    }

    pub fn plugin_mut(&mut self) -> &mut PluginRef {
        match &mut self.kind {
            EffectKind::Vst3(p) => p,
        }
    }

    pub fn name(&self) -> &str {
        &self.plugin().name
    }
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// Bytes kept as base64 text in the project file. Cloning only bumps a
/// reference count, so the undo history of a song with big plugin settings
/// costs no extra memory.
#[derive(Clone, PartialEq, Eq)]
pub struct Blob(Arc<[u8]>);

impl Blob {
    pub fn new(bytes: impl Into<Arc<[u8]>>) -> Blob {
        Blob(bytes.into())
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn to_base64(&self) -> String {
        base64_encode(&self.0)
    }

    pub fn from_base64(text: &str) -> Result<Blob> {
        Ok(Blob::new(base64_decode(text)?))
    }
}

impl fmt::Debug for Blob {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Blob({} bytes)", self.0.len())
    }
}

impl Serialize for Blob {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_base64())
    }
}

impl<'de> Deserialize<'de> for Blob {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Blob, D::Error> {
        let text = String::deserialize(d)?;
        Blob::from_base64(&text).map_err(serde::de::Error::custom)
    }
}

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard base64 (RFC 4648) with `=` padding.
pub fn base64_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk.len();
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let v = u32::from(b[0]) << 16 | u32::from(b[1]) << 8 | u32::from(b[2]);
        for i in 0..4 {
            if i <= n {
                out.push(ALPHABET[(v >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Decode standard base64; whitespace is ignored, padding is optional.
pub fn base64_decode(text: &str) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    let mut acc = 0u32;
    let mut bits = 0;
    let mut padding = false;
    for c in text.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' => {
                padding = true;
                continue;
            }
            c if c.is_ascii_whitespace() => continue,
            c => bail!("not base64: unexpected character {:?}", c as char),
        };
        if padding {
            bail!("not base64: data after the '=' padding");
        }
        acc = acc << 6 | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    if bits >= 6 {
        bail!("not base64: the text is cut off");
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_round_trips_every_length() {
        let data: Vec<u8> = (0..=255u8)
            .chain((0..40u32).map(|i| (i * 37 % 256) as u8))
            .collect();
        for n in 0..data.len() {
            let text = base64_encode(&data[..n]);
            assert_eq!(text.len() % 4, 0);
            assert_eq!(base64_decode(&text).unwrap(), &data[..n], "length {n}");
        }
        assert_eq!(base64_encode(b"Man"), "TWFu");
        assert_eq!(base64_encode(b"Ma"), "TWE=");
        assert_eq!(base64_encode(b"M"), "TQ==");
        assert_eq!(base64_decode("TW E=\n").unwrap(), b"Ma");
        assert_eq!(base64_decode("TWE").unwrap(), b"Ma");
        assert!(base64_decode("TW*=").is_err());
        assert!(base64_decode("TQ==TQ==").is_err());
        assert!(base64_decode("T").is_err());
    }

    #[test]
    fn plugin_refs_are_checked() {
        let ok = PluginRef::new("Synth", "0123456789abcdef0123456789ABCDEF");
        assert!(ok.check().is_ok());
        assert!(PluginRef::new("", &ok.uid).check().is_err());
        assert!(PluginRef::new("Synth", "1234").check().is_err());
        let mut bad = ok.clone();
        bad.params.insert("7".into(), 1.5);
        assert!(bad.check().is_err());
        bad.params.insert("7".into(), 0.5);
        assert!(bad.check().is_ok());
        assert_eq!(bad.param_values().collect::<Vec<_>>(), vec![(7, 0.5)]);
        bad.params.insert("x".into(), 0.5);
        assert!(bad.check().is_err());
    }

    #[test]
    fn effects_serialise_flat_with_their_type() {
        let mut p = PluginRef::new("Echo", "0123456789ABCDEF0123456789ABCDEF");
        p.state = Some(Blob::new(vec![1u8, 2, 3]));
        p.params.insert("3".into(), 0.25);
        let fx = Effect {
            id: "fx1".into(),
            bypass: false,
            kind: EffectKind::Vst3(p),
        };
        let json = serde_json::to_string(&fx).unwrap();
        assert_eq!(
            json,
            r#"{"id":"fx1","type":"vst3","name":"Echo","uid":"0123456789ABCDEF0123456789ABCDEF","state":"AQID","params":{"3":0.25}}"#
        );
        let back: Effect = serde_json::from_str(&json).unwrap();
        assert_eq!(back, fx);
        assert!(serde_json::from_str::<Effect>(r#"{"id":"fx1","type":"eq"}"#).is_err());
    }

    #[test]
    fn plugin_instruments_and_tracks_round_trip() {
        use crate::model::{Instrument, Project};
        let json = r#"{"type":"vst3","name":"Synth","uid":"0123456789ABCDEF0123456789ABCDEF","vendor":"Acme","path":"C:\\VST3\\Synth.vst3","state":"AAEC","controller_state":"/w==","params":{"12":1.0}}"#;
        let inst: Instrument = serde_json::from_str(json).unwrap();
        let p = inst.plugin().unwrap();
        assert_eq!(p.state.as_ref().unwrap().as_bytes(), &[0, 1, 2]);
        assert_eq!(p.controller_state.as_ref().unwrap().as_bytes(), &[255]);
        assert_eq!(p.param_values().collect::<Vec<_>>(), vec![(12, 1.0)]);
        assert_eq!(serde_json::to_string(&inst).unwrap(), json);

        let mut song = Project::new("x", 100.0);
        let mut t = serde_json::from_str::<crate::Track>(
            r#"{"id":"t1","name":"Lead","instrument":{"type":"drums"}}"#,
        )
        .unwrap();
        assert!(t.effects.is_empty());
        t.instrument = inst;
        t.effects.push(Effect {
            id: "fx1".into(),
            bypass: true,
            kind: EffectKind::Vst3(PluginRef::new("Echo", "FFEEDDCCBBAA99887766554433221100")),
        });
        song.tracks.push(t);
        let text = serde_json::to_string(&song).unwrap();
        assert!(
            text.contains(r#""effects":[{"id":"fx1","bypass":true,"type":"vst3""#),
            "{text}"
        );
        assert_eq!(serde_json::from_str::<Project>(&text).unwrap(), song);
        assert!(song.uses_plugins());
        assert_eq!(song.next_id("fx"), "fx2");
        assert_eq!(
            song.effect("fx1").map(|(t, e)| (t.id.as_str(), e.name())),
            Some(("t1", "Echo"))
        );
    }
}
