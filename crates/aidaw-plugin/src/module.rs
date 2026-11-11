//! Loading a VST3 module (a `.vst3` bundle folder, or on Windows also a
//! single `.vst3` file) and reading the plugin classes its factory offers.
//!
//! A module, once loaded, stays loaded until the program ends: many plugins
//! do not survive being unloaded and loaded again, and a plugin's code may
//! still be running (an editor window, a worker thread of its own) after the
//! host is done with it.

use std::collections::HashMap;
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::{Arc, Mutex, OnceLock};

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use vst3::Steinberg::*;
use vst3::{ComPtr, Interface};

use crate::host::{self, iid};
use crate::util;

/// The class category of a plugin's audio half.
pub const AUDIO_MODULE_CLASS: &str = "Audio Module Class";

/// One plugin a module offers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClassInfo {
    pub name: String,
    /// The class id as 32 hex digits.
    pub uid: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub vendor: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub version: String,
    /// What kind of plugin it says it is, e.g. "Instrument|Synth" or
    /// "Fx|Reverb".
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub sub_categories: String,
}

impl ClassInfo {
    /// Instruments make sound from notes; everything else is an effect.
    pub fn is_instrument(&self) -> bool {
        self.sub_categories
            .split('|')
            .any(|c| c.trim().eq_ignore_ascii_case("Instrument"))
    }
}

/// A loaded module and its plugin factory.
pub struct Module {
    /// The `.vst3` the user installed (bundle folder or file).
    pub path: PathBuf,
    /// The code inside it that was loaded.
    pub binary: PathBuf,
    factory: ComPtr<IPluginFactory>,
}

impl Module {
    /// Load the module at `path` (a `.vst3` bundle or file), or return the
    /// one already loaded from there.
    pub fn load(path: &Path) -> Result<Arc<Module>> {
        static LOADED: OnceLock<Mutex<HashMap<PathBuf, Arc<Module>>>> = OnceLock::new();
        let key = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let mut loaded = LOADED
            .get_or_init(Default::default)
            .lock()
            .map_err(|_| anyhow!("plugin loader is broken (a plugin crashed while loading)"))?;
        if let Some(m) = loaded.get(&key) {
            return Ok(m.clone());
        }
        let binary = binary_path(&key)?;
        let factory = unsafe { open(&binary) }
            .with_context(|| format!("could not load {}", path.display()))?;
        // Plugins that ask the host for objects (messages) find it here.
        if let Some(f3) = factory.cast::<IPluginFactory3>() {
            unsafe { f3.setHostContext(host::host_context()) };
        }
        let module = Arc::new(Module {
            path: key.clone(),
            binary,
            factory,
        });
        loaded.insert(key, module.clone());
        Ok(module)
    }

    /// The vendor the factory names (classes may name their own).
    pub fn vendor(&self) -> String {
        let mut info: PFactoryInfo = unsafe { std::mem::zeroed() };
        if unsafe { self.factory.getFactoryInfo(&mut info) } == kResultOk {
            util::from_c(&info.vendor)
        } else {
            String::new()
        }
    }

    /// Every class the factory offers: (category, class).
    pub fn all_classes(&self) -> Vec<(String, ClassInfo)> {
        let vendor = self.vendor();
        let n = unsafe { self.factory.countClasses() }.max(0);
        let f2 = self.factory.cast::<IPluginFactory2>();
        let f3 = self.factory.cast::<IPluginFactory3>();
        let mut out = Vec::new();
        for i in 0..n {
            let mut class = None;
            if let Some(f3) = &f3 {
                let mut w: PClassInfoW = unsafe { std::mem::zeroed() };
                if unsafe { f3.getClassInfoUnicode(i, &mut w) } == kResultOk {
                    class = Some((
                        util::from_c(&w.category),
                        ClassInfo {
                            name: util::from_utf16(&w.name),
                            uid: util::uid_to_hex(&w.cid),
                            vendor: util::from_utf16(&w.vendor),
                            version: util::from_utf16(&w.version),
                            sub_categories: util::from_c(&w.subCategories),
                        },
                    ));
                }
            }
            if class.is_none() {
                if let Some(f2) = &f2 {
                    let mut c: PClassInfo2 = unsafe { std::mem::zeroed() };
                    if unsafe { f2.getClassInfo2(i, &mut c) } == kResultOk {
                        class = Some((
                            util::from_c(&c.category),
                            ClassInfo {
                                name: util::from_c(&c.name),
                                uid: util::uid_to_hex(&c.cid),
                                vendor: util::from_c(&c.vendor),
                                version: util::from_c(&c.version),
                                sub_categories: util::from_c(&c.subCategories),
                            },
                        ));
                    }
                }
            }
            if class.is_none() {
                let mut c: PClassInfo = unsafe { std::mem::zeroed() };
                if unsafe { self.factory.getClassInfo(i, &mut c) } == kResultOk {
                    class = Some((
                        util::from_c(&c.category),
                        ClassInfo {
                            name: util::from_c(&c.name),
                            uid: util::uid_to_hex(&c.cid),
                            vendor: String::new(),
                            version: String::new(),
                            sub_categories: String::new(),
                        },
                    ));
                }
            }
            if let Some((category, mut class)) = class {
                if class.vendor.is_empty() {
                    class.vendor = vendor.clone();
                }
                out.push((category, class));
            }
        }
        out
    }

    /// The plugins in this module (their audio halves).
    pub fn classes(&self) -> Vec<ClassInfo> {
        self.all_classes()
            .into_iter()
            .filter(|(category, _)| category == AUDIO_MODULE_CLASS)
            .map(|(_, c)| c)
            .collect()
    }

    /// Make an object of class `cid`, asking for interface `I`.
    pub fn create<I: Interface>(&self, cid: &TUID) -> Option<ComPtr<I>> {
        let want = iid::<I>();
        let mut obj: *mut c_void = ptr::null_mut();
        let result = unsafe {
            self.factory
                .createInstance(cid.as_ptr(), want.as_ptr(), &mut obj)
        };
        if result != kResultOk || obj.is_null() {
            return None;
        }
        // createInstance hands over a reference, which the ComPtr now owns.
        unsafe { ComPtr::from_raw(obj as *mut I) }
    }
}

/// Where the code of the module at `path` is.
pub fn binary_path(path: &Path) -> Result<PathBuf> {
    if path.is_file() {
        if cfg!(windows) {
            // An old-style plugin: the .vst3 file is the DLL itself.
            return Ok(path.to_path_buf());
        }
        bail!("{} is a file, not a VST3 bundle folder", path.display());
    }
    if !path.is_dir() {
        bail!("{} does not exist", path.display());
    }
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    for arch in arch_folders() {
        let dir = path.join("Contents").join(arch);
        let named = dir.join(format!("{stem}.{}", BINARY_EXTENSION));
        if named.is_file() {
            return Ok(named);
        }
        // Some bundles name their binary differently from the folder.
        if let Ok(entries) = std::fs::read_dir(&dir) {
            let mut found: Vec<PathBuf> = entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.is_file()
                        && p.extension()
                            .is_some_and(|e| e.eq_ignore_ascii_case(BINARY_EXTENSION))
                })
                .collect();
            found.sort();
            if let Some(p) = found.into_iter().next() {
                return Ok(p);
            }
        }
    }
    bail!(
        "{} has no {} binary for this computer (looked in Contents/{})",
        path.display(),
        BINARY_EXTENSION,
        arch_folders().join(", Contents/")
    )
}

#[cfg(windows)]
const BINARY_EXTENSION: &str = "vst3";
#[cfg(not(windows))]
const BINARY_EXTENSION: &str = "so";

/// The bundle sub-folders holding code this computer can run, best first.
fn arch_folders() -> Vec<&'static str> {
    if cfg!(windows) {
        match std::env::consts::ARCH {
            "aarch64" => vec!["arm64-win", "arm64x-win", "arm64ec-win"],
            "x86" => vec!["x86-win"],
            _ => vec!["x86_64-win"],
        }
    } else {
        match std::env::consts::ARCH {
            "aarch64" => vec!["aarch64-linux"],
            "x86" => vec!["i386-linux"],
            _ => vec!["x86_64-linux"],
        }
    }
}

/// Load the binary, run its start-up function and get its factory. The
/// library is never unloaded (see the module documentation).
#[cfg(windows)]
unsafe fn open(binary: &Path) -> Result<ComPtr<IPluginFactory>> {
    use libloading::os::windows::{Library, LOAD_WITH_ALTERED_SEARCH_PATH};
    // The altered search path lets a plugin find DLLs it ships next to itself.
    let lib = unsafe { Library::load_with_flags(binary, LOAD_WITH_ALTERED_SEARCH_PATH) }?;
    let lib: &'static Library = Box::leak(Box::new(lib));
    if let Ok(init) = unsafe { lib.get::<unsafe extern "system" fn() -> bool>(b"InitDll\0") } {
        if !unsafe { init() } {
            bail!("the plugin refused to start (InitDll failed)");
        }
    }
    unsafe { factory_of(lib.get(b"GetPluginFactory\0").ok().map(|s| *s)) }
}

#[cfg(unix)]
unsafe fn open(binary: &Path) -> Result<ComPtr<IPluginFactory>> {
    use libloading::os::unix::{Library, RTLD_LOCAL, RTLD_NOW};
    let lib = unsafe { Library::open(Some(binary), RTLD_NOW | RTLD_LOCAL) }?;
    let raw = lib.into_raw();
    let lib: &'static Library = Box::leak(Box::new(unsafe { Library::from_raw(raw) }));
    if let Ok(entry) =
        unsafe { lib.get::<unsafe extern "system" fn(*mut c_void) -> bool>(b"ModuleEntry\0") }
    {
        if !unsafe { entry(raw) } {
            bail!("the plugin refused to start (ModuleEntry failed)");
        }
    }
    unsafe { factory_of(lib.get(b"GetPluginFactory\0").ok().map(|s| *s)) }
}

type GetFactory = unsafe extern "system" fn() -> *mut IPluginFactory;

unsafe fn factory_of(get: Option<GetFactory>) -> Result<ComPtr<IPluginFactory>> {
    let Some(get) = get else {
        bail!("not a VST3 plugin (it has no GetPluginFactory)");
    };
    // The factory comes with a reference for the host.
    unsafe { ComPtr::from_raw(get()) }.ok_or_else(|| anyhow!("the plugin has no factory"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundles_are_resolved_to_their_binary() {
        let dir = std::env::temp_dir().join(format!("aidaw-bundle-{}", std::process::id()));
        let bundle = dir.join("Thing.vst3");
        let arch = bundle.join("Contents").join(arch_folders()[0]);
        std::fs::create_dir_all(&arch).unwrap();
        assert!(binary_path(&bundle).is_err(), "no binary yet");
        let other = arch.join(format!("Other.{BINARY_EXTENSION}"));
        std::fs::write(&other, b"").unwrap();
        assert_eq!(binary_path(&bundle).unwrap(), other);
        let named = arch.join(format!("Thing.{BINARY_EXTENSION}"));
        std::fs::write(&named, b"").unwrap();
        assert_eq!(binary_path(&bundle).unwrap(), named, "the bundle's own name wins");
        assert!(binary_path(&dir.join("Missing.vst3")).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn instruments_are_told_from_effects() {
        let mut c = ClassInfo {
            name: "X".into(),
            uid: "0".repeat(32),
            vendor: String::new(),
            version: String::new(),
            sub_categories: "Instrument|Synth".into(),
        };
        assert!(c.is_instrument());
        c.sub_categories = "Fx|Reverb".into();
        assert!(!c.is_instrument());
    }
}
