//! Which plugin parameter is meant, and what value: the AI names parameters
//! by name or id and gives values as a knob position (0..1), an option name
//! ("Sine"), or text the way the plugin shows it ("-6 dB", "800 Hz", "30%").

use anyhow::{anyhow, bail, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::instance::{ParamInfo, Plugin};

/// What a plugin can tell about its parameters.
pub trait Knobs {
    /// The parameters, with their current values.
    fn params(&self) -> Vec<ParamInfo>;
    /// How the plugin shows `value` (0..1) of parameter `id`.
    fn display(&self, id: u32, value: f64) -> String;
    /// The value (0..1) the plugin reads `text` as, if it can.
    fn parse(&self, id: u32, text: &str) -> Option<f64>;
}

impl Knobs for Plugin {
    fn params(&self) -> Vec<ParamInfo> {
        Plugin::params(self)
    }

    fn display(&self, id: u32, value: f64) -> String {
        Plugin::display(self, id, value)
    }

    fn parse(&self, id: u32, text: &str) -> Option<f64> {
        Plugin::parse(self, id, text)
    }
}

/// A parameter value worked out from a request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Setting {
    pub id: u32,
    pub name: String,
    /// 0..1.
    pub value: f64,
    /// How the plugin shows it.
    pub display: String,
}

/// Work out every (parameter, value) in `wanted`. Errors name the
/// parameter and say what would have worked.
pub fn resolve(knobs: &dyn Knobs, wanted: &[(String, Value)]) -> Result<Vec<Setting>> {
    let params = knobs.params();
    if params.is_empty() {
        bail!("this plugin has no parameters that can be set from outside");
    }
    wanted
        .iter()
        .map(|(key, v)| {
            let p = find(&params, key)?;
            let value = value_for(knobs, p, v)?;
            Ok(Setting {
                id: p.id,
                name: p.name.clone(),
                value,
                display: knobs.display(p.id, value),
            })
        })
        .collect()
}

/// Lower case letters and digits only: "Filter 1 Cut-off" -> "filter1cutoff".
fn squash(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// The parameter `key` means: its id, its name, or a unique part of it.
pub fn find<'a>(params: &'a [ParamInfo], key: &str) -> Result<&'a ParamInfo> {
    let key = key.trim();
    if let Ok(id) = key.parse::<u32>() {
        if let Some(p) = params.iter().find(|p| p.id == id) {
            return Ok(p);
        }
    }
    if let Some(p) = params.iter().find(|p| p.name.eq_ignore_ascii_case(key)) {
        return Ok(p);
    }
    let want = squash(key);
    if want.is_empty() {
        bail!("a parameter name is empty");
    }
    if let Some(p) = params.iter().find(|p| squash(&p.name) == want) {
        return Ok(p);
    }
    let partial: Vec<&ParamInfo> = params
        .iter()
        .filter(|p| squash(&p.name).contains(&want))
        .collect();
    match partial.as_slice() {
        [one] => Ok(one),
        [] => {
            // Suggest parameters sharing a word with what was asked for.
            let words: Vec<String> = key
                .split(|c: char| !c.is_alphanumeric())
                .filter(|w| w.chars().count() >= 3)
                .map(str::to_lowercase)
                .collect();
            let near: Vec<String> = params
                .iter()
                .filter(|p| {
                    let name = p.name.to_lowercase();
                    words.iter().any(|w| name.contains(w.as_str()))
                })
                .take(8)
                .map(|p| format!("{} {:?}", p.id, p.name))
                .collect();
            if near.is_empty() {
                bail!(
                    "no parameter {key:?} (the plugin has {}; plugin_params lists them)",
                    params.len()
                )
            }
            bail!("no parameter {key:?}; similar: {}", near.join(", "))
        }
        many => bail!(
            "{key:?} could mean {}{}; use the full name or the id",
            many.iter()
                .take(8)
                .map(|p| format!("{} {:?}", p.id, p.name))
                .collect::<Vec<_>>()
                .join(", "),
            if many.len() > 8 { ", ..." } else { "" }
        ),
    }
}

/// The value (0..1) `v` asks for: a knob position from 0 to 1, true/false,
/// an option name, or text as the plugin shows values (other numbers too).
pub fn value_for(knobs: &dyn Knobs, p: &ParamInfo, v: &Value) -> Result<f64> {
    let value = match v {
        Value::Bool(b) => f64::from(u8::from(*b)),
        Value::Number(n) => {
            let x = n.as_f64().unwrap_or(f64::NAN);
            if (0.0..=1.0).contains(&x) {
                x
            } else {
                from_text(knobs, p, &n.to_string())?
            }
        }
        Value::String(s) => match s.trim().parse::<f64>() {
            Ok(x) if (0.0..=1.0).contains(&x) => x,
            _ => from_text(knobs, p, s.trim())?,
        },
        other => bail!(
            "{:?} needs a number from 0 to 1 or text such as {:?} (got {other})",
            p.name,
            p.display
        ),
    };
    Ok(snap(p, value))
}

/// A stepped parameter only takes its steps.
fn snap(p: &ParamInfo, v: f64) -> f64 {
    let v = v.clamp(0.0, 1.0);
    if p.steps > 0 {
        let steps = f64::from(p.steps);
        (v * steps).round() / steps
    } else {
        v
    }
}

fn from_text(knobs: &dyn Knobs, p: &ParamInfo, text: &str) -> Result<f64> {
    if let Some(i) = p
        .options
        .iter()
        .position(|o| o.eq_ignore_ascii_case(text) || squash(o) == squash(text))
    {
        return Ok(i as f64 / f64::from(p.steps.max(1)));
    }
    if let Some(v) = knobs.parse(p.id, text) {
        return Ok(v);
    }
    if let Some(target) = number_in(text) {
        if let Some(v) = search(knobs, p.id, target) {
            return Ok(v);
        }
    }
    let options = if p.options.is_empty() {
        String::new()
    } else {
        format!(" (options: {})", p.options.join(" | "))
    };
    Err(anyhow!(
        "{:?} did not understand {text:?}; it shows values like {:?}{options}. Give that kind of text, or a knob position from 0 to 1",
        p.name,
        p.display
    ))
}

/// The first number in `text`, scaled by a metric prefix before a unit:
/// "-6.0 dB" -> -6, "1.2 kHz" -> 1200, "250 ms" -> 0.25, "30%" -> 30.
pub fn number_in(text: &str) -> Option<f64> {
    let chars: Vec<char> = text.chars().collect();
    let start = chars.iter().enumerate().position(|(i, c)| {
        c.is_ascii_digit()
            || (matches!(c, '-' | '+' | '.')
                && chars
                    .get(i + 1)
                    .is_some_and(|n| n.is_ascii_digit() || *n == '.'))
    })?;
    let mut end = start + 1;
    while end < chars.len() && (chars[end].is_ascii_digit() || chars[end] == '.') {
        end += 1;
    }
    let number: String = chars[start..end].iter().collect();
    let x: f64 = number.parse().ok()?;
    let mut rest = chars[end..].iter().skip_while(|c| c.is_whitespace());
    let prefix = rest.next();
    let unit_follows = rest.next().is_some_and(|c| c.is_alphabetic());
    let scale = match prefix {
        Some('k' | 'K') if unit_follows => 1e3,
        Some('M') if unit_follows => 1e6,
        Some('m') if unit_follows => 1e-3,
        Some('µ' | 'u') if unit_follows => 1e-6,
        _ => 1.0,
    };
    let x = x * scale;
    x.is_finite().then_some(x)
}

/// The knob position whose shown value is closest to `target`, assuming
/// the shown value rises or falls steadily along the knob. Targets beyond
/// either end give that end.
fn search(knobs: &dyn Knobs, id: u32, target: f64) -> Option<f64> {
    let at = |v: f64| number_in(&knobs.display(id, v));
    // "-inf dB" at the very bottom: look just above it.
    let (mut lo, mut hi) = (0.0, 1.0);
    let a = at(lo).or_else(|| {
        lo = 1e-4;
        at(lo)
    })?;
    let b = at(hi).or_else(|| {
        hi = 1.0 - 1e-4;
        at(hi)
    })?;
    if a == b {
        return None;
    }
    let rising = b > a;
    if (target <= a) == rising {
        return Some(lo);
    }
    if (target >= b) == rising {
        return Some(hi);
    }
    for _ in 0..48 {
        let mid = (lo + hi) / 2.0;
        let m = at(mid)?;
        if (m < target) == rising {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    // Shown values are rounded: of the two last positions, take the one
    // that shows the closer value.
    let (shown_lo, shown_hi) = (at(lo)?, at(hi)?);
    Some(if (shown_hi - target).abs() <= (shown_lo - target).abs() {
        hi
    } else {
        lo
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Cutoff 20 Hz .. 20 kHz (exponential, shown as "1.20 kHz"), a
    /// three-way wave switch, and a volume in dB that shows -inf at 0.
    struct Fake;

    const CUTOFF: u32 = 7;
    const WAVE: u32 = 8;
    const VOLUME: u32 = 9;

    fn hz(v: f64) -> f64 {
        20.0 * 1000f64.powf(v)
    }

    impl Knobs for Fake {
        fn params(&self) -> Vec<ParamInfo> {
            let p = |id, name: &str, steps, options: &[&str]| ParamInfo {
                id,
                name: name.into(),
                units: String::new(),
                value: 0.5,
                display: self.display(id, 0.5),
                default: 0.5,
                steps,
                options: options.iter().map(|s| s.to_string()).collect(),
                bypass: false,
            };
            vec![
                p(CUTOFF, "Filter Cutoff", 0, &[]),
                p(WAVE, "Osc Wave", 2, &["Sine", "Saw", "Square"]),
                p(VOLUME, "Volume", 0, &[]),
                p(10, "Filter Resonance", 0, &[]),
            ]
        }

        fn display(&self, id: u32, v: f64) -> String {
            match id {
                CUTOFF if hz(v) >= 1000.0 => format!("{:.2} kHz", hz(v) / 1000.0),
                CUTOFF => format!("{:.1} Hz", hz(v)),
                WAVE => ["Sine", "Saw", "Square"][(v * 2.0).round() as usize].into(),
                VOLUME if v == 0.0 => "-inf dB".into(),
                VOLUME => format!("{:.1} dB", 20.0 * v.log10()),
                _ => format!("{:.0} %", v * 100.0),
            }
        }

        fn parse(&self, id: u32, text: &str) -> Option<f64> {
            // Like many plugins: only its own volume field reads text.
            (id == VOLUME)
                .then(|| number_in(text))
                .flatten()
                .map(|db| 10f64.powf(db / 20.0).clamp(0.0, 1.0))
        }
    }

    fn one(key: &str, v: Value) -> Result<Setting> {
        Ok(resolve(&Fake, &[(key.into(), v)])?.remove(0))
    }

    #[test]
    fn numbers_with_units_are_read_like_people_write_them() {
        assert_eq!(number_in("-6.0 dB"), Some(-6.0));
        assert_eq!(number_in("1.2 kHz"), Some(1200.0));
        assert_eq!(number_in("250 ms"), Some(0.25));
        assert_eq!(number_in("30%"), Some(30.0));
        assert_eq!(number_in("Mix +3"), Some(3.0));
        assert_eq!(number_in("-inf dB"), None);
        assert_eq!(number_in("Sine"), None);
    }

    #[test]
    fn parameters_are_found_by_id_name_or_a_unique_part() {
        let params = Fake.params();
        assert_eq!(find(&params, "7").unwrap().id, CUTOFF);
        assert_eq!(find(&params, "osc wave").unwrap().id, WAVE);
        assert_eq!(find(&params, "cutoff").unwrap().id, CUTOFF);
        assert_eq!(find(&params, "Filter-Resonance").unwrap().id, 10);
        let many = find(&params, "filter").unwrap_err().to_string();
        assert!(many.contains("could mean"), "{many}");
        let none = find(&params, "Wave Shape").unwrap_err().to_string();
        assert!(
            none.contains("similar") && none.contains("Osc Wave"),
            "{none}"
        );
    }

    #[test]
    fn values_come_as_positions_options_or_text() {
        assert_eq!(one("Volume", json!(0.25)).unwrap().value, 0.25);
        assert_eq!(one("Volume", json!("0.25")).unwrap().value, 0.25);
        assert_eq!(one("Osc Wave", json!("square")).unwrap().value, 1.0);
        assert_eq!(one("Osc Wave", json!(0.4)).unwrap().display, "Saw");
        assert_eq!(one("Osc Wave", json!(true)).unwrap().value, 1.0);
        // The plugin reads this one itself.
        let v = one("Volume", json!("-6 dB")).unwrap();
        assert!((v.value - 0.501).abs() < 1e-3, "{v:?}");
        // This one it does not: found by trying positions along the knob.
        let c = one("cutoff", json!("800 Hz")).unwrap();
        assert!((hz(c.value) - 800.0).abs() < 1.0, "{c:?}");
        let c = one("cutoff", json!(1500)).unwrap();
        assert_eq!(c.display, "1.50 kHz");
        // Beyond the ends: the end.
        assert_eq!(one("cutoff", json!("50 kHz")).unwrap().value, 1.0);
        assert_eq!(one("cutoff", json!("1 Hz")).unwrap().value, 0.0);
        let err = one("Osc Wave", json!("Triangle")).unwrap_err().to_string();
        assert!(err.contains("Sine | Saw | Square"), "{err}");
        assert!(one("Volume", json!([1])).is_err());
    }
}
