//! Finding installed plugins: which folders to look in, what each `.vst3`
//! there offers, and a cache (`plugins.json` next to AIDAW's settings) so
//! the slow part, loading every module, happens once per installed version.
//! Modules are opened in worker processes, so a plugin that crashes while
//! being looked at only takes its worker down.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::Mutex;
use std::time::{Duration, UNIX_EPOCH};

use aidaw_core::PluginRef;
use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::module::ClassInfo;
use crate::worker::{Job, Worker};

/// What a plugin does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// Plays notes.
    Instrument,
    /// Changes sound.
    Effect,
}

/// An installed plugin.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PluginInfo {
    pub name: String,
    /// Class id, 32 hex digits: what a song stores to find it again.
    pub uid: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub vendor: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub version: String,
    pub kind: Kind,
    /// What it says it is, e.g. "Instrument|Synth" or "Fx|Reverb".
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub category: String,
    /// The `.vst3` it is in.
    pub path: String,
}

impl PluginInfo {
    fn from_class(c: ClassInfo, path: &Path) -> PluginInfo {
        PluginInfo {
            kind: if c.is_instrument() {
                Kind::Instrument
            } else {
                Kind::Effect
            },
            name: c.name,
            uid: c.uid,
            vendor: c.vendor,
            version: c.version,
            category: c.sub_categories,
            path: path.display().to_string(),
        }
    }

    /// A fresh song entry for this plugin (it starts with its defaults).
    pub fn to_ref(&self) -> PluginRef {
        let mut p = PluginRef::new(self.name.clone(), self.uid.clone());
        p.vendor = self.vendor.clone();
        p.path = self.path.clone();
        p
    }
}

/// One `.vst3` as last scanned.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Scanned {
    path: String,
    /// Size and modification time (seconds since 1970) of the module when
    /// it was scanned: a change means it was updated and is scanned again.
    size: u64,
    modified: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    plugins: Vec<PluginInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

/// The plugin list and where to look for plugins (`plugins.json`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Store {
    /// Folders to look in besides the standard ones.
    #[serde(default)]
    pub folders: Vec<String>,
    #[serde(default)]
    modules: Vec<Scanned>,
    #[serde(skip)]
    file: Option<PathBuf>,
}

/// What a scan found.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ScanReport {
    /// Modules looked at this time (the rest came from the cache).
    pub scanned: usize,
    pub plugins: usize,
    /// (module, why it could not be used).
    pub problems: Vec<(String, String)>,
}

/// Where AIDAW keeps per-user files: `%APPDATA%\AIDAW` on Windows,
/// `~/.config/AIDAW` elsewhere, or `$AIDAW_CONFIG_DIR` when set.
pub fn config_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("AIDAW_CONFIG_DIR").filter(|d| !d.is_empty()) {
        return Some(PathBuf::from(dir));
    }
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("AIDAW"))
}

/// The standard places plugins are installed, then `$AIDAW_VST3_PATH`.
pub fn standard_folders() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if cfg!(windows) {
        if let Some(common) = std::env::var_os("CommonProgramFiles") {
            out.push(PathBuf::from(common).join("VST3"));
        } else {
            out.push(PathBuf::from(r"C:\Program Files\Common Files\VST3"));
        }
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            out.push(
                PathBuf::from(local)
                    .join("Programs")
                    .join("Common")
                    .join("VST3"),
            );
        }
    } else {
        if let Some(home) = std::env::var_os("HOME") {
            out.push(PathBuf::from(home).join(".vst3"));
        }
        out.push(PathBuf::from("/usr/lib/vst3"));
        out.push(PathBuf::from("/usr/local/lib/vst3"));
    }
    if let Some(extra) = std::env::var_os("AIDAW_VST3_PATH") {
        out.extend(std::env::split_paths(&extra).filter(|p| !p.as_os_str().is_empty()));
    }
    out
}

/// Every `.vst3` in `folders` (and their sub-folders), sorted.
pub fn find_modules(folders: &[PathBuf]) -> Vec<PathBuf> {
    fn walk(dir: &Path, depth: u32, out: &mut Vec<PathBuf>, seen: &mut HashSet<PathBuf>) {
        if depth > 8 {
            return;
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for e in entries.flatten() {
            let path = e.path();
            let is_vst3 = path
                .extension()
                .is_some_and(|x| x.eq_ignore_ascii_case("vst3"));
            if is_vst3 {
                if seen.insert(path.canonicalize().unwrap_or_else(|_| path.clone())) {
                    out.push(path);
                }
            } else if path.is_dir() {
                walk(&path, depth + 1, out, seen);
            }
        }
    }
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for f in folders {
        walk(f, 0, &mut out, &mut seen);
    }
    out.sort();
    out
}

/// Size and modification time of a module's code, to notice updates.
fn stamp(path: &Path) -> (u64, u64) {
    let binary = crate::module::binary_path(path).unwrap_or_else(|_| path.to_path_buf());
    let Ok(meta) = std::fs::metadata(&binary) else {
        return (0, 0);
    };
    let modified = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs());
    (meta.len(), modified)
}

/// How long a worker may stay silent while looking at one module.
const SCAN_TIMEOUT: Duration = Duration::from_secs(30);
/// Modules looked at side by side.
const PARALLEL: usize = 4;

impl Store {
    /// The store's usual file.
    pub fn default_path() -> Option<PathBuf> {
        config_dir().map(|d| d.join("plugins.json"))
    }

    /// Read the store from `file` (an empty one if it isn't there yet).
    pub fn open(file: Option<PathBuf>) -> Store {
        let mut store: Store = file
            .as_deref()
            .and_then(|f| std::fs::read_to_string(f).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        store.file = file;
        store
    }

    /// Read the usual store.
    pub fn load() -> Store {
        Store::open(Store::default_path())
    }

    /// Write the store back to its file (atomically).
    pub fn save(&self) -> Result<()> {
        let Some(file) = &self.file else {
            return Ok(());
        };
        if let Some(dir) = file.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = file.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(self)?)?;
        std::fs::rename(&tmp, file).with_context(|| format!("could not save {}", file.display()))
    }

    /// All folders this store looks in.
    pub fn all_folders(&self) -> Vec<PathBuf> {
        let mut out = standard_folders();
        out.extend(
            self.folders
                .iter()
                .filter(|f| !f.trim().is_empty())
                .map(PathBuf::from),
        );
        out
    }

    /// Whether anything was ever scanned.
    pub fn is_empty(&self) -> bool {
        self.modules.is_empty()
    }

    /// Look at every `.vst3` in the folders: new and updated ones through
    /// `worker`, the rest from the cache (all of them when `force`).
    /// `progress` hears (done, total, module) as it goes. The store is
    /// saved afterwards.
    pub fn scan(
        &mut self,
        worker: &Worker,
        force: bool,
        progress: &mut dyn FnMut(usize, usize, &str),
    ) -> ScanReport {
        let found = find_modules(&self.all_folders());
        let mut report = ScanReport::default();
        let mut keep = Vec::new();
        let mut todo = Vec::new();
        for path in found {
            let key = path.display().to_string();
            let (size, modified) = stamp(&path);
            match self.modules.iter().find(|m| m.path == key) {
                Some(m) if !force && m.size == size && m.modified == modified => {
                    keep.push(m.clone())
                }
                _ => todo.push((path, size, modified)),
            }
        }
        let total = todo.len();
        let queue = Mutex::new(todo.into_iter());
        let (tx, rx) = mpsc::channel::<Scanned>();
        std::thread::scope(|s| {
            for _ in 0..PARALLEL.min(total) {
                let tx = tx.clone();
                let queue = &queue;
                s.spawn(move || loop {
                    let next = queue.lock().ok().and_then(|mut q| q.next());
                    let Some((path, size, modified)) = next else {
                        break;
                    };
                    let (plugins, error) = match scan_module(worker, &path) {
                        Ok(classes) => (
                            classes
                                .into_iter()
                                .map(|c| PluginInfo::from_class(c, &path))
                                .collect(),
                            None,
                        ),
                        Err(e) => (Vec::new(), Some(format!("{e:#}"))),
                    };
                    let _ = tx.send(Scanned {
                        path: path.display().to_string(),
                        size,
                        modified,
                        plugins,
                        error,
                    });
                });
            }
            drop(tx);
            for (done, m) in rx.iter().enumerate() {
                progress(done + 1, total, &m.path);
                keep.push(m);
            }
        });
        keep.sort_by(|a, b| a.path.cmp(&b.path));
        report.scanned = total;
        self.modules = keep;
        report.plugins = self.plugins().len();
        report.problems = self.problems();
        let _ = self.save();
        report
    }

    /// Every installed plugin, instruments first, by name (one entry per
    /// uid; the first module found wins).
    pub fn plugins(&self) -> Vec<PluginInfo> {
        let mut seen = HashSet::new();
        let mut out: Vec<PluginInfo> = self
            .modules
            .iter()
            .flat_map(|m| m.plugins.iter())
            .filter(|p| seen.insert(p.uid.to_ascii_uppercase()))
            .cloned()
            .collect();
        out.sort_by_key(|p| (p.kind, p.name.to_lowercase()));
        out
    }

    /// Modules that could not be used, and why.
    pub fn problems(&self) -> Vec<(String, String)> {
        self.modules
            .iter()
            .filter_map(|m| Some((m.path.clone(), m.error.clone()?)))
            .collect()
    }

    /// The plugin with class id `uid`.
    pub fn find(&self, uid: &str) -> Option<PluginInfo> {
        self.modules
            .iter()
            .flat_map(|m| m.plugins.iter())
            .find(|p| p.uid.eq_ignore_ascii_case(uid))
            .cloned()
    }

    /// The plugin a person or the AI means by `name`: its uid, its exact
    /// name, or a unique part of its name (case does not matter).
    pub fn resolve(&self, name: &str, kind: Option<Kind>) -> Result<PluginInfo> {
        let name = name.trim();
        let all: Vec<PluginInfo> = self
            .plugins()
            .into_iter()
            .filter(|p| kind.is_none_or(|k| p.kind == k))
            .collect();
        if let Some(p) = all.iter().find(|p| p.uid.eq_ignore_ascii_case(name)) {
            return Ok(p.clone());
        }
        let lower = name.to_lowercase();
        if let Some(p) = all.iter().find(|p| p.name.to_lowercase() == lower) {
            return Ok(p.clone());
        }
        let partial: Vec<&PluginInfo> = all
            .iter()
            .filter(|p| p.name.to_lowercase().contains(&lower))
            .collect();
        let what = match kind {
            Some(Kind::Instrument) => "instrument plugin",
            Some(Kind::Effect) => "effect plugin",
            None => "plugin",
        };
        match partial.as_slice() {
            [one] => Ok((*one).clone()),
            [] if all.is_empty() => bail!(
                "no {what}s are installed (or they were not scanned yet: list_plugins with rescan=true looks again)"
            ),
            [] => bail!(
                "no {what} called {name:?}; installed: {}",
                all.iter().map(|p| p.name.as_str()).collect::<Vec<_>>().join(", ")
            ),
            many => bail!(
                "{name:?} could mean {}; use the full name or the uid",
                many.iter().map(|p| p.name.as_str()).collect::<Vec<_>>().join(", ")
            ),
        }
    }

    /// Where the plugin a song asks for is installed on this computer.
    pub fn locate(&self, p: &PluginRef) -> Result<PathBuf> {
        if let Some(found) = self.find(&p.uid) {
            let path = PathBuf::from(&found.path);
            if path.exists() {
                return Ok(path);
            }
        }
        let hint = PathBuf::from(&p.path);
        if !p.path.is_empty() && hint.exists() {
            return Ok(hint);
        }
        Err(anyhow!(
            "{} is not installed on this computer (if it is, scan for plugins again)",
            p.name
        ))
    }
}

/// The plugins of one module, looked at by `worker`.
pub fn scan_module(worker: &Worker, path: &Path) -> Result<Vec<ClassInfo>> {
    let job = Job::Scan {
        path: path.display().to_string(),
    };
    let done = worker.run(&job, SCAN_TIMEOUT, &mut |_| {})?;
    let classes = done
        .get("classes")
        .cloned()
        .ok_or_else(|| anyhow!("the scan worker sent no classes"))?;
    Ok(serde_json::from_value(classes)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(name: &str, uid: &str, kind: Kind) -> PluginInfo {
        PluginInfo {
            name: name.into(),
            uid: uid.into(),
            vendor: String::new(),
            version: String::new(),
            kind,
            category: String::new(),
            path: "/nowhere/x.vst3".into(),
        }
    }

    fn store() -> Store {
        Store {
            folders: Vec::new(),
            modules: vec![
                Scanned {
                    path: "/a.vst3".into(),
                    size: 1,
                    modified: 1,
                    plugins: vec![
                        info("Big Reverb", &"A".repeat(32), Kind::Effect),
                        info("Bass Synth", &"B".repeat(32), Kind::Instrument),
                    ],
                    error: None,
                },
                Scanned {
                    path: "/b.vst3".into(),
                    size: 1,
                    modified: 1,
                    plugins: vec![
                        info("Small Reverb", &"C".repeat(32), Kind::Effect),
                        info("Bass Synth copy", &"B".repeat(32), Kind::Instrument),
                    ],
                    error: None,
                },
                Scanned {
                    path: "/broken.vst3".into(),
                    size: 1,
                    modified: 1,
                    plugins: Vec::new(),
                    error: Some("crashed".into()),
                },
            ],
            file: None,
        }
    }

    #[test]
    fn plugins_are_listed_once_instruments_first() {
        let names: Vec<String> = store().plugins().into_iter().map(|p| p.name).collect();
        assert_eq!(names, ["Bass Synth", "Big Reverb", "Small Reverb"]);
        assert_eq!(
            store().problems(),
            [("/broken.vst3".into(), "crashed".into())]
        );
    }

    #[test]
    fn names_resolve_by_uid_name_or_unique_part() {
        let s = store();
        assert_eq!(s.resolve("big reverb", None).unwrap().uid, "A".repeat(32));
        assert_eq!(
            s.resolve(&"c".repeat(32), None).unwrap().name,
            "Small Reverb"
        );
        assert_eq!(s.resolve("bass", None).unwrap().name, "Bass Synth");
        let e = s.resolve("reverb", None).unwrap_err().to_string();
        assert!(e.contains("could mean"), "{e}");
        assert!(s.resolve("bass", Some(Kind::Effect)).is_err());
        let e = s.resolve("piano", None).unwrap_err().to_string();
        assert!(e.contains("installed: Bass Synth"), "{e}");
    }

    #[test]
    fn songs_find_plugins_by_uid_then_by_path() {
        let dir = std::env::temp_dir().join(format!("aidaw-locate-{}", std::process::id()));
        let bundle = dir.join("Moved.vst3");
        std::fs::create_dir_all(&bundle).unwrap();
        let s = store();
        let mut p = PluginRef::new("Gone", "D".repeat(32));
        assert!(s.locate(&p).is_err());
        p.path = bundle.display().to_string();
        assert_eq!(s.locate(&p).unwrap(), bundle);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn modules_are_found_in_nested_folders_but_not_inside_bundles() {
        let dir = std::env::temp_dir().join(format!("aidaw-find-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Vendor/Synth.vst3/Contents/x86_64-linux")).unwrap();
        std::fs::write(
            dir.join("Vendor/Synth.vst3/Contents/x86_64-linux/Synth.so"),
            b"",
        )
        .unwrap();
        std::fs::write(dir.join("Old.VST3"), b"").unwrap();
        std::fs::write(dir.join("readme.txt"), b"").unwrap();
        let found = find_modules(std::slice::from_ref(&dir));
        assert_eq!(
            found,
            vec![dir.join("Old.VST3"), dir.join("Vendor/Synth.vst3")]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
