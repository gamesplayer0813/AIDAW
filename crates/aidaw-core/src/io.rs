//! Loading and saving project files.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};

use crate::model::Project;

pub const EXTENSION: &str = "aidaw.json";

pub fn load(path: &Path) -> Result<Project> {
    let text = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))
}

/// Write atomically (temp file + rename) so a reader such as the GUI's file
/// watcher never sees a half-written project.
pub fn save(project: &Project, path: &Path) -> Result<()> {
    let text = serde_json::to_string_pretty(project)?;
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, text).with_context(|| format!("writing {}", tmp.display()))?;
    fs::rename(&tmp, path).with_context(|| format!("replacing {}", path.display()))?;
    Ok(())
}
