//! Loading and saving project files.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

use anyhow::{Context, Result};

use crate::model::Project;

pub const EXTENSION: &str = "aidaw.json";

pub fn load(path: &Path) -> Result<Project> {
    let text = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))
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
