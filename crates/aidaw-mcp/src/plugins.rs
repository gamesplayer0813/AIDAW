//! VST3 plugins for the AI: what is installed, which plugin a name means,
//! a plugin's parameters, and rendering songs that use plugins. Plugins
//! only ever load in worker processes (this program started again with
//! `--aidaw-plugin-worker`), so one that crashes, hangs or prints to stdout
//! cannot break the MCP connection.
//!
//! VST is a registered trademark of Steinberg Media Technologies GmbH.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use aidaw_core::analysis::Analysis;
use aidaw_core::{PluginRef, Project, Track};
use aidaw_plugin::instance::ParamInfo;
use aidaw_plugin::params::Setting;
use aidaw_plugin::scan::{Kind, PluginInfo, ScanReport, Store};
use aidaw_plugin::worker::{Job, Worker};
use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};

use crate::session::{find_track, fmt_num, norm, Args};

/// How long a worker may stay silent while it loads a plugin (big sample
/// libraries take a while).
const LOAD_QUIET: Duration = Duration::from_secs(180);
/// ... while it renders or measures a song.
const RENDER_QUIET: Duration = Duration::from_secs(900);
/// Most plugins one list_plugins answer shows.
const MAX_LISTED: usize = 200;
/// Most parameters one plugin_params answer shows.
const MAX_PARAMS: usize = 120;

/// Starts plugin workers and reads the list of installed plugins (shared
/// with the AIDAW app, which scans the same `plugins.json`).
#[derive(Default)]
pub struct Plugins {
    worker: Option<Worker>,
}

impl Plugins {
    fn worker(&mut self) -> Result<&Worker> {
        if self.worker.is_none() {
            self.worker = Some(Worker::current_exe()?);
        }
        Ok(self.worker.as_ref().expect("just set"))
    }

    fn run(&mut self, job: &Job, quiet: Duration) -> Result<Value> {
        self.worker()?.run(job, quiet, &mut |_| {})
    }

    /// The installed plugins, after looking for new and updated ones (at
    /// every plugin again when `force`).
    fn scan(&mut self, force: bool) -> Result<(Store, ScanReport)> {
        let mut store = Store::load();
        let report = store.scan(self.worker()?, force, &mut |_, _, _| {});
        Ok((store, report))
    }

    /// The plugin `name` means (its name, a unique part of it, or its uid),
    /// looking for newly installed plugins when it is not known yet.
    pub fn find(&mut self, name: &str, kind: Kind) -> Result<PluginInfo> {
        if let Ok(p) = Store::load().resolve(name, Some(kind)) {
            return Ok(p);
        }
        let (store, _) = self.scan(false)?;
        store.resolve(name, Some(kind)).map_err(|e| {
            let other = match kind {
                Kind::Instrument => Kind::Effect,
                Kind::Effect => Kind::Instrument,
            };
            match store.resolve(name, Some(other)) {
                Ok(p) if kind == Kind::Instrument => anyhow!(
                    "{} is an effect, not an instrument: put it on a track with add_effect {{\"track\": \"...\", \"plugin\": \"{}\"}}",
                    p.name,
                    p.name
                ),
                Ok(p) => anyhow!(
                    "{} is an instrument, not an effect: let a track play it with add_track {{\"name\": \"...\", \"plugin\": \"{}\"}}",
                    p.name,
                    p.name
                ),
                Err(_) => e,
            }
        })
    }

    /// `project` with each plugin's `path` set to where it is installed on
    /// this computer, or emptied when it is not (the worker then reports
    /// it as missing and renders without it).
    fn located(&mut self, project: &Project) -> Result<Project> {
        let mut out = project.clone();
        if Store::load().locate_all(&mut out) {
            // Something is missing: maybe it was installed since the last scan.
            let (store, _) = self.scan(false)?;
            out = project.clone();
            store.locate_all(&mut out);
        }
        Ok(out)
    }

    /// The list_plugins tool.
    pub fn list(&mut self, args: &Args) -> Result<String> {
        let force = args.get("rescan").and_then(Value::as_bool) == Some(true);
        let kind = match args
            .get("kind")
            .and_then(Value::as_str)
            .map(norm)
            .as_deref()
        {
            None | Some("" | "all" | "any" | "both") => None,
            Some(k) if k.starts_with("instrument") || k == "synth" || k == "vsti" => {
                Some(Kind::Instrument)
            }
            Some(k) if k.starts_with("effect") || k == "fx" => Some(Kind::Effect),
            Some(k) => bail!("kind is \"instrument\" or \"effect\" (got {k:?})"),
        };
        let search = args
            .get("search")
            .and_then(Value::as_str)
            .map(str::to_lowercase)
            .filter(|s| !s.trim().is_empty());
        let (store, report) = self.scan(force)?;
        let all = store.plugins();
        let mut out = String::new();
        if all.is_empty() {
            let folders: Vec<String> = store
                .all_folders()
                .iter()
                .map(|f| f.display().to_string())
                .collect();
            out += &format!(
                "No VST3 plugins are installed (AIDAW looks in {}). Install some there, or add their folder in \
                 the AIDAW app (Plugins window) or with the AIDAW_VST3_PATH environment variable, then call \
                 list_plugins again. The built-in presets (list_presets) always work.",
                folders.join(", ")
            );
        } else {
            let shown: Vec<&PluginInfo> = all
                .iter()
                .filter(|p| kind.is_none_or(|k| p.kind == k))
                .filter(|p| {
                    search.as_deref().is_none_or(|s| {
                        [&p.name, &p.vendor, &p.category]
                            .iter()
                            .any(|f| f.to_lowercase().contains(s))
                    })
                })
                .collect();
            let instruments = all.iter().filter(|p| p.kind == Kind::Instrument).count();
            out += &format!(
                "{} installed ({}, {})",
                count(all.len(), "VST3 plugin"),
                count(instruments, "instrument"),
                count(all.len() - instruments, "effect")
            );
            if shown.len() != all.len() {
                out += &format!(", {} shown", shown.len());
            }
            out += ":";
            let mut heading = None;
            for p in shown.iter().take(MAX_LISTED) {
                if heading != Some(p.kind) {
                    heading = Some(p.kind);
                    out += match p.kind {
                        Kind::Instrument => {
                            "\n[instruments: add_track {\"name\": ..., \"plugin\": ...}]"
                        }
                        Kind::Effect => "\n[effects: add_effect {\"track\": ..., \"plugin\": ...}]",
                    };
                }
                out += &format!("\n- {}", p.name);
                let about: Vec<&str> = [p.vendor.as_str(), p.category.as_str()]
                    .into_iter()
                    .filter(|s| !s.is_empty())
                    .collect();
                if !about.is_empty() {
                    out += &format!(" ({})", about.join(", "));
                }
                // Two plugins with one name are told apart by uid.
                if all.iter().filter(|q| q.name == p.name).count() > 1 {
                    out += &format!(" uid {}", p.uid);
                }
            }
            if shown.len() > MAX_LISTED {
                out += &format!(
                    "\n... and {} more; narrow it down with \"kind\" or \"search\"",
                    shown.len() - MAX_LISTED
                );
            }
            out += "\nExample: add_track {\"name\": \"Keys\", \"plugin\": \"<instrument name>\"}, then \
                    plugin_params {\"track\": \"Keys\"} to see its knobs.";
        }
        if report.scanned > 0 {
            out += &format!(
                "\n(looked at {})",
                count(report.scanned, "new or updated plugin file")
            );
        }
        if !report.problems.is_empty() {
            let shown: Vec<String> = report
                .problems
                .iter()
                .take(10)
                .map(|(path, why)| format!("{} ({why})", file_name(path)))
                .collect();
            out += &format!(
                "\nCould not use {} plugin file(s): {}",
                report.problems.len(),
                shown.join("; ")
            );
        }
        Ok(out)
    }

    /// The plugin_params tool.
    pub fn params(&mut self, project: &Project, args: &Args) -> Result<String> {
        let target = Target::find(project, args)?;
        let search = args
            .get("search")
            .and_then(Value::as_str)
            .map(norm)
            .filter(|s| !s.is_empty());
        let mut plugin = target.plugin.clone();
        plugin.path = self.path_of(&plugin)?;
        let done = self.run(
            &Job::Params {
                plugin: Box::new(plugin),
            },
            LOAD_QUIET,
        )?;
        let params: Vec<ParamInfo> = serde_json::from_value(done["params"].clone())?;
        let shown: Vec<&ParamInfo> = params
            .iter()
            .filter(|p| search.as_deref().is_none_or(|s| norm(&p.name).contains(s)))
            .collect();
        let mut out = format!(
            "{} ({}): {}",
            target.plugin.name,
            target.label,
            count(params.len(), "parameter")
        );
        if let Some(s) = &search {
            out += &format!(", {} matching {s:?}", shown.len());
        }
        out += ". id \"name\" = knob position 0-1 (as the plugin shows it):";
        for p in shown.iter().take(MAX_PARAMS) {
            let display = if p.units.is_empty() || p.display.contains(&p.units) {
                p.display.clone()
            } else {
                format!("{} {}", p.display, p.units)
            };
            out += &format!(
                "\n- {} \"{}\" = {} ({display})",
                p.id,
                p.name,
                fmt_num(p.value)
            );
            if !p.options.is_empty() {
                out += &format!(" options: {}", p.options.join(" | "));
            }
            if target.plugin.params.contains_key(&p.id.to_string()) {
                out += " [set with set_plugin_params]";
            }
            if p.bypass {
                out += " [the plugin's own bypass]";
            }
        }
        if shown.len() > MAX_PARAMS {
            out += &format!(
                "\n... and {} more; narrow it down with \"search\"",
                shown.len() - MAX_PARAMS
            );
        }
        if params.is_empty() {
            out += "\n(none: this plugin can only be set up in its own window, in the AIDAW app)";
        } else {
            out += &format!(
                "\nChange them with set_plugin_params {{\"track\": \"{}\"{}, \"param\": \"<name or id>\", \"value\": \"<0-1, or text like the plugin shows>\"}}",
                target.track_name,
                target
                    .effect_id
                    .as_ref()
                    .map(|e| format!(", \"effect\": \"{e}\""))
                    .unwrap_or_default()
            );
        }
        Ok(out)
    }

    /// Before set_plugin_params edits the song: turn parameter names and
    /// values given as text into ids and knob positions, which takes the
    /// plugin itself. `args` ends up with `params` as {id: 0..1}. Returns
    /// what each value means, or `None` when the call already used ids and
    /// knob positions only (no plugin needs loading then).
    pub fn prepare_params(
        &mut self,
        project: &Project,
        args: &mut Args,
    ) -> Result<Option<Vec<Setting>>> {
        let wanted = wanted_params(args)?;
        if let Some(plain) = plain_params(&wanted) {
            args.insert("params".into(), json!(plain));
            return Ok(None);
        }
        let target = Target::find(project, args)?;
        let mut plugin = target.plugin.clone();
        plugin.path = self.path_of(&plugin)?;
        let done = self.run(
            &Job::SetParams {
                plugin: Box::new(plugin),
                wanted,
            },
            LOAD_QUIET,
        )?;
        let set: Vec<Setting> = serde_json::from_value(done["set"].clone())?;
        let params: BTreeMap<String, f64> =
            set.iter().map(|s| (s.id.to_string(), s.value)).collect();
        args.insert("params".into(), json!(params));
        Ok(Some(set))
    }

    /// Where `plugin` is installed, scanning once if it is not known.
    fn path_of(&mut self, plugin: &PluginRef) -> Result<String> {
        let found = match Store::load().locate(plugin) {
            Ok(path) => path,
            Err(_) => self.scan(false)?.0.locate(plugin)?,
        };
        Ok(found.display().to_string())
    }

    /// render_wav for a song with plugins.
    pub fn render(&mut self, project: &Project, out: &Path, sample_rate: u32) -> Result<String> {
        let job = Job::Render {
            project: Box::new(self.located(project)?),
            out: out.display().to_string(),
            sample_rate,
        };
        let done = self.run(&job, RENDER_QUIET)?;
        let seconds = done["seconds"].as_f64().unwrap_or(0.0);
        Ok(format!(
            "rendered {seconds:.1}s to {}{}",
            out.display(),
            problems_note(&done["problems"])
        ))
    }

    /// analyze for a song with plugins: the measurements, and what went
    /// wrong with which plugin.
    pub fn analyze(&mut self, project: &Project) -> Result<(Analysis, String)> {
        let job = Job::Analyze {
            project: Box::new(self.located(project)?),
        };
        let done = self.run(&job, RENDER_QUIET)?;
        let analysis = serde_json::from_value(done["analysis"].clone())
            .map_err(|e| anyhow!("the plugin worker sent a broken analysis: {e}"))?;
        Ok((analysis, problems_note(&done["problems"])))
    }
}

/// The installed instrument plugin called exactly `name` (case aside), if
/// a scan found one before. Nothing is scanned: this only lets
/// `"instrument": "Surge XT"` mean the plugin.
pub fn known_instrument(name: &str) -> Option<PluginInfo> {
    let name = name.trim();
    Store::load()
        .plugins()
        .into_iter()
        .find(|p| p.kind == Kind::Instrument && p.name.eq_ignore_ascii_case(name))
}

/// "\nwarning: ..." for plugins that could not play, or nothing.
fn problems_note(problems: &Value) -> String {
    let list: Vec<&str> = problems
        .as_array()
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    if list.is_empty() {
        String::new()
    } else {
        format!(
            "\nwarning: left out, so those tracks are silent or dry: {}",
            list.join("; ")
        )
    }
}

fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

/// A plugin of the song that a tool works on: a track's instrument, or one
/// of its effects (`effect`).
struct Target<'a> {
    plugin: &'a PluginRef,
    track_name: String,
    effect_id: Option<String>,
    /// "instrument of track t2 \"Keys\"" or "effect fx1 on track t2 \"Keys\"".
    label: String,
}

impl<'a> Target<'a> {
    fn find(project: &'a Project, args: &Args) -> Result<Target<'a>> {
        let q = ["track", "track_id", "track_name"]
            .iter()
            .find_map(|k| args.get(*k))
            .and_then(|v| match v {
                Value::String(s) => Some(s.clone()),
                Value::Number(n) => Some(n.to_string()),
                _ => None,
            })
            .ok_or_else(|| anyhow!("missing \"track\" (the track whose plugin you mean)"))?;
        let track = find_track(project, &q)?;
        let at = format!("track {} \"{}\"", track.id, track.name);
        let effect = ["effect", "effect_id"]
            .iter()
            .find_map(|k| args.get(*k))
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty());
        match effect {
            Some(q) => {
                let fx = find_effect_on(track, q)?;
                Ok(Target {
                    plugin: fx.plugin(),
                    track_name: track.name.clone(),
                    effect_id: Some(fx.id.clone()),
                    label: format!("effect {} on {at}", fx.id),
                })
            }
            None => match track.instrument.plugin() {
                Some(p) => Ok(Target {
                    plugin: p,
                    track_name: track.name.clone(),
                    effect_id: None,
                    label: format!("instrument of {at}"),
                }),
                None if track.effects.is_empty() => bail!(
                    "{at} plays a built-in instrument, not a plugin, and has no effects; \
                     change its sound with update_track instead"
                ),
                None => bail!(
                    "{at} plays a built-in instrument; to set one of its effects, add \"effect\": {}",
                    effect_names(track)
                ),
            },
        }
    }
}

/// `fx1 "Supermassive", fx2 "Pro-Q 3"`.
pub fn effect_names(track: &Track) -> String {
    let names: Vec<String> = track
        .effects
        .iter()
        .map(|e| format!("{} \"{}\"", e.id, e.name()))
        .collect();
    names.join(", ")
}

/// The effect on `track` that `q` means: its id, its plugin's name, or a
/// unique part of that name.
pub fn find_effect_on<'a>(track: &'a Track, q: &str) -> Result<&'a aidaw_core::Effect> {
    let q = q.trim();
    if track.effects.is_empty() {
        bail!(
            "track {} \"{}\" has no effects; add one with add_effect",
            track.id,
            track.name
        );
    }
    if let Some(e) = track.effects.iter().find(|e| e.id == q) {
        return Ok(e);
    }
    if let Some(e) = track
        .effects
        .iter()
        .find(|e| e.name().eq_ignore_ascii_case(q))
    {
        return Ok(e);
    }
    let n = norm(q);
    let partial: Vec<_> = track
        .effects
        .iter()
        .filter(|e| !n.is_empty() && norm(e.name()).contains(&n))
        .collect();
    match partial.as_slice() {
        [one] => Ok(one),
        [] => bail!(
            "track \"{}\" has no effect {q:?}; its effects: {}",
            track.name,
            effect_names(track)
        ),
        _ => bail!(
            "{q:?} could be several effects on track \"{}\" ({}); use the id",
            track.name,
            effect_names(track)
        ),
    }
}

/// Which effect `q` means, anywhere in the song or on `track` (a track
/// name or id). Returns (track id, effect id).
pub fn find_effect(project: &Project, q: &str, track: Option<&str>) -> Result<(String, String)> {
    if let Some(t) = track {
        let t = find_track(project, t)?;
        let e = find_effect_on(t, q)?;
        return Ok((t.id.clone(), e.id.clone()));
    }
    if let Some((t, e)) = project.effect(q.trim()) {
        return Ok((t.id.clone(), e.id.clone()));
    }
    let with_effects: Vec<&Track> = project
        .tracks
        .iter()
        .filter(|t| !t.effects.is_empty())
        .collect();
    let found: Vec<(&Track, &aidaw_core::Effect)> = with_effects
        .iter()
        .filter_map(|t| find_effect_on(t, q).ok().map(|e| (*t, e)))
        .collect();
    match found.as_slice() {
        [(t, e)] => Ok((t.id.clone(), e.id.clone())),
        [] if with_effects.is_empty() => {
            bail!("the song has no effects yet; add one with add_effect")
        }
        [] => bail!(
            "no effect {q:?}; effects in the song: {}",
            with_effects
                .iter()
                .map(|t| format!("on \"{}\": {}", t.name, effect_names(t)))
                .collect::<Vec<_>>()
                .join("; ")
        ),
        _ => bail!("{q:?} is on several tracks; add \"track\" or use the effect id"),
    }
}

/// The (parameter, value) pairs a set_plugin_params call asks for, from
/// `params` (a list of {param, value} or an object) and `param` + `value`.
/// Takes them out of `args`.
pub fn wanted_params(args: &mut Args) -> Result<Vec<(String, Value)>> {
    let mut out = Vec::new();
    let key = |v: &Value| match v {
        Value::String(s) => Some(s.trim().to_string()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    };
    match args.remove("params") {
        None | Some(Value::Null) => {}
        Some(Value::Object(m)) => out.extend(m),
        Some(Value::Array(items)) => {
            for item in items {
                let Value::Object(m) = &item else {
                    bail!("each entry of params is {{\"param\": \"<name or id>\", \"value\": ...}} (got {item})")
                };
                let name = ["param", "name", "id"]
                    .iter()
                    .find_map(|k| m.get(*k).and_then(key))
                    .ok_or_else(|| anyhow!("an entry of params has no \"param\": {item}"))?;
                let value = m
                    .get("value")
                    .cloned()
                    .ok_or_else(|| anyhow!("an entry of params has no \"value\": {item}"))?;
                out.push((name, value));
            }
        }
        Some(other) => bail!(
            "params is a list like [{{\"param\": \"Cutoff\", \"value\": \"800 Hz\"}}] (got {other})"
        ),
    }
    let one = args.remove("param");
    let value = args.remove("value");
    match (one, value) {
        (None, None) => {}
        (Some(p), Some(v)) => {
            let name =
                key(&p).ok_or_else(|| anyhow!("param is a parameter name or id (got {p})"))?;
            out.push((name, v));
        }
        (Some(_), None) => bail!("\"param\" needs a \"value\" (0-1, or text like \"-6 dB\")"),
        (None, Some(_)) => bail!("\"value\" needs a \"param\" (a parameter name or id)"),
    }
    if out.is_empty() {
        bail!("say which parameter to set: {{\"param\": \"Cutoff\", \"value\": 0.4}}");
    }
    Ok(out)
}

/// The request as {id: knob position}, if it only uses parameter ids and
/// positions from 0 to 1 (so the plugin need not be asked).
pub fn plain_params(wanted: &[(String, Value)]) -> Option<BTreeMap<String, f64>> {
    wanted
        .iter()
        .map(|(k, v)| {
            let id: u32 = k.trim().parse().ok()?;
            let x = match v {
                Value::Number(n) => n.as_f64()?,
                Value::Bool(b) => f64::from(u8::from(*b)),
                _ => return None,
            };
            (0.0..=1.0).contains(&x).then(|| (id.to_string(), x))
        })
        .collect()
}

/// The project as pretty JSON, with plugins' own saved settings (long base64
/// text nobody can read) replaced by their size.
pub fn readable_json(p: &Project) -> Result<String> {
    fn walk(v: &mut Value) {
        match v {
            Value::Object(m) => {
                for (k, x) in m.iter_mut() {
                    if matches!(k.as_str(), "state" | "controller_state") {
                        if let Value::String(s) = x {
                            *x = json!(format!(
                                "({} bytes of the plugin's own settings, not shown)",
                                s.len() / 4 * 3
                            ));
                            continue;
                        }
                    }
                    walk(x);
                }
            }
            Value::Array(items) => items.iter_mut().for_each(walk),
            _ => {}
        }
    }
    let mut v = serde_json::to_value(p)?;
    walk(&mut v);
    Ok(serde_json::to_string_pretty(&v)?)
}

/// "1 effect", "2 effects".
pub fn count(n: usize, thing: &str) -> String {
    if n == 1 {
        format!("1 {thing}")
    } else {
        format!("{n} {thing}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parameter_requests_come_in_three_shapes() {
        let mut args: Args = serde_json::from_value(json!({
            "params": [{"param": "Cutoff", "value": "800 Hz"}, {"param": 3, "value": 0.5}],
            "param": "Mix", "value": 0.2
        }))
        .unwrap();
        let w = wanted_params(&mut args).unwrap();
        assert_eq!(
            w,
            vec![
                ("Cutoff".into(), json!("800 Hz")),
                ("3".into(), json!(0.5)),
                ("Mix".into(), json!(0.2))
            ]
        );
        assert!(args.is_empty());
        let mut args: Args = serde_json::from_value(json!({"params": {"7": 0.25}})).unwrap();
        let w = wanted_params(&mut args).unwrap();
        assert_eq!(
            plain_params(&w),
            Some(BTreeMap::from([("7".to_string(), 0.25)]))
        );
        assert_eq!(plain_params(&[("Cutoff".into(), json!(0.2))]), None);
        assert_eq!(plain_params(&[("7".into(), json!("800 Hz"))]), None);
        assert_eq!(plain_params(&[("7".into(), json!(3))]), None);
        let mut args: Args = serde_json::from_value(json!({"param": "Mix"})).unwrap();
        assert!(wanted_params(&mut args).is_err());
        assert!(wanted_params(&mut Args::new()).is_err());
    }
}
