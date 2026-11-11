//! Loading and saving project files.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::Value;

use crate::model::{Project, FORMAT_VERSION};

pub const EXTENSION: &str = "aidaw.json";

/// The error for a project file saved by a newer AIDAW, whose format this
/// build does not know. Reading it anyway could drop what is new, and saving
/// it back would destroy that for good, so it is refused instead.
#[derive(Debug)]
pub struct NewerFormat {
    pub found: u32,
}

impl std::fmt::Display for NewerFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "this project was saved by a newer AIDAW (file format {}, this version reads up to {FORMAT_VERSION}); update AIDAW to open it",
            self.found
        )
    }
}

impl std::error::Error for NewerFormat {}

/// Whether `e` (or what caused it) is a [`NewerFormat`] error.
pub fn is_newer_format(e: &anyhow::Error) -> bool {
    e.chain().any(|c| c.is::<NewerFormat>())
}

pub fn load(path: &Path) -> Result<Project> {
    let text = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    from_json(&text).with_context(|| format!("parsing {}", path.display()))
}

/// Parse a project from JSON in any format version this build knows,
/// upgrading older ones to [`FORMAT_VERSION`]. Notes come out in playing
/// order (see [`crate::Clip::sort_notes`]).
pub fn from_json(text: &str) -> Result<Project> {
    /// Just the version; every other field is skipped unparsed.
    #[derive(Deserialize)]
    struct Header {
        version: Option<u64>,
    }
    // A file that is not even valid JSON gets serde's message, with the line.
    let version = match serde_json::from_str::<Header>(text) {
        Ok(h) => h.version.unwrap_or(1),
        Err(_) => return Ok(serde_json::from_str::<Project>(text).map(normalized)?),
    };
    if version > u64::from(FORMAT_VERSION) {
        return Err(NewerFormat {
            found: u32::try_from(version).unwrap_or(u32::MAX),
        }
        .into());
    }
    let project = if version == u64::from(FORMAT_VERSION) {
        serde_json::from_str(text)?
    } else {
        let mut value: Value = serde_json::from_str(text)?;
        migrate(&mut value, version as u32)?;
        serde_json::from_value(value)?
    };
    Ok(normalized(project))
}

fn normalized(mut project: Project) -> Project {
    project.version = FORMAT_VERSION;
    project.sort_notes();
    project
}

/// Upgrade project JSON written in format `from` to [`FORMAT_VERSION`], one
/// version at a time. A change to the file format that old files cannot
/// simply load with (a renamed or re-meant field) bumps `FORMAT_VERSION`
/// and adds the step from the previous version here, so every older file
/// keeps opening. New optional fields need neither: old files lack them and
/// load with the default.
fn migrate(value: &mut Value, from: u32) -> Result<()> {
    for version in from..FORMAT_VERSION {
        match version {
            // Format 0 never existed in a release; it reads like 1.
            0 => {}
            // e.g. `1 => rename(value, "old_name", "new_name"),`
            _ => anyhow::bail!("no upgrade from file format {version}"),
        }
        if let Some(obj) = value.as_object_mut() {
            obj.insert("version".into(), Value::from(version + 1));
        }
    }
    Ok(())
}

/// Last modification time of `path`, if it exists. Writers compare this
/// before saving to notice that somebody else changed the file meanwhile.
pub fn modified(path: &Path) -> Option<SystemTime> {
    fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// Write atomically (temp file + rename) so a reader such as the GUI's file
/// watcher never sees a half-written project. The temp name is unique per
/// process and call, so the GUI and the MCP server saving at the same moment
/// never trample each other's temp file.
pub fn save(project: &Project, path: &Path) -> Result<()> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let text = serde_json::to_string_pretty(project)?;
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("project");
    let tmp: PathBuf = path.with_file_name(format!(
        ".{name}.{}-{}.tmp",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::write(&tmp, text).with_context(|| format!("writing {}", tmp.display()))?;
    if let Err(e) = fs::rename(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        return Err(e).with_context(|| format!("replacing {}", path.display()));
    }
    Ok(())
}

/// Whether `path` names a project file (`*.aidaw.json`, case-insensitive).
pub fn is_project_path(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.to_ascii_lowercase().ends_with(&format!(".{EXTENSION}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Note;

    #[test]
    fn files_from_a_newer_aidaw_are_refused() {
        let newer = format!(
            r#"{{"version": {}, "name": "x", "bpm": 100, "tracks": [], "new_thing": {{}}}}"#,
            FORMAT_VERSION + 1
        );
        let err = from_json(&newer).unwrap_err();
        assert!(is_newer_format(&err), "{err:#}");
        assert!(format!("{err:#}").contains("newer AIDAW"), "{err:#}");
        // Also through `load`, which adds the file name.
        let dir = std::env::temp_dir().join(format!("aidaw-io-newer-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("song.aidaw.json");
        fs::write(&path, &newer).unwrap();
        let err = load(&path).unwrap_err();
        assert!(is_newer_format(&err), "{err:#}");
        assert!(!is_newer_format(
            &load(&dir.join("missing.aidaw.json")).unwrap_err()
        ));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn old_and_unversioned_files_load_as_the_current_format() {
        for text in [
            r#"{"name": "x", "bpm": 100}"#,
            r#"{"version": 0, "name": "x", "bpm": 100}"#,
            r#"{"version": 1, "name": "x", "bpm": 100}"#,
        ] {
            let p = from_json(text).unwrap();
            assert_eq!(
                (p.version, p.name.as_str(), p.bpm),
                (FORMAT_VERSION, "x", 100.0)
            );
        }
        let mut v: Value = serde_json::json!({"version": 0});
        migrate(&mut v, 0).unwrap();
        assert_eq!(v["version"], FORMAT_VERSION);
    }

    #[test]
    fn broken_json_keeps_the_parser_message() {
        let err = from_json("{\"name\": \"x\",, }").unwrap_err();
        assert!(format!("{err:#}").contains("line 1"), "{err:#}");
        assert!(!is_newer_format(&err));
    }

    #[test]
    fn notes_load_in_playing_order() {
        let text = r#"{"name": "x", "bpm": 100, "tracks": [{"id": "t1", "name": "A",
            "instrument": {"type": "drums"}, "clips": [{"id": "c1", "start": 0, "length": 4,
            "notes": [{"pitch": 42, "start": 2, "duration": 1}, {"pitch": 38, "start": 0, "duration": 1},
                      {"pitch": 36, "start": 0, "duration": 1}]}]}]}"#;
        let p = from_json(text).unwrap();
        let notes: Vec<(f64, u8)> = p.tracks[0].clips[0]
            .notes
            .iter()
            .map(|n: &Note| (n.start, n.pitch))
            .collect();
        assert_eq!(notes, vec![(0.0, 36), (0.0, 38), (2.0, 42)]);
    }

    #[test]
    fn save_leaves_no_temp_files_and_round_trips() {
        let dir = std::env::temp_dir().join(format!("aidaw-io-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("song.aidaw.json");
        let p = Project::new("x", 100.0);
        save(&p, &path).unwrap();
        save(&p, &path).unwrap();
        assert_eq!(load(&path).unwrap(), p);
        let names: Vec<String> = fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["song.aidaw.json"]);
        assert!(is_project_path(&path));
        assert!(is_project_path(Path::new("X.AIDAW.JSON")));
        assert!(!is_project_path(Path::new("song.json")));
        fs::remove_dir_all(&dir).ok();
    }
}
