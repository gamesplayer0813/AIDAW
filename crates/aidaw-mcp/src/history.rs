//! The AI's undo: before every edit made through these tools, the song as it
//! was is kept on disk (the server may be a new process for every call), so
//! the `undo` tool can bring it back.
//!
//! Each project gets a folder of numbered entries under the user's app data
//! folder (`%APPDATA%\AIDAW\history` on Windows), out of sight of the song's
//! own folder. An entry also remembers a fingerprint of the song right after
//! the edit: undo only goes ahead while the file still holds exactly that, so
//! it never throws away what the human changed in the app since.

use std::fs;
use std::path::{Path, PathBuf};

use aidaw_core::{io as pio, Project};
use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};

/// Edits kept per project.
pub const KEEP: usize = 30;

#[derive(Serialize, Deserialize)]
struct Entry {
    /// What the edit was, e.g. `add_chords {"track":"Keys",...}`.
    what: String,
    /// [`fingerprint`] of the song right after the edit.
    after: String,
    /// The song before the edit.
    before: Project,
}

/// What [`History::undo`] brought back.
#[derive(Debug)]
pub struct Undone {
    pub project: Project,
    pub what: String,
}

pub struct History {
    dir: PathBuf,
}

/// The app data folder: `%APPDATA%` on Windows, `~/.config` elsewhere.
fn app_dir() -> PathBuf {
    std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(std::env::temp_dir)
        .join("AIDAW")
}

fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, &b| {
        (h ^ b as u64).wrapping_mul(0x0100_0000_01b3)
    })
}

/// A short fingerprint of the music (the loop region and playhead requests
/// left out: undo neither restores nor minds them). Notes count in playing
/// order, the order the file is read in.
fn fingerprint(p: &Project) -> String {
    let mut music = p.clone();
    music.transport = Default::default();
    music.sort_notes();
    let json = serde_json::to_vec(&music).unwrap_or_default();
    format!("{:016x}-{}", fnv(&json), json.len())
}

impl History {
    /// The history of the project file at `path` (an absolute path).
    pub fn of(path: &Path) -> History {
        let key = path.to_string_lossy();
        // Windows paths are not case sensitive.
        let key = if cfg!(windows) {
            key.to_lowercase()
        } else {
            key.into_owned()
        };
        History {
            dir: app_dir()
                .join("history")
                .join(format!("{:016x}", fnv(key.as_bytes()))),
        }
    }

    /// The entries' numbers, oldest first.
    fn numbers(&self) -> Vec<u64> {
        let mut n: Vec<u64> = fs::read_dir(&self.dir)
            .into_iter()
            .flatten()
            .filter_map(|e| {
                e.ok()?
                    .file_name()
                    .to_str()?
                    .strip_suffix(".json")?
                    .parse()
                    .ok()
            })
            .collect();
        n.sort_unstable();
        n
    }

    fn entry_path(&self, n: u64) -> PathBuf {
        self.dir.join(format!("{n:08}.json"))
    }

    /// How many edits can be undone.
    pub fn len(&self) -> usize {
        self.numbers().len()
    }

    /// Remember `before`, the song as it was before an edit that made `after`.
    pub fn record(&self, before: &Project, after: &Project, what: &str) -> Result<()> {
        fs::create_dir_all(&self.dir)
            .with_context(|| format!("creating {}", self.dir.display()))?;
        let numbers = self.numbers();
        let next = numbers.last().map_or(1, |n| n + 1);
        let entry = Entry {
            what: what.to_string(),
            after: fingerprint(after),
            before: before.clone(),
        };
        fs::write(self.entry_path(next), serde_json::to_vec(&entry)?)?;
        let too_many = (numbers.len() + 1).saturating_sub(KEEP);
        for n in numbers.into_iter().take(too_many) {
            let _ = fs::remove_file(self.entry_path(n));
        }
        Ok(())
    }

    /// Forget everything (a new song was created at this path).
    pub fn clear(&self) {
        let _ = fs::remove_dir_all(&self.dir);
    }

    /// The newest entry, as the song before that edit; `current` is what the
    /// file holds now and must be what that edit left. The entry is removed
    /// once [`Self::forget_newest`] is called (after the song was saved).
    pub fn undo(&self, current: &Project) -> Result<Undone> {
        let Some(&n) = self.numbers().last() else {
            bail!(
                "there is nothing to undo: no edits made through these tools are remembered for this song \
                 (the last {KEEP} are kept). Changes made in the AIDAW app are undone there with Ctrl+Z"
            );
        };
        let text = fs::read(self.entry_path(n))?;
        let entry: Entry = serde_json::from_slice(&text).map_err(|e| {
            anyhow!("the undo history of this song is damaged ({e}); nothing was changed")
        })?;
        if entry.after != fingerprint(current) {
            bail!(
                "the song was changed after your last edit ({}), probably by the human in the AIDAW app, \
                 so undoing it would throw their changes away too. Nothing was changed. Fix things with a new \
                 edit instead, or ask the human to press Ctrl+Z in the app",
                entry.what
            );
        }
        let mut project = entry.before;
        // Undo is about the music: the loop and the playhead stay as they are.
        project.transport = current.transport.clone();
        Ok(Undone {
            project,
            what: entry.what,
        })
    }

    pub fn forget_newest(&self) {
        if let Some(&n) = self.numbers().last() {
            let _ = fs::remove_file(self.entry_path(n));
        }
    }
}

/// Load, check the history, save: undo `steps` edits on the project at
/// `path`. Returns what was undone, newest first.
pub fn undo(path: &Path, steps: usize) -> Result<(Vec<String>, Project)> {
    let history = History::of(path);
    let mut undone = Vec::new();
    let mut current = pio::load(path)?;
    for _ in 0..steps {
        let before = pio::modified(path);
        current = pio::load(path)?;
        let step = match history.undo(&current) {
            Ok(step) => step,
            // Report what was undone so far, if anything, rather than failing.
            Err(e) if !undone.is_empty() => {
                undone.push(format!("(stopped there: {e:#})"));
                break;
            }
            Err(e) => return Err(e),
        };
        if pio::modified(path) != before {
            bail!(
                "{} changed while undoing; nothing more was changed, please look at it again",
                path.display()
            );
        }
        pio::save(&step.project, path)?;
        history.forget_newest();
        undone.push(step.what);
        current = step.project;
    }
    Ok((undone, current))
}

#[cfg(test)]
mod tests {
    use super::*;
    use aidaw_core::edit::{apply_all, Op};

    #[test]
    fn edits_come_back_in_order_and_human_changes_are_safe() {
        let dir = std::env::temp_dir().join(format!("aidaw-history-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let history = History { dir: dir.join("h") };
        let mut song = Project::new("x", 120.0);
        let mut versions = vec![song.clone()];
        for name in ["A", "B", "C"] {
            let before = song.clone();
            apply_all(
                &mut song,
                vec![Op::AddTrack {
                    name: name.into(),
                    instrument: None,
                    gain: None,
                    pan: None,
                    reverb: None,
                }],
            )
            .unwrap();
            history
                .record(&before, &song, &format!("add_track {name}"))
                .unwrap();
            versions.push(song.clone());
        }
        assert_eq!(history.len(), 3);

        // The playhead moved since: that is no reason to refuse.
        let mut now = song.clone();
        now.transport.cue = Some(aidaw_core::Cue {
            beat: 4.0,
            play: None,
            id: 9,
        });
        let back = history.undo(&now).unwrap();
        assert_eq!(back.what, "add_track C");
        assert!(back.project.same_music(&versions[2]));
        assert_eq!(back.project.transport, now.transport, "kept as it is");
        history.forget_newest();

        // The human renamed the song in the app: undo would lose that.
        let mut changed = back.project.clone();
        changed.name = "renamed".into();
        let err = history.undo(&changed).unwrap_err().to_string();
        assert!(
            err.contains("changed after your last edit (add_track B)"),
            "{err}"
        );
        assert_eq!(history.len(), 2, "a refused undo keeps the entry");

        // Only the newest KEEP entries are kept.
        for _ in 0..KEEP + 5 {
            history.record(&song, &song, "same").unwrap();
        }
        assert_eq!(history.len(), KEEP);
        history.clear();
        assert!(history
            .undo(&song)
            .unwrap_err()
            .to_string()
            .contains("nothing to undo"));
        fs::remove_dir_all(&dir).ok();
    }
}
